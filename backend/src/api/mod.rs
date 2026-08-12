mod dataset;
mod files;
mod media;
mod query;

pub(crate) use dataset::{connect_dataset, dataset_info, discover_dataset};
pub(crate) use files::{file_preview, files, transaction};
pub(crate) use media::media;
pub(crate) use query::{cancel_sql, rows, sql_page, start_sql};

use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::Result;
use axum::{
    Json,
    http::{
        HeaderValue, StatusCode,
        header::{ACCEPT_RANGES, CONTENT_RANGE},
    },
    response::IntoResponse,
};
use foyer::{Cache, CacheBuilder};
use lance::Dataset;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

const MAX_CONNECTIONS: usize = 256;
const MAX_QUERY_CURSORS: usize = 256;
const CONNECTION_IDLE_TTL: Duration = Duration::from_secs(60 * 60);
const BLOB_EXTENSION: &str = "lance.blob.v2";

pub(crate) struct AppState {
    connections: Cache<Uuid, SessionEntry>,
    queries: Cache<Uuid, Arc<AsyncMutex<query::QueryCursor>>>,
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
struct SessionEntry {
    connection: ConnectedDataset,
    last_accessed: Arc<Mutex<Instant>>,
    file_listing: Arc<AsyncMutex<Option<files::FileListing>>>,
}

impl SessionEntry {
    fn new(connection: ConnectedDataset) -> Self {
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
struct ConnectedDataset {
    dataset: Arc<Dataset>,
    dataset_uri: String,
    reference: String,
}

pub(crate) struct ApiError(anyhow::Error);

#[derive(Debug)]
struct UnknownConnection(Uuid);

#[derive(Debug)]
struct UnknownQueryCursor(Uuid);

#[derive(Debug)]
struct InvalidRequest(String);

#[derive(Debug)]
struct QueryExecutionFailed(String);

#[derive(Debug)]
struct RangeNotSatisfiable {
    size: u64,
    message: String,
}

impl std::fmt::Display for UnknownConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "connection {} was not found or has expired; reconnect the dataset",
            self.0
        )
    }
}

impl std::error::Error for UnknownConnection {}

impl std::fmt::Display for UnknownQueryCursor {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "SQL cursor {} was not found or has expired; rerun the query",
            self.0
        )
    }
}

impl std::error::Error for UnknownQueryCursor {}

impl std::fmt::Display for InvalidRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InvalidRequest {}

impl std::fmt::Display for QueryExecutionFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "SQL execution failed; rerun the query: {}",
            self.0
        )
    }
}

impl std::error::Error for QueryExecutionFailed {}

impl std::fmt::Display for RangeNotSatisfiable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RangeNotSatisfiable {}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        if let Some(error) = self.0.downcast_ref::<RangeNotSatisfiable>() {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [
                    (ACCEPT_RANGES, HeaderValue::from_static("bytes")),
                    (
                        CONTENT_RANGE,
                        HeaderValue::from_str(&format!("bytes */{}", error.size))
                            .expect("blob size always produces a valid header"),
                    ),
                ],
                Json(json!({ "error": error.to_string() })),
            )
                .into_response();
        }
        let status = if self.0.downcast_ref::<UnknownConnection>().is_some() {
            StatusCode::GONE
        } else if self.0.downcast_ref::<UnknownQueryCursor>().is_some() {
            StatusCode::NOT_FOUND
        } else if self.0.downcast_ref::<QueryExecutionFailed>().is_some() {
            StatusCode::UNPROCESSABLE_ENTITY
        } else if self.0.downcast_ref::<InvalidRequest>().is_some() {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        (status, Json(json!({ "error": self.0.to_string() }))).into_response()
    }
}

#[derive(Debug, Deserialize)]
pub(super) struct ConnectionQuery {
    connection_id: Uuid,
}

pub(crate) async fn health() -> Json<crate::models::HealthResponse> {
    Json(crate::models::HealthResponse { status: "ok" })
}

fn connected(state: &AppState, connection_id: Uuid) -> Result<ConnectedDataset> {
    connected_session(state, connection_id).map(|(_, connection)| connection)
}

fn connected_session(
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

fn is_blob_field(field: &arrow_schema::Field) -> bool {
    field
        .metadata()
        .get("ARROW:extension:name")
        .is_some_and(|name| name == BLOB_EXTENSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{RecordBatch, RecordBatchIterator};
    use arrow_schema::{DataType, Field, Schema as ArrowSchema};

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
        state.connections.insert(
            first_id,
            SessionEntry::new(ConnectedDataset {
                dataset: dataset.clone(),
                dataset_uri: "memory://first".to_string(),
                reference: "main".to_string(),
            }),
        );
        state.connections.insert(
            second_id,
            SessionEntry::new(ConnectedDataset {
                dataset,
                dataset_uri: "memory://second".to_string(),
                reference: "version:1".to_string(),
            }),
        );

        assert_eq!(
            connected(&state, first_id).unwrap().dataset_uri,
            "memory://first"
        );
        assert_eq!(
            connected(&state, second_id).unwrap().dataset_uri,
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
