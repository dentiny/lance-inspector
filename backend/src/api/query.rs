use std::{collections::VecDeque, sync::Arc, time::Instant};

use anyhow::{Result, anyhow};
use arrow_array::RecordBatch;
use arrow_json::ArrayWriter;
use arrow_schema::Schema as ArrowSchema;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use futures::{StreamExt, TryStreamExt};
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::models::{RowsResponse, SqlCursorResponse, SqlPageResponse};

use super::{
    error::{ApiError, InvalidRequest, QueryExecutionFailed, UnknownQueryCursor},
    schema::{dataset_columns, sql_result_columns},
    state::{AppState, ConnectedDataset, ConnectionQuery, QUERY_IDLE_TTL, QueryCursor, connected},
};

// Maximum number of rows accepted by the direct row-preview endpoint.
const MAX_ROWS: usize = 100;
// Number of rows returned when a row-preview request omits its limit.
const DEFAULT_ROW_LIMIT: usize = 20;
// Number of rows pulled from a SQL cursor per page.
const SQL_PAGE_ROWS: usize = 100;
// Hard cap on rows retained and returned by one SQL query.
const MAX_SQL_RESULT_ROWS: usize = 10_000;
#[derive(Debug, Deserialize)]
pub(crate) struct RowsQuery {
    connection_id: Uuid,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

pub(crate) async fn rows(
    State(state): State<Arc<AppState>>,
    Query(query): Query<RowsQuery>,
) -> Result<Json<RowsResponse>, ApiError> {
    let connection = connected(&state, query.connection_id).map_err(ApiError)?;
    read_rows(&connection, query)
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn read_rows(connection: &ConnectedDataset, query: RowsQuery) -> Result<RowsResponse> {
    let limit = query.limit.unwrap_or(DEFAULT_ROW_LIMIT).clamp(1, MAX_ROWS);
    let arrow_schema = ArrowSchema::from(&connection.dataset.manifest().schema);
    let columns = dataset_columns(&arrow_schema);

    let mut scanner = connection.dataset.scan();
    scanner.project(&columns.scalar)?;
    scanner.with_row_address();
    scanner.limit(Some(limit as i64), Some(query.offset as i64))?;
    let batches = scanner
        .try_into_stream()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut output = Vec::new();
    {
        let mut writer = ArrayWriter::new(&mut output);
        for batch in &batches {
            writer.write(batch)?;
        }
        writer.finish()?;
    }
    let rows: Vec<Value> = serde_json::from_slice(&output)?;

    Ok(RowsResponse {
        offset: query.offset,
        limit,
        total: connection.info.rows,
        columns: columns.scalar,
        media_columns: columns.media,
        rows,
    })
}

#[derive(Debug, Deserialize)]
pub(crate) struct SqlRequest {
    sql: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SqlPageQuery {
    connection_id: Uuid,
    sequence: u64,
}

pub(crate) async fn start_sql(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConnectionQuery>,
    Json(request): Json<SqlRequest>,
) -> Result<Json<SqlCursorResponse>, ApiError> {
    let connection = connected(&state, query.connection_id).map_err(ApiError)?;
    create_sql_cursor(&state, query.connection_id, &connection, &request.sql)
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn create_sql_cursor(
    state: &AppState,
    connection_id: Uuid,
    connection: &ConnectedDataset,
    sql: &str,
) -> Result<SqlCursorResponse> {
    let sql = read_only_sql(sql)?;
    let query = connection
        .dataset
        .sql(sql)
        .with_row_addr(true)
        .build()
        .await
        .map_err(|error| anyhow!(InvalidRequest(error.to_string())))?;
    let record_stream = query.into_stream().await?;
    let result_schema = record_stream.schema();
    let dataset_schema = ArrowSchema::from(&connection.dataset.manifest().schema);
    let columns = sql_result_columns(&result_schema, &dataset_schema);
    let cursor_id = Uuid::new_v4();
    state.queries.insert(
        cursor_id,
        Arc::new(AsyncMutex::new(QueryCursor {
            connection_id,
            stream: record_stream,
            scalar_indices: columns.scalar_indices,
            pending_rows: VecDeque::new(),
            next_sequence: 0,
            rows_returned: 0,
            last_page: None,
            last_accessed: Instant::now(),
            done: false,
        })),
    );
    Ok(SqlCursorResponse {
        cursor_id,
        columns: columns.scalar,
        media_columns: columns.media,
    })
}

pub(crate) async fn sql_page(
    State(state): State<Arc<AppState>>,
    Path(cursor_id): Path<Uuid>,
    Query(query): Query<SqlPageQuery>,
) -> Result<Json<SqlPageResponse>, ApiError> {
    connected(&state, query.connection_id).map_err(ApiError)?;
    read_sql_page(&state, cursor_id, query)
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn read_sql_page(
    state: &AppState,
    cursor_id: Uuid,
    query: SqlPageQuery,
) -> Result<SqlPageResponse> {
    let entry = state
        .queries
        .get(&cursor_id)
        .ok_or(UnknownQueryCursor(cursor_id))?;
    let cursor_handle = entry.value().clone();
    drop(entry);
    let mut cursor = cursor_handle.lock().await;
    if cursor.connection_id != query.connection_id
        || cursor.last_accessed.elapsed() >= QUERY_IDLE_TTL
    {
        drop(cursor);
        state.queries.remove(&cursor_id);
        return Err(UnknownQueryCursor(cursor_id).into());
    }
    cursor.last_accessed = Instant::now();
    if let Some(page) = cursor
        .last_page
        .as_ref()
        .filter(|page| page.sequence == query.sequence)
    {
        return Ok(page.clone());
    }
    if query.sequence != cursor.next_sequence {
        return Err(anyhow!(InvalidRequest(format!(
            "expected SQL page sequence {}, received {}",
            cursor.next_sequence, query.sequence
        ))));
    }

    let remaining = MAX_SQL_RESULT_ROWS.saturating_sub(cursor.rows_returned);
    let page_size = SQL_PAGE_ROWS.min(remaining);
    let mut rows = Vec::with_capacity(page_size);
    while rows.len() < page_size {
        if let Some(row) = cursor.pending_rows.pop_front() {
            rows.push(row);
            continue;
        }
        if cursor.done {
            break;
        }
        let scalar_indices = cursor.scalar_indices.clone();
        match cursor.stream.next().await {
            Some(Ok(batch)) => {
                let rows = batch
                    .project(&scalar_indices)
                    .map_err(|error| QueryExecutionFailed(error.to_string()))
                    .and_then(|batch| {
                        serialize_rows(&batch)
                            .map_err(|error| QueryExecutionFailed(error.to_string()))
                    });
                match rows {
                    Ok(rows) => cursor.pending_rows.extend(rows),
                    Err(error) => {
                        cursor.done = true;
                        drop(cursor);
                        state.queries.remove(&cursor_id);
                        return Err(error.into());
                    }
                }
            }
            Some(Err(error)) => {
                let error = QueryExecutionFailed(error.to_string());
                cursor.done = true;
                drop(cursor);
                state.queries.remove(&cursor_id);
                return Err(error.into());
            }
            None => cursor.done = true,
        }
    }

    cursor.rows_returned += rows.len();
    let capped = cursor.rows_returned >= MAX_SQL_RESULT_ROWS;
    let done = capped || (cursor.done && cursor.pending_rows.is_empty());
    if capped {
        cursor.done = true;
    }
    let page = SqlPageResponse {
        sequence: query.sequence,
        rows,
        done,
        truncated: capped,
    };
    cursor.next_sequence += 1;
    cursor.last_page = Some(page.clone());
    Ok(page)
}

pub(crate) async fn cancel_sql(
    State(state): State<Arc<AppState>>,
    Path(cursor_id): Path<Uuid>,
    Query(query): Query<ConnectionQuery>,
) -> Result<StatusCode, ApiError> {
    connected(&state, query.connection_id).map_err(ApiError)?;
    if let Some(entry) = state.queries.get(&cursor_id) {
        let cursor_handle = entry.value().clone();
        drop(entry);
        if cursor_handle.lock().await.connection_id == query.connection_id {
            state.queries.remove(&cursor_id);
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

fn read_only_sql(sql: &str) -> Result<&str> {
    let sql = sql.trim().trim_end_matches(';').trim_end();
    if sql.is_empty() {
        return Err(anyhow!(InvalidRequest(
            "SQL query cannot be empty".to_string()
        )));
    }
    let first_keyword = sql
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !matches!(first_keyword.as_str(), "select" | "with") {
        return Err(anyhow!(InvalidRequest(
            "only read-only SELECT or WITH queries are supported".to_string()
        )));
    }
    Ok(sql)
}

fn serialize_rows(batch: &RecordBatch) -> Result<Vec<Value>> {
    let mut output = Vec::new();
    {
        let mut writer = ArrayWriter::new(&mut output);
        writer.write(batch)?;
        writer.finish()?;
    }
    Ok(serde_json::from_slice(&output)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Int32Array, RecordBatchIterator};
    use arrow_schema::{DataType, Field};
    use lance::Dataset;

    use crate::api::{dataset::build_dataset_info, state::SessionEntry};

    #[test]
    fn accepts_only_read_only_sql() {
        assert_eq!(
            read_only_sql(" SELECT * FROM dataset; ").unwrap(),
            "SELECT * FROM dataset"
        );
        assert!(read_only_sql("WITH selected AS (SELECT 1) SELECT * FROM selected").is_ok());
        assert!(read_only_sql("DELETE FROM dataset").is_err());
        assert!(read_only_sql("CREATE EXTERNAL TABLE secret").is_err());
        assert!(read_only_sql("  ").is_err());
    }
    #[tokio::test]
    async fn sql_cursor_pages_once_and_retries_idempotently() {
        let schema = Arc::new(ArrowSchema::new(vec![Field::new(
            "value",
            DataType::Int32,
            false,
        )]));
        let batch = RecordBatch::try_new(
            schema.clone(),
            vec![Arc::new(Int32Array::from_iter_values(0..205))],
        )
        .unwrap();
        let reader = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let uri = format!("memory://cursor-test-{}", Uuid::new_v4());
        let dataset = Arc::new(Dataset::write(reader, &uri, None).await.unwrap());
        let info = Arc::new(build_dataset_info(&dataset, &uri, "main").await.unwrap());
        let connection = ConnectedDataset { dataset, info };
        let connection_id = Uuid::new_v4();
        let state = AppState::new();
        state
            .connections
            .insert(connection_id, SessionEntry::new(connection.clone()));
        let started = create_sql_cursor(
            &state,
            connection_id,
            &connection,
            "SELECT * FROM dataset ORDER BY value",
        )
        .await
        .unwrap();

        let first = read_sql_page(
            &state,
            started.cursor_id,
            SqlPageQuery {
                connection_id,
                sequence: 0,
            },
        )
        .await
        .unwrap();
        let retry = read_sql_page(
            &state,
            started.cursor_id,
            SqlPageQuery {
                connection_id,
                sequence: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(first, retry);
        assert_eq!(first.rows.len(), SQL_PAGE_ROWS);
        assert!(!first.done);

        let second = read_sql_page(
            &state,
            started.cursor_id,
            SqlPageQuery {
                connection_id,
                sequence: 1,
            },
        )
        .await
        .unwrap();
        let last = read_sql_page(
            &state,
            started.cursor_id,
            SqlPageQuery {
                connection_id,
                sequence: 2,
            },
        )
        .await
        .unwrap();
        assert_eq!(second.rows.len(), SQL_PAGE_ROWS);
        assert_eq!(last.rows.len(), 5);
        assert!(last.done);
        assert!(!last.truncated);
    }
}
