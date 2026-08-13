use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Result;
use datafusion_execution::SendableRecordBatchStream;
use foyer::{Cache, CacheBuilder};
use lance::Dataset;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use uuid::Uuid;

use crate::models::{DatasetInfo, FileEntry, FilesPage, SqlPageResponse};

use super::error::UnknownConnection;

// Maximum client dataset sessions retained in memory.
const MAX_CONNECTIONS: usize = 256;
// Maximum active SQL cursors retained in memory.
const MAX_QUERY_CURSORS: usize = 256;
// Dataset sessions expire after this period without access.
const CONNECTION_IDLE_TTL: Duration = Duration::from_secs(60 * 60);
// SQL cursors expire after this period without access.
pub(super) const QUERY_IDLE_TTL: Duration = Duration::from_secs(10 * 60);

pub(crate) struct AppState {
    pub(super) connections: Cache<Uuid, SessionEntry>,
    pub(super) queries: Cache<Uuid, Arc<AsyncMutex<QueryCursor>>>,
}

impl AppState {
    pub(crate) fn new() -> Self {
        Self {
            connections: CacheBuilder::new(MAX_CONNECTIONS).build(),
            queries: CacheBuilder::new(MAX_QUERY_CURSORS).build(),
        }
    }
}

#[derive(Clone)]
pub(super) struct SessionEntry {
    connection: ConnectedDataset,
    last_accessed: Arc<Mutex<Instant>>,
    pub(super) file_listing: Arc<AsyncMutex<Option<FileListing>>>,
}

impl SessionEntry {
    pub(super) fn new(connection: ConnectedDataset) -> Self {
        Self {
            connection,
            last_accessed: Arc::new(Mutex::new(Instant::now())),
            file_listing: Arc::new(AsyncMutex::new(None)),
        }
    }

    fn access(&self) -> Option<ConnectedDataset> {
        let now = Instant::now();
        let mut last_accessed = self
            .last_accessed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if now.duration_since(*last_accessed) >= CONNECTION_IDLE_TTL {
            return None;
        }
        *last_accessed = now;
        Some(self.connection.clone())
    }
}

#[derive(Clone)]
pub(super) struct ConnectedDataset {
    pub(super) dataset: Arc<Dataset>,
    pub(super) info: Arc<DatasetInfo>,
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
    pub(super) pending_rows: VecDeque<Value>,
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

pub(super) fn connected_session(
    state: &AppState,
    connection_id: Uuid,
) -> Result<(SessionEntry, ConnectedDataset)> {
    let entry = state
        .connections
        .get(&connection_id)
        .ok_or(UnknownConnection(connection_id))?;
    let session = entry.value().clone();
    drop(entry);
    if let Some(connection) = session.access() {
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
        error::{ApiError, UnknownConnection},
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
        state.connections.insert(
            first_id,
            SessionEntry::new(ConnectedDataset {
                dataset: dataset.clone(),
                info: first_info,
            }),
        );
        state.connections.insert(
            second_id,
            SessionEntry::new(ConnectedDataset {
                dataset,
                info: second_info,
            }),
        );

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
                .downcast_ref::<UnknownConnection>()
                .map(ToString::to_string)
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
