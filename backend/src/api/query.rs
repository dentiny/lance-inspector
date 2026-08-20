use std::{collections::HashMap, sync::Arc, time::Instant};

use anyhow::{Result, anyhow};
use arrow_array::RecordBatch;
use arrow_json::ArrayWriter;
use arrow_schema::Schema as ArrowSchema;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use datafusion_expr::{Expr, LogicalPlan};
use datafusion_sql::sqlparser::{
    ast::{
        Expr as SqlExpr, GroupByExpr, Ident, OrderByKind, SelectItem, SetExpr,
        Statement as SqlStatement, TableFactor,
    },
    dialect::GenericDialect,
    parser::Parser,
};
use futures::{StreamExt, TryStreamExt};
use lance::dataset::sql::SqlQuery;
use lance_core::datatypes::BlobHandling;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::models::{RowView, RowsResponse, SqlCursorResponse, SqlPageResponse};

use super::{
    blob::media_items,
    error::{
        ApiError,
        RequestError::{InvalidRequest, QueryExecutionFailed, UnknownQueryCursor},
    },
    schema::{MediaProjection, dataset_columns, sql_columns},
    state::{AppState, ConnectedDataset, ConnectionQuery, QUERY_IDLE_TTL, QueryCursor, connected},
};

// Maximum number of rows accepted by the direct row-preview endpoint.
const MAX_ROWS: usize = 100;
// Number of rows returned when a row-preview request omits its limit.
const DEFAULT_ROW_LIMIT: usize = 20;
// Number of rows pulled from a SQL cursor per page.
const SQL_PAGE_ROWS: usize = 20;
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
    let connection = connected(&state, query.connection_id)?;
    Ok(Json(read_rows(&connection, query).await?))
}

async fn read_rows(connection: &ConnectedDataset, query: RowsQuery) -> Result<RowsResponse> {
    let limit = query.limit.unwrap_or(DEFAULT_ROW_LIMIT).clamp(1, MAX_ROWS);
    let manifest = connection.dataset.manifest();
    let arrow_schema = ArrowSchema::from(&manifest.schema);
    let source_field_ids = manifest
        .schema
        .fields
        .iter()
        .map(|field| (field.name.clone(), field.id))
        .collect::<HashMap<_, _>>();
    let columns = dataset_columns(&arrow_schema, &source_field_ids);

    let mut scanner = connection.dataset.scan();
    scanner.project(&columns.projection)?;
    scanner.with_row_address();
    scanner.blob_handling(BlobHandling::BlobsDescriptions);
    scanner.limit(Some(limit as i64), Some(query.offset as i64))?;
    let batches = scanner
        .try_into_stream()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut rows = Vec::new();
    for batch in &batches {
        let scalar_indices = columns
            .scalar
            .iter()
            .filter_map(|name| batch.schema().index_of(name).ok())
            .chain(batch.schema().index_of("_rowaddr").ok())
            .collect::<Vec<_>>();
        rows.extend(serialize_rows(batch, &scalar_indices, &columns.media)?);
    }

    Ok(RowsResponse {
        offset: query.offset,
        limit,
        total: connection.info.rows,
        columns: columns.scalar,
        media_columns: columns
            .media
            .iter()
            .map(|projection| projection.column.clone())
            .collect(),
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
    let connection = connected(&state, query.connection_id)?;
    Ok(Json(
        create_sql_cursor(&state, query.connection_id, &connection, &request.sql).await?,
    ))
}

async fn create_sql_cursor(
    state: &AppState,
    connection_id: Uuid,
    connection: &ConnectedDataset,
    sql: &str,
) -> Result<SqlCursorResponse> {
    let sql = read_only_sql(sql)?;
    let prepared = prepare_sql_with_row_addr(sql);
    let (query, media_safe) = match prepared.as_deref() {
        Some(prepared) => match build_sql_query(connection, prepared).await {
            Ok(query) => (query, true),
            Err(_) => (
                build_sql_query(connection, sql)
                    .await
                    .map_err(|error| anyhow!(InvalidRequest(error.to_string())))?,
                false,
            ),
        },
        None => (
            build_sql_query(connection, sql)
                .await
                .map_err(|error| anyhow!(InvalidRequest(error.to_string())))?,
            false,
        ),
    };
    let dataframe = query.into_dataframe();
    let source_names = if media_safe {
        direct_projection_sources(dataframe.logical_plan())
    } else {
        Vec::new()
    };
    let query = SqlQuery::new(dataframe);
    let record_stream = query.into_stream().await?;
    let result_schema = record_stream.schema();
    let manifest = connection.dataset.manifest();
    let dataset_schema = ArrowSchema::from(&manifest.schema);
    let source_field_ids = manifest
        .schema
        .fields
        .iter()
        .map(|field| (field.name.clone(), field.id))
        .collect::<HashMap<_, _>>();
    let columns = sql_columns(
        &result_schema,
        &dataset_schema,
        &source_names,
        &source_field_ids,
    );
    let media_columns = columns
        .media_projections
        .iter()
        .map(|projection| projection.column.clone())
        .collect();
    let cursor_id = Uuid::new_v4();
    state.queries.insert(
        cursor_id,
        Arc::new(AsyncMutex::new(QueryCursor {
            connection_id,
            stream: record_stream,
            scalar_indices: columns.scalar_indices,
            media_projections: columns.media_projections,
            pending_batch: None,
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
        media_columns,
    })
}

async fn build_sql_query(connection: &ConnectedDataset, sql: &str) -> lance_core::Result<SqlQuery> {
    connection
        .dataset
        .sql(sql)
        .with_row_addr(true)
        .blob_handling(BlobHandling::BlobsDescriptions)
        .build()
        .await
}

pub(crate) async fn sql_page(
    State(state): State<Arc<AppState>>,
    Path(cursor_id): Path<Uuid>,
    Query(query): Query<SqlPageQuery>,
) -> Result<Json<SqlPageResponse>, ApiError> {
    connected(&state, query.connection_id)?;
    Ok(Json(read_sql_page(&state, cursor_id, query).await?))
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
    let mut cursor = entry.lock().await;
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
        if let Some(batch) = cursor.pending_batch.take() {
            let take = (page_size - rows.len()).min(batch.num_rows());
            let page_batch = batch.slice(0, take);
            if take < batch.num_rows() {
                cursor.pending_batch = Some(batch.slice(take, batch.num_rows() - take));
            }
            let serialized = serialize_rows(
                &page_batch,
                &cursor.scalar_indices,
                &cursor.media_projections,
            )
            .map_err(|error| QueryExecutionFailed(error.to_string()));
            match serialized {
                Ok(serialized) => rows.extend(serialized),
                Err(error) => {
                    cursor.done = true;
                    drop(cursor);
                    state.queries.remove(&cursor_id);
                    return Err(error.into());
                }
            }
            continue;
        }
        if cursor.done {
            break;
        }
        match cursor.stream.next().await {
            Some(Ok(batch)) => cursor.pending_batch = Some(batch),
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
    let done = capped || (cursor.done && cursor.pending_batch.is_none());
    if capped {
        cursor.done = true;
        cursor.pending_batch = None;
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
    connected(&state, query.connection_id)?;
    if let Some(cursor) = state.queries.get(&cursor_id)
        && cursor.lock().await.connection_id == query.connection_id
    {
        state.queries.remove(&cursor_id);
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

fn prepare_sql_with_row_addr(sql: &str) -> Option<String> {
    let [mut statement] = Parser::parse_sql(&GenericDialect, sql)
        .ok()?
        .try_into()
        .ok()?;
    let SqlStatement::Query(query) = &mut statement else {
        return None;
    };
    if query.with.is_some() {
        return None;
    }
    let order_by_is_safe = query
        .order_by
        .as_ref()
        .is_none_or(|order_by| match &order_by.kind {
            OrderByKind::All(_) => false,
            OrderByKind::Expressions(expressions) => expressions
                .iter()
                .all(|expression| is_direct_column(&expression.expr)),
        });
    let SetExpr::Select(select) = query.body.as_mut() else {
        return None;
    };
    let GroupByExpr::Expressions(group_by, modifiers) = &select.group_by else {
        return None;
    };
    let [from] = select.from.as_slice() else {
        return None;
    };
    let TableFactor::Table {
        name, alias, args, ..
    } = &from.relation
    else {
        return None;
    };
    if !name.to_string().eq_ignore_ascii_case("dataset")
        || args.is_some()
        || alias
            .as_ref()
            .is_some_and(|alias| !alias.columns.is_empty())
        || !from.joins.is_empty()
        || select.distinct.is_some()
        || !order_by_is_safe
        || !group_by.is_empty()
        || !modifiers.is_empty()
        || select.having.is_some()
        || select.qualify.is_some()
        || !select.named_window.is_empty()
    {
        return None;
    }

    let mut has_row_addr = false;
    for item in &select.projection {
        match item {
            SelectItem::Wildcard(options) | SelectItem::QualifiedWildcard(_, options)
                if options.to_string().is_empty() =>
            {
                has_row_addr = true;
            }
            SelectItem::UnnamedExpr(expression) if is_direct_column(expression) => {
                has_row_addr |= is_row_addr(expression);
            }
            SelectItem::ExprWithAlias { expr, alias } if is_direct_column(expr) => {
                if alias.value.eq_ignore_ascii_case("_rowaddr") && !is_row_addr(expr) {
                    return None;
                }
                has_row_addr |= is_row_addr(expr) && alias.value.eq_ignore_ascii_case("_rowaddr");
            }
            _ => return None,
        }
    }
    if !has_row_addr {
        select
            .projection
            .push(SelectItem::UnnamedExpr(SqlExpr::Identifier(Ident::new(
                "_rowaddr",
            ))));
    }
    Some(statement.to_string())
}

fn is_direct_column(expression: &SqlExpr) -> bool {
    matches!(
        expression,
        SqlExpr::Identifier(_) | SqlExpr::CompoundIdentifier(_)
    )
}

fn is_row_addr(expression: &SqlExpr) -> bool {
    match expression {
        SqlExpr::Identifier(identifier) => identifier.value.eq_ignore_ascii_case("_rowaddr"),
        SqlExpr::CompoundIdentifier(identifiers) => identifiers
            .last()
            .is_some_and(|identifier| identifier.value.eq_ignore_ascii_case("_rowaddr")),
        SqlExpr::Nested(expression) => is_row_addr(expression),
        _ => false,
    }
}

fn direct_projection_sources(plan: &LogicalPlan) -> Vec<Option<String>> {
    match plan {
        LogicalPlan::Projection(projection) => {
            projection.expr.iter().map(direct_column_source).collect()
        }
        LogicalPlan::Sort(plan) => direct_projection_sources(&plan.input),
        LogicalPlan::Filter(plan) => direct_projection_sources(&plan.input),
        LogicalPlan::Limit(plan) => direct_projection_sources(&plan.input),
        LogicalPlan::SubqueryAlias(plan) => direct_projection_sources(&plan.input),
        LogicalPlan::Distinct(plan) => direct_projection_sources(plan.input()),
        LogicalPlan::TableScan(scan) => scan
            .projected_schema
            .fields()
            .iter()
            .map(|field| Some(field.name().to_string()))
            .collect(),
        _ => Vec::new(),
    }
}

fn direct_column_source(expression: &Expr) -> Option<String> {
    match expression {
        Expr::Column(column) => Some(column.name.clone()),
        Expr::Alias(alias) => match alias.expr.as_ref() {
            Expr::Column(column) => Some(column.name.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn serialize_rows(
    batch: &RecordBatch,
    scalar_indices: &[usize],
    media_projections: &[MediaProjection],
) -> Result<Vec<RowView>> {
    let scalar_batch = batch.project(scalar_indices)?;
    let mut output = Vec::new();
    {
        let mut writer = ArrayWriter::new(&mut output);
        writer.write(&scalar_batch)?;
        writer.finish()?;
    }
    let values: Vec<Value> = serde_json::from_slice(&output)?;
    values
        .into_iter()
        .enumerate()
        .map(|(row_index, values)| {
            let mut media = std::collections::BTreeMap::new();
            for projection in media_projections {
                let descriptor = batch.column(projection.result_index);
                let items = media_items(descriptor.as_ref(), row_index)?;
                if !items.is_empty() {
                    media.insert(projection.column.name.clone(), items);
                }
            }
            Ok(RowView { values, media })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Int32Array, RecordBatchIterator, UInt64Array};
    use arrow_schema::{DataType, Field};
    use datafusion_expr::{col, lit};
    use lance::{
        BlobArrayBuilder, BlobDescriptorArrayBuilder, Dataset, blob_field,
        dataset::write::WriteParams,
    };
    use lance_file::version::LanceFileVersion;

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

    #[test]
    fn prepares_only_simple_row_address_queries() {
        let prepared = prepare_sql_with_row_addr(
            "SELECT blob AS preview FROM dataset WHERE id > 0 ORDER BY id LIMIT 10",
        )
        .unwrap();
        assert!(prepared.contains("blob AS preview"));
        assert!(prepared.contains("_rowaddr"));
        assert!(prepare_sql_with_row_addr("SELECT * FROM dataset").is_some());
        assert!(prepare_sql_with_row_addr("SELECT blob, _rowaddr FROM dataset").is_some());

        for sql in [
            "SELECT DISTINCT blob FROM dataset",
            "SELECT max(blob) FROM dataset",
            "SELECT blob FROM dataset GROUP BY blob",
            "SELECT left.blob FROM dataset left JOIN dataset right ON true",
            "SELECT blob FROM dataset UNION ALL SELECT blob FROM dataset",
            "WITH selected AS (SELECT blob FROM dataset) SELECT blob FROM selected",
            "SELECT blob FROM (SELECT blob FROM dataset) nested",
            "SELECT (SELECT blob FROM dataset LIMIT 1) AS blob FROM dataset",
            "SELECT blob, 0 AS _rowaddr FROM dataset",
        ] {
            assert!(prepare_sql_with_row_addr(sql).is_none(), "{sql}");
        }
    }

    #[test]
    fn traces_only_direct_columns_and_simple_aliases() {
        assert_eq!(direct_column_source(&col("blob")), Some("blob".to_string()));
        assert_eq!(
            direct_column_source(&col("blob").alias("preview")),
            Some("blob".to_string())
        );
        assert_eq!(direct_column_source(&lit(1).alias("blob")), None);
    }

    #[test]
    fn strips_blob_descriptors_and_emits_media_items() {
        let mut blobs = BlobDescriptorArrayBuilder::new("blob");
        blobs.push_dedicated(7, 42).unwrap();
        blobs.push_null().unwrap();
        let (blob_field, blob_array) = blobs.finish().unwrap().into_parts();
        let schema = Arc::new(ArrowSchema::new(vec![
            Field::new("value", DataType::Int32, false),
            blob_field,
            Field::new("_rowaddr", DataType::UInt64, false),
        ]));
        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int32Array::from(vec![1, 2])),
                blob_array,
                Arc::new(UInt64Array::from(vec![10, 11])),
            ],
        )
        .unwrap();
        let projection = MediaProjection {
            result_index: 1,
            column: crate::models::MediaColumn {
                name: "blob".to_string(),
                source_field_id: 1,
            },
        };

        let rows = serialize_rows(&batch, &[0, 2], std::slice::from_ref(&projection)).unwrap();

        assert_eq!(rows[0].values["value"], 1);
        assert!(rows[0].values.get("blob").is_none());
        assert!(rows[0].media["blob"][0].is_empty());
        assert!(rows[1].media.is_empty());

        let sliced = serialize_rows(
            &batch.slice(1, 1),
            &[0, 2],
            std::slice::from_ref(&projection),
        )
        .unwrap();
        assert_eq!(sliced[0].values["value"], 2);
        assert_eq!(sliced[0].values["_rowaddr"], 11);
        assert!(sliced[0].media.is_empty());
    }

    #[tokio::test]
    async fn normalizes_direct_and_aliased_sql_blob_results() {
        let schema = Arc::new(ArrowSchema::new(vec![blob_field("blob", true)]));
        let mut blobs = BlobArrayBuilder::new(2);
        blobs.push_bytes(b"\x89PNG\r\n\x1a\n").unwrap();
        blobs.push_null().unwrap();
        let batch = RecordBatch::try_new(schema.clone(), vec![blobs.finish().unwrap()]).unwrap();
        let uri = format!("memory://media-query-test-{}", Uuid::new_v4());
        let dataset = Arc::new(
            Dataset::write(
                RecordBatchIterator::new([Ok(batch)], schema),
                &uri,
                Some(WriteParams {
                    data_storage_version: Some(LanceFileVersion::V2_3),
                    ..Default::default()
                }),
            )
            .await
            .unwrap(),
        );
        let info = Arc::new(build_dataset_info(&dataset, &uri, "main").await.unwrap());
        let connection = ConnectedDataset::new(dataset, info);
        let connection_id = Uuid::new_v4();
        let state = AppState::new();
        state
            .connections
            .insert(connection_id, SessionEntry::new(connection.clone()));

        let preview = read_rows(
            &connection,
            RowsQuery {
                connection_id,
                offset: 0,
                limit: Some(20),
            },
        )
        .await
        .unwrap();
        assert!(preview.rows[0].values.get("blob").is_none());
        assert!(preview.rows[0].media["blob"][0].is_empty());

        let started = create_sql_cursor(
            &state,
            connection_id,
            &connection,
            "SELECT blob AS preview FROM dataset",
        )
        .await
        .unwrap();
        assert_eq!(started.media_columns[0].name, "preview");
        assert_eq!(
            started.media_columns[0].source_field_id,
            connection.dataset.schema().field("blob").unwrap().id
        );
        let page = read_sql_page(
            &state,
            started.cursor_id,
            SqlPageQuery {
                connection_id,
                sequence: 0,
            },
        )
        .await
        .unwrap();

        assert!(page.rows[0].values.get("preview").is_none());
        assert!(page.rows[0].values.get("_rowaddr").is_some());
        assert!(page.rows[0].media["preview"][0].is_empty());
        assert!(page.rows[1].media.is_empty());

        let direct = create_sql_cursor(&state, connection_id, &connection, "SELECT * FROM dataset")
            .await
            .unwrap();
        assert_eq!(direct.media_columns[0].name, "blob");
        let direct_page = read_sql_page(
            &state,
            direct.cursor_id,
            SqlPageQuery {
                connection_id,
                sequence: 0,
            },
        )
        .await
        .unwrap();
        assert!(direct_page.rows[0].values.get("blob").is_none());
        assert!(direct_page.rows[0].media["blob"][0].is_empty());
    }

    #[tokio::test]
    async fn reads_checked_in_nested_multimodal_fixture_lazily() {
        let uri = format!(
            "{}/../testdata/nested_multimodal.lance",
            env!("CARGO_MANIFEST_DIR")
        );
        let dataset = Arc::new(Dataset::open(&uri).await.unwrap());
        let info = Arc::new(build_dataset_info(&dataset, &uri, "main").await.unwrap());
        let connection = ConnectedDataset::new(dataset, info);
        let connection_id = Uuid::new_v4();

        let preview = read_rows(
            &connection,
            RowsQuery {
                connection_id,
                offset: 0,
                limit: Some(20),
            },
        )
        .await
        .unwrap();

        assert_eq!(preview.rows.len(), 2);
        assert!(preview.rows[0].values.get("image").is_none());
        assert_eq!(
            preview.rows[0].media["nested_media"],
            vec![vec![0, 0], vec![0, 1], vec![1, 0]]
        );
        assert!(preview.rows[1].media.is_empty());
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
            vec![Arc::new(Int32Array::from_iter_values(
                0..(SQL_PAGE_ROWS * 2 + 5) as i32,
            ))],
        )
        .unwrap();
        let reader = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let uri = format!("memory://cursor-test-{}", Uuid::new_v4());
        let dataset = Arc::new(Dataset::write(reader, &uri, None).await.unwrap());
        let info = Arc::new(build_dataset_info(&dataset, &uri, "main").await.unwrap());
        let connection = ConnectedDataset::new(dataset, info);
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
        let pending_after_first = state
            .queries
            .get(&started.cursor_id)
            .unwrap()
            .lock()
            .await
            .pending_batch
            .as_ref()
            .map(RecordBatch::num_rows);
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
        let pending_after_retry = state
            .queries
            .get(&started.cursor_id)
            .unwrap()
            .lock()
            .await
            .pending_batch
            .as_ref()
            .map(RecordBatch::num_rows);
        assert_eq!(pending_after_first, pending_after_retry);
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
        let values = [first, second, last]
            .into_iter()
            .flat_map(|page| page.rows)
            .map(|row| row.values["value"].as_i64().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(values, (0..45).collect::<Vec<_>>());
    }
}
