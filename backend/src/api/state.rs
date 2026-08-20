use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Result;
use arrow_array::{ArrayRef, RecordBatch};
use datafusion_execution::SendableRecordBatchStream;
use lance::Dataset;
use serde::Deserialize;
use tokio::sync::{Mutex as AsyncMutex, OnceCell, mpsc};
use uuid::Uuid;

use crate::{
    cache::BoundedCache,
    models::{DatasetInfo, FileEntry, FilesPage, SqlPageResponse},
};

use super::{
    error::RequestError::{UnknownConnection, UnknownDiscovery},
    schema::MediaProjection,
};

// Maximum client dataset sessions retained in memory.
const MAX_CONNECTIONS: usize = 256;
// Maximum discovered dataset roots retained while clients choose snapshots.
const MAX_DISCOVERIES: usize = 256;
// Maximum active SQL cursors retained in memory.
const MAX_QUERY_CURSORS: usize = 256;
// Maximum materialized Blob-array rows retained server-wide. This is
// deliberately small; bounding this cache by bytes is a follow-up.
const MAX_BLOB_ARRAY_ROWS: usize = 8;
// Dataset sessions expire after this period without access.
const CONNECTION_IDLE_TTL: Duration = Duration::from_secs(60 * 60);
// Dataset discoveries expire after this period without access.
const DISCOVERY_IDLE_TTL: Duration = Duration::from_secs(60 * 60);
// SQL cursors expire after this period without access.
pub(super) const QUERY_IDLE_TTL: Duration = Duration::from_secs(10 * 60);

type BlobArrayKey = (Uuid, String, u64);
type BlobArraySlot = Arc<OnceCell<ArrayRef>>;

pub(crate) struct AppState {
    pub(super) connections: BoundedCache<Uuid, SessionEntry>,
    pub(super) discoveries: BoundedCache<Uuid, DiscoveryEntry>,
    pub(super) queries: BoundedCache<Uuid, Arc<AsyncMutex<QueryCursor>>>,
    blob_arrays: Arc<BoundedCache<BlobArrayKey, BlobArraySlot>>,
}

impl AppState {
    pub(crate) fn new() -> Self {
        Self {
            connections: BoundedCache::new(MAX_CONNECTIONS),
            discoveries: BoundedCache::new(MAX_DISCOVERIES),
            queries: BoundedCache::new(MAX_QUERY_CURSORS),
            blob_arrays: Arc::new(BoundedCache::new(MAX_BLOB_ARRAY_ROWS)),
        }
    }

    pub(super) fn connected_dataset(
        &self,
        dataset: Arc<Dataset>,
        info: Arc<DatasetInfo>,
    ) -> ConnectedDataset {
        ConnectedDataset::with_blob_cache(dataset, info, self.blob_arrays.clone())
    }
}

#[derive(Clone)]
pub(super) struct IdleEntry<T> {
    value: T,
    last_accessed: Arc<Mutex<Instant>>,
}

impl<T: Clone> IdleEntry<T> {
    pub(super) fn new(value: T) -> Self {
        Self {
            value,
            last_accessed: Arc::new(Mutex::new(Instant::now())),
        }
    }

    fn access(&self, ttl: Duration) -> Option<T> {
        let now = Instant::now();
        let mut last_accessed = self
            .last_accessed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if now.duration_since(*last_accessed) >= ttl {
            return None;
        }
        *last_accessed = now;
        Some(self.value.clone())
    }
}

pub(super) type DiscoveryEntry = IdleEntry<(Arc<Dataset>, String)>;

#[derive(Clone)]
pub(super) struct SessionEntry {
    connection: IdleEntry<ConnectedDataset>,
    pub(super) file_listing: Arc<AsyncMutex<Option<FileListing>>>,
}

impl SessionEntry {
    pub(super) fn new(connection: ConnectedDataset) -> Self {
        Self {
            connection: IdleEntry::new(connection),
            file_listing: Arc::new(AsyncMutex::new(None)),
        }
    }
}

#[derive(Clone)]
pub(super) struct ConnectedDataset {
    pub(super) dataset: Arc<Dataset>,
    pub(super) info: Arc<DatasetInfo>,
    blob_cache_namespace: Uuid,
    blob_arrays: Arc<BoundedCache<BlobArrayKey, BlobArraySlot>>,
}

impl ConnectedDataset {
    #[cfg(test)]
    pub(super) fn new(dataset: Arc<Dataset>, info: Arc<DatasetInfo>) -> Self {
        Self::with_blob_cache(
            dataset,
            info,
            Arc::new(BoundedCache::new(MAX_BLOB_ARRAY_ROWS)),
        )
    }

    fn with_blob_cache(
        dataset: Arc<Dataset>,
        info: Arc<DatasetInfo>,
        blob_arrays: Arc<BoundedCache<BlobArrayKey, BlobArraySlot>>,
    ) -> Self {
        Self {
            dataset,
            info,
            blob_cache_namespace: Uuid::new_v4(),
            blob_arrays,
        }
    }

    pub(super) fn blob_array_slot(&self, column: &str, row_address: u64) -> BlobArraySlot {
        self.blob_arrays.get_or_insert_with(
            (self.blob_cache_namespace, column.to_string(), row_address),
            || Arc::new(OnceCell::new()),
        )
    }
}

pub(super) struct FileListing {
    pub(super) receiver: mpsc::Receiver<Result<FileEntry, String>>,
    pub(super) pending: Option<FileEntry>,
    pub(super) next_offset: usize,
    pub(super) last_offset: Option<usize>,
    pub(super) last_page: Option<FilesPage>,
}

pub(super) struct QueryCursor {
    pub(super) connection_id: Uuid,
    pub(super) stream: SendableRecordBatchStream,
    pub(super) scalar_indices: Vec<usize>,
    pub(super) media_projections: Vec<MediaProjection>,
    pub(super) pending_batch: Option<RecordBatch>,
    pub(super) next_sequence: u64,
    pub(super) rows_returned: usize,
    pub(super) last_page: Option<SqlPageResponse>,
    pub(super) last_accessed: Instant,
    pub(super) done: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ConnectionQuery {
    pub(super) connection_id: Uuid,
}

pub(super) fn connected(state: &AppState, connection_id: Uuid) -> Result<ConnectedDataset> {
    connected_session(state, connection_id).map(|(_, connection)| connection)
}

pub(super) fn discovered(state: &AppState, discovery_id: Uuid) -> Result<(Arc<Dataset>, String)> {
    let entry = state
        .discoveries
        .get(&discovery_id)
        .ok_or(UnknownDiscovery(discovery_id))?;
    if let Some(dataset) = entry.access(DISCOVERY_IDLE_TTL) {
        return Ok(dataset);
    }
    state.discoveries.remove(&discovery_id);
    Err(UnknownDiscovery(discovery_id).into())
}

pub(super) fn connected_session(
    state: &AppState,
    connection_id: Uuid,
) -> Result<(SessionEntry, ConnectedDataset)> {
    let entry = state
        .connections
        .get(&connection_id)
        .ok_or(UnknownConnection(connection_id))?;
    let session = entry;
    if let Some(connection) = session.connection.access(CONNECTION_IDLE_TTL) {
        return Ok((session, connection));
    }
    state.connections.remove(&connection_id);
    Err(UnknownConnection(connection_id).into())
}

#[cfg(test)]
mod tests {
    use arrow_array::{RecordBatch, RecordBatchIterator};
    use arrow_schema::{DataType, Field, Schema as ArrowSchema};
    use axum::{http::StatusCode, response::IntoResponse};

    use super::*;
    use crate::api::{
        dataset::build_dataset_info,
        error::{ApiError, RequestError},
    };

    #[tokio::test]
    async fn isolates_connections_and_rejects_unknown_ids() {
        let schema = Arc::new(ArrowSchema::new(vec![Field::new(
            "value",
            DataType::Int32,
            false,
        )]));
        let reader = RecordBatchIterator::new(
            Vec::<std::result::Result<RecordBatch, arrow_schema::ArrowError>>::new().into_iter(),
            schema,
        );
        let uri = format!("memory://session-test-{}", Uuid::new_v4());
        let dataset = Arc::new(Dataset::write(reader, &uri, None).await.unwrap());
        let state = AppState::new();
        let first_id = Uuid::new_v4();
        let second_id = Uuid::new_v4();
        let first_info = Arc::new(
            build_dataset_info(&dataset, "memory://first", "main")
                .await
                .unwrap(),
        );
        let second_info = Arc::new(
            build_dataset_info(&dataset, "memory://second", "version:1")
                .await
                .unwrap(),
        );
        let first_connection = state.connected_dataset(dataset.clone(), first_info.clone());
        let second_connection = state.connected_dataset(dataset.clone(), second_info.clone());
        let first_slot = first_connection.blob_array_slot("blob", 7);
        assert!(Arc::ptr_eq(
            &first_slot,
            &first_connection.clone().blob_array_slot("blob", 7)
        ));
        assert!(!Arc::ptr_eq(
            &first_slot,
            &second_connection.blob_array_slot("blob", 7)
        ));

        for _ in 0..7 {
            state
                .connected_dataset(dataset.clone(), first_info.clone())
                .blob_array_slot("blob", 7);
        }
        assert!(!Arc::ptr_eq(
            &first_slot,
            &first_connection.blob_array_slot("blob", 7)
        ));

        state
            .connections
            .insert(first_id, SessionEntry::new(first_connection));
        state
            .connections
            .insert(second_id, SessionEntry::new(second_connection));

        assert_eq!(
            connected(&state, first_id).unwrap().info.uri,
            "memory://first"
        );
        assert_eq!(
            connected(&state, second_id).unwrap().info.uri,
            "memory://second"
        );

        let unknown_id = Uuid::new_v4();
        let error = match connected(&state, unknown_id) {
            Ok(_) => panic!("unknown connection unexpectedly resolved"),
            Err(error) => error,
        };
        assert_eq!(
            error
                .downcast_ref::<RequestError>()
                .and_then(|error| matches!(error, RequestError::UnknownConnection(_))
                    .then(|| error.to_string()))
                .as_deref(),
            Some(
                format!(
                    "connection {unknown_id} was not found or has expired; reconnect the dataset"
                )
                .as_str()
            )
        );
        assert_eq!(ApiError(error).into_response().status(), StatusCode::GONE);
    }
}
