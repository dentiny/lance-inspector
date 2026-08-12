use std::{
    collections::{HashSet, VecDeque},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use arrow_array::RecordBatch;
use arrow_json::ArrayWriter;
use arrow_schema::Schema as ArrowSchema;
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{
        HeaderMap, HeaderValue, Response, StatusCode,
        header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE},
    },
    response::IntoResponse,
};
use datafusion_execution::SendableRecordBatchStream;
use foyer::{Cache, CacheBuilder};
use futures::{StreamExt, TryStreamExt};
use lance::{
    Dataset,
    dataset::transaction::{Operation, Transaction},
};
use prost::Message;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::models::{
    BranchHistory, BranchView, ConnectResponse, DataFileView, DatasetInfo, DeletionView, FileEntry,
    FilePreview, FragmentView, HealthResponse, ManifestView, MediaColumn, ReferenceCatalog,
    RowsResponse, SchemaField, SqlCursorResponse, SqlPageResponse, TagView, TransactionView,
    VersionView,
};

const FILE_PREVIEW_BYTES: usize = 64 * 1024;
const MAX_ROWS: usize = 100;
const SQL_PAGE_ROWS: usize = 100;
const MAX_SQL_RESULT_ROWS: usize = 10_000;
const MAX_DELETION_OFFSETS: usize = 2_000;
const MAX_CONNECTIONS: usize = 256;
const MAX_QUERY_CURSORS: usize = 256;
const CONNECTION_IDLE_TTL: Duration = Duration::from_secs(60 * 60);
const QUERY_IDLE_TTL: Duration = Duration::from_secs(10 * 60);
const BLOB_EXTENSION: &str = "lance.blob.v2";

pub struct AppState {
    connections: Cache<Uuid, SessionEntry>,
    queries: Cache<Uuid, Arc<AsyncMutex<QueryCursor>>>,
}

impl AppState {
    pub fn new() -> Self {
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
}

impl SessionEntry {
    fn new(connection: ConnectedDataset) -> Self {
        Self {
            connection,
            last_accessed: Arc::new(Mutex::new(Instant::now())),
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
pub struct ConnectedDataset {
    pub dataset: Arc<Dataset>,
    pub dataset_uri: String,
    pub reference: String,
}

struct QueryCursor {
    connection_id: Uuid,
    stream: SendableRecordBatchStream,
    scalar_indices: Vec<usize>,
    pending_rows: VecDeque<Value>,
    next_sequence: u64,
    rows_returned: usize,
    last_page: Option<SqlPageResponse>,
    last_accessed: Instant,
    done: bool,
}

pub struct ApiError(anyhow::Error);

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

pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

pub async fn dataset_info(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConnectionQuery>,
) -> Result<Json<DatasetInfo>, ApiError> {
    let connection = connected(&state, query.connection_id).map_err(ApiError)?;
    build_dataset_info(&connection)
        .await
        .map(Json)
        .map_err(ApiError)
}

#[derive(Debug, Deserialize)]
pub struct ConnectRequest {
    uri: String,
    reference: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConnectionQuery {
    connection_id: Uuid,
}

#[derive(Debug, Deserialize)]
pub struct DiscoverRequest {
    uri: String,
}

pub async fn discover_dataset(
    Json(request): Json<DiscoverRequest>,
) -> Result<Json<ReferenceCatalog>, ApiError> {
    discover(request).await.map(Json).map_err(ApiError)
}

async fn discover(request: DiscoverRequest) -> Result<ReferenceCatalog> {
    let uri = request.uri.trim();
    if uri.is_empty() {
        bail!("dataset location cannot be empty");
    }
    let root = Dataset::open(uri)
        .await
        .with_context(|| format!("failed to open Lance dataset {uri}"))?;
    let branch_contents = root.list_branches().await?;
    let tag_contents = root.tags().list().await?;

    let mut tags: Vec<_> = tag_contents
        .into_iter()
        .map(|(name, contents)| TagView {
            name,
            branch: contents.branch.unwrap_or_else(|| "main".to_string()),
            version: contents.version,
        })
        .collect();
    tags.sort_by(|left, right| left.name.cmp(&right.name));

    let mut branch_names: Vec<_> = branch_contents.keys().cloned().collect();
    branch_names.sort();
    branch_names.insert(0, "main".to_string());

    let mut branches = Vec::with_capacity(branch_names.len());
    for name in branch_names {
        let dataset = if name == "main" {
            root.clone()
        } else {
            root.checkout_branch(&name).await?
        };
        let versions = dataset
            .versions()
            .await?
            .into_iter()
            .map(|version| VersionView {
                version: version.version,
                timestamp: version.timestamp.to_rfc3339(),
                total_rows: version
                    .metadata
                    .get("total_rows")
                    .and_then(|value| value.parse().ok()),
                tags: tags
                    .iter()
                    .filter(|tag| tag.branch == name && tag.version == version.version)
                    .map(|tag| tag.name.clone())
                    .collect(),
            })
            .collect();
        let metadata = branch_contents.get(&name);
        branches.push(BranchHistory {
            name,
            parent_branch: metadata.and_then(|branch| branch.parent_branch.clone()),
            parent_version: metadata.map(|branch| branch.parent_version),
            versions,
        });
    }

    Ok(ReferenceCatalog {
        uri: uri.to_string(),
        branches,
        tags,
    })
}

pub async fn connect_dataset(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ConnectRequest>,
) -> Result<Json<ConnectResponse>, ApiError> {
    connect(&state, request).await.map(Json).map_err(ApiError)
}

async fn connect(state: &AppState, request: ConnectRequest) -> Result<ConnectResponse> {
    let uri = request.uri.trim();
    if uri.is_empty() {
        bail!("dataset location cannot be empty");
    }
    let reference = request.reference.as_deref().unwrap_or("main").trim();
    let reference = if reference.is_empty() {
        "main"
    } else {
        reference
    };
    let root = Dataset::open(uri)
        .await
        .with_context(|| format!("failed to open Lance dataset {uri}"))?;
    let dataset = Arc::new(resolve_reference(root, reference).await?);
    let connection = ConnectedDataset {
        dataset,
        dataset_uri: uri.to_string(),
        reference: reference.to_string(),
    };
    let dataset = build_dataset_info(&connection).await?;
    let connection_id = Uuid::new_v4();
    state
        .connections
        .insert(connection_id, SessionEntry::new(connection));
    Ok(ConnectResponse {
        connection_id,
        dataset,
    })
}

async fn resolve_reference(root: Dataset, reference: &str) -> Result<Dataset> {
    if reference.eq_ignore_ascii_case("main") {
        return Ok(root);
    }
    if let Ok(version) = reference.parse::<u64>() {
        return root
            .checkout_version(version)
            .await
            .with_context(|| format!("failed to check out version {version}"));
    }
    if let Some(version) = reference.strip_prefix("version:") {
        let version = version
            .parse::<u64>()
            .with_context(|| format!("invalid version reference {reference}"))?;
        return root
            .checkout_version(version)
            .await
            .with_context(|| format!("failed to check out version {version}"));
    }
    if let Some(branch) = reference.strip_prefix("branch:") {
        return root
            .checkout_branch(branch)
            .await
            .with_context(|| format!("failed to check out branch {branch}"));
    }
    if let Some(tag) = reference.strip_prefix("tag:") {
        return root
            .checkout_version(tag)
            .await
            .with_context(|| format!("failed to check out tag {tag}"));
    }
    if let Some((branch, version)) = reference.rsplit_once(':')
        && let Ok(version) = version.parse::<u64>()
    {
        return root
            .checkout_version((branch, version))
            .await
            .with_context(|| format!("failed to check out {branch} version {version}"));
    }

    let branches = root.list_branches().await?;
    if branches.contains_key(reference) {
        return root
            .checkout_branch(reference)
            .await
            .with_context(|| format!("failed to check out branch {reference}"));
    }
    let tags = root.tags().list().await?;
    if tags.contains_key(reference) {
        return root
            .checkout_version(reference)
            .await
            .with_context(|| format!("failed to check out tag {reference}"));
    }
    bail!(
        "reference '{reference}' was not found as a branch or tag; use a numeric version, branch:<name>, or tag:<name>"
    )
}

fn connected(state: &AppState, connection_id: Uuid) -> Result<ConnectedDataset> {
    let entry = state
        .connections
        .get(&connection_id)
        .ok_or(UnknownConnection(connection_id))?;
    if let Some(connection) = entry.value().access() {
        return Ok(connection);
    }
    drop(entry);
    state.connections.remove(&connection_id);
    Err(UnknownConnection(connection_id).into())
}

async fn build_dataset_info(connection: &ConnectedDataset) -> Result<DatasetInfo> {
    let dataset = &connection.dataset;
    let manifest = dataset.manifest();
    let arrow_schema = ArrowSchema::from(&manifest.schema);
    let blob_columns: HashSet<_> = arrow_schema
        .fields()
        .iter()
        .filter(|field| is_blob_field(field))
        .map(|field| field.name().as_str())
        .collect();

    let schema = arrow_schema
        .fields()
        .iter()
        .map(|field| SchemaField {
            name: field.name().clone(),
            data_type: field.data_type().to_string(),
            nullable: field.is_nullable(),
            media: blob_columns.contains(field.name().as_str()),
            metadata: field
                .metadata()
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        })
        .collect();

    let mut fragments = Vec::with_capacity(manifest.fragments.len());
    for file_fragment in dataset.get_fragments() {
        let metadata = file_fragment.metadata();
        let deletion = if let Some(deletion_file) = &metadata.deletion_file {
            let vector = file_fragment.get_deletion_vector().await?;
            let all_offsets: Vec<u32> = vector
                .as_deref()
                .into_iter()
                .flat_map(|value| value.iter())
                .collect();
            let file_type = format!("{:?}", deletion_file.file_type);
            let extension = if file_type.to_ascii_lowercase().contains("array") {
                "arrow"
            } else {
                "bin"
            };
            Some(DeletionView {
                path: format!(
                    "_deletions/{}-{}-{}.{}",
                    metadata.id, deletion_file.read_version, deletion_file.id, extension
                ),
                file_type,
                read_version: deletion_file.read_version,
                id: deletion_file.id,
                count: all_offsets.len(),
                offsets: all_offsets
                    .iter()
                    .copied()
                    .take(MAX_DELETION_OFFSETS)
                    .collect(),
                offsets_truncated: all_offsets.len() > MAX_DELETION_OFFSETS,
            })
        } else {
            None
        };

        fragments.push(FragmentView {
            id: metadata.id,
            physical_rows: metadata.physical_rows,
            visible_rows: metadata.num_rows(),
            files: metadata
                .files
                .iter()
                .map(|file| DataFileView {
                    path: format!("data/{}", file.path),
                    fields: file.fields.to_vec(),
                    column_indices: file.column_indices.to_vec(),
                    format: format!("{}.{}", file.file_major_version, file.file_minor_version),
                    size_bytes: file.file_size_bytes.get().map(u64::from),
                    base_id: file.base_id,
                })
                .collect(),
            deletion,
        });
    }

    let mut branches: Vec<_> = dataset
        .list_branches()
        .await?
        .into_iter()
        .map(|(name, branch)| BranchView {
            name,
            parent_branch: branch.parent_branch,
            parent_version: branch.parent_version,
        })
        .collect();
    branches.sort_by(|left, right| left.name.cmp(&right.name));

    let manifest_location = dataset.manifest_location();
    Ok(DatasetInfo {
        uri: connection.dataset_uri.clone(),
        reference: connection.reference.clone(),
        version: manifest.version,
        branch: manifest
            .branch
            .clone()
            .unwrap_or_else(|| "main".to_string()),
        rows: dataset.count_rows(None).await?,
        schema,
        manifest: ManifestView {
            version: manifest.version,
            path: manifest_location.path.to_string(),
            size: manifest_location.size,
            writer_version: manifest
                .writer_version
                .as_ref()
                .map(|value| format!("{value:?}")),
            timestamp_nanos: manifest.timestamp_nanos.to_string(),
            reader_feature_flags: manifest.reader_feature_flags,
            writer_feature_flags: manifest.writer_feature_flags,
            next_row_id: manifest.next_row_id,
            max_fragment_id: manifest.max_fragment_id,
            transaction_file: manifest.transaction_file.clone(),
            data_storage_format: format!("{:?}", manifest.data_storage_format),
            config: manifest
                .config
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            table_metadata: manifest
                .table_metadata
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            base_paths: manifest
                .base_paths
                .iter()
                .map(|(key, value)| (key.to_string(), format!("{value:?}")))
                .collect(),
        },
        fragments,
        branches,
    })
}

pub async fn files(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConnectionQuery>,
) -> Result<Json<Vec<FileEntry>>, ApiError> {
    let connection = connected(&state, query.connection_id).map_err(ApiError)?;
    list_files(&connection).await.map(Json).map_err(ApiError)
}

async fn list_files(connection: &ConnectedDataset) -> Result<Vec<FileEntry>> {
    let store = connection.dataset.object_store(None).await?;
    let base = connection.dataset.branch_location().path;
    let base_string = base.as_ref().trim_end_matches('/');
    let objects = store
        .read_dir_all(&base, None)
        .try_collect::<Vec<_>>()
        .await?;

    let mut entries: Vec<_> = objects
        .into_iter()
        .filter_map(|object| {
            let full_path = object.location.as_ref();
            let relative = full_path
                .strip_prefix(base_string)
                .unwrap_or(full_path)
                .trim_start_matches('/');
            if relative.is_empty() {
                return None;
            }
            Some(FileEntry {
                path: relative.to_string(),
                size: object.size,
                kind: classify_file(relative),
                modified: object.last_modified.to_string(),
            })
        })
        .collect();
    entries.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(entries)
}

#[derive(Debug, Deserialize)]
pub struct FileQuery {
    connection_id: Uuid,
    path: String,
}

pub async fn file_preview(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FileQuery>,
) -> Result<Json<FilePreview>, ApiError> {
    let connection = connected(&state, query.connection_id).map_err(ApiError)?;
    read_file_preview(&connection, &query.path)
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn read_file_preview(connection: &ConnectedDataset, path: &str) -> Result<FilePreview> {
    validate_relative_path(path)?;
    let store = connection.dataset.object_store(None).await?;
    let full_path = path.split('/').fold(
        connection.dataset.branch_location().path,
        |base, segment| base.join(segment),
    );
    let size = store.size(&full_path).await? as usize;
    let read_size = size.min(FILE_PREVIEW_BYTES);
    let bytes = store.read_one_range(&full_path, 0..read_size).await?;
    let (format, content) = match std::str::from_utf8(&bytes) {
        Ok(text) => ("text", text.to_string()),
        Err(_) => (
            "hex",
            bytes
                .chunks(16)
                .enumerate()
                .map(|(index, chunk)| {
                    format!(
                        "{:08x}  {}",
                        index * 16,
                        chunk
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                })
                .collect::<Vec<_>>()
                .join("\n"),
        ),
    };
    Ok(FilePreview {
        path: path.to_string(),
        size,
        format,
        content,
        truncated: size > read_size,
    })
}

pub async fn transaction(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FileQuery>,
) -> Result<Json<TransactionView>, ApiError> {
    let connection = connected(&state, query.connection_id).map_err(ApiError)?;
    read_transaction(&connection, &query.path)
        .await
        .map(Json)
        .map_err(ApiError)
}

async fn read_transaction(connection: &ConnectedDataset, path: &str) -> Result<TransactionView> {
    validate_relative_path(path)?;
    if !path.ends_with(".txn") {
        bail!("transaction path must end in .txn");
    }
    let store = connection.dataset.object_store(None).await?;
    let full_path = path.split('/').fold(
        connection.dataset.branch_location().path,
        |base, segment| base.join(segment),
    );
    let bytes = store.read_one_all(&full_path).await?;
    let protobuf = lance::table::format::pb::Transaction::decode(bytes)?;
    let transaction = Transaction::try_from(protobuf)?;
    let operation_type = transaction.operation.to_string();
    let operation = operation_json(&transaction.operation);

    Ok(TransactionView {
        path: path.to_string(),
        read_version: transaction.read_version,
        uuid: transaction.uuid,
        tag: transaction.tag,
        properties: transaction
            .transaction_properties
            .as_deref()
            .into_iter()
            .flat_map(|properties| properties.iter())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        operation_type,
        operation,
    })
}

fn operation_json(operation: &Operation) -> Value {
    match operation {
        Operation::Append { fragments } => json!({
            "fragment_count": fragments.len(),
            "fragments": fragments.iter().map(fragment_json).collect::<Vec<_>>(),
        }),
        Operation::Delete {
            updated_fragments,
            deleted_fragment_ids,
            predicate,
        } => json!({
            "predicate": predicate,
            "updated_fragments": updated_fragments.iter().map(fragment_json).collect::<Vec<_>>(),
            "deleted_fragment_ids": deleted_fragment_ids,
        }),
        Operation::Overwrite {
            fragments,
            schema,
            config_upsert_values,
            initial_bases,
        } => json!({
            "fragment_count": fragments.len(),
            "fragments": fragments.iter().map(fragment_json).collect::<Vec<_>>(),
            "schema_fields": schema.fields.iter().map(|field| field.name.clone()).collect::<Vec<_>>(),
            "config_upsert_values": config_upsert_values,
            "initial_bases": initial_bases.as_ref().map(|bases| format!("{bases:#?}")),
        }),
        Operation::Clone {
            is_shallow,
            ref_name,
            ref_version,
            ref_path,
            branch_name,
        } => json!({
            "is_shallow": is_shallow,
            "source_reference": ref_name,
            "source_version": ref_version,
            "source_path": ref_path,
            "branch_name": branch_name,
        }),
        Operation::Restore { version } => json!({ "restored_version": version }),
        Operation::ReserveFragments { num_fragments } => {
            json!({ "reserved_fragment_count": num_fragments })
        }
        other => json!({
            "summary": other.to_string(),
            "details": format!("{other:#?}"),
        }),
    }
}

fn fragment_json(fragment: &lance::table::format::Fragment) -> Value {
    json!({
        "id": fragment.id,
        "physical_rows": fragment.physical_rows,
        "visible_rows": fragment.num_rows(),
        "data_files": fragment.files.iter().map(|file| file.path.clone()).collect::<Vec<_>>(),
        "deletion_count": fragment.deletion_file.as_ref().and_then(|file| file.num_deleted_rows),
    })
}

#[derive(Debug, Deserialize)]
pub struct RowsQuery {
    connection_id: Uuid,
    #[serde(default)]
    offset: usize,
    #[serde(default = "default_row_limit")]
    limit: usize,
}

fn default_row_limit() -> usize {
    20
}

pub async fn rows(
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
    let limit = query.limit.clamp(1, MAX_ROWS);
    let arrow_schema = ArrowSchema::from(&connection.dataset.manifest().schema);
    let media_columns: Vec<_> = arrow_schema
        .fields()
        .iter()
        .filter(|field| is_blob_field(field))
        .map(|field| {
            let mime_name = format!("{}_mime", field.name());
            MediaColumn {
                name: field.name().clone(),
                mime_column: arrow_schema
                    .field_with_name(&mime_name)
                    .ok()
                    .map(|_| mime_name),
            }
        })
        .collect();
    let scalar_columns: Vec<String> = arrow_schema
        .fields()
        .iter()
        .filter(|field| !is_blob_field(field))
        .map(|field| field.name().clone())
        .collect();

    let mut scanner = connection.dataset.scan();
    scanner.project(&scalar_columns)?;
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
        total: connection.dataset.count_rows(None).await?,
        columns: scalar_columns,
        media_columns,
        rows,
    })
}

#[derive(Debug, Deserialize)]
pub struct SqlRequest {
    sql: String,
}

#[derive(Debug, Deserialize)]
pub struct SqlPageQuery {
    connection_id: Uuid,
    sequence: u64,
}

pub async fn start_sql(
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
    let scalar_indices: Vec<_> = result_schema
        .fields()
        .iter()
        .enumerate()
        .filter_map(|(index, field)| (!is_sql_blob_field(field, &dataset_schema)).then_some(index))
        .collect();
    let columns: Vec<_> = scalar_indices
        .iter()
        .map(|index| result_schema.field(*index).name().clone())
        .collect();
    let media_columns: Vec<_> = result_schema
        .fields()
        .iter()
        .filter(|field| is_sql_blob_field(field, &dataset_schema))
        .map(|field| {
            let mime_name = format!("{}_mime", field.name());
            MediaColumn {
                name: field.name().clone(),
                mime_column: result_schema
                    .field_with_name(&mime_name)
                    .ok()
                    .map(|_| mime_name),
            }
        })
        .collect();
    let cursor_id = Uuid::new_v4();
    state.queries.insert(
        cursor_id,
        Arc::new(AsyncMutex::new(QueryCursor {
            connection_id,
            stream: record_stream,
            scalar_indices,
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
        columns,
        media_columns,
    })
}

pub async fn sql_page(
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

pub async fn cancel_sql(
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

#[derive(Debug, Deserialize)]
pub struct MediaQuery {
    connection_id: Uuid,
    mime: Option<String>,
}

pub async fn media(
    State(state): State<Arc<AppState>>,
    Path((column, row_address)): Path<(String, u64)>,
    Query(query): Query<MediaQuery>,
    headers: HeaderMap,
) -> Result<Response<Body>, ApiError> {
    let connection = connected(&state, query.connection_id).map_err(ApiError)?;
    read_media(&connection, &column, row_address, query, &headers)
        .await
        .map_err(ApiError)
}

async fn read_media(
    connection: &ConnectedDataset,
    column: &str,
    row_address: u64,
    query: MediaQuery,
    headers: &HeaderMap,
) -> Result<Response<Body>> {
    let arrow_schema = ArrowSchema::from(&connection.dataset.manifest().schema);
    let field = arrow_schema
        .field_with_name(column)
        .with_context(|| format!("unknown column {column}"))?;
    if !is_blob_field(field) {
        bail!("column {column} is not a Lance blob column");
    }

    let mut blobs = connection
        .dataset
        .take_blobs_by_addresses(&[row_address], column)
        .await?;
    let blob = blobs.pop().ok_or_else(|| anyhow!("blob row not found"))?;
    let size = blob.size();
    let (start, end, partial) = parse_range(headers.get(RANGE), size)?;
    let bytes = blob.read_range(start..end).await?;
    let mime = query
        .mime
        .filter(|value| {
            value.starts_with("image/")
                || value.starts_with("audio/")
                || value.starts_with("video/")
                || value == "application/octet-stream"
        })
        .unwrap_or_else(|| "application/octet-stream".to_string());

    let mut response = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(CONTENT_TYPE, HeaderValue::from_str(&mime)?)
        .header(ACCEPT_RANGES, "bytes")
        .header(CONTENT_LENGTH, bytes.len().to_string());
    if partial {
        response = response.header(CONTENT_RANGE, format!("bytes {start}-{}/{}", end - 1, size));
    }
    Ok(response.body(Body::from(bytes))?)
}

fn parse_range(header: Option<&HeaderValue>, size: u64) -> Result<(u64, u64, bool)> {
    let Some(header) = header else {
        return Ok((0, size, false));
    };
    let invalid = |message: &str| {
        anyhow!(RangeNotSatisfiable {
            size,
            message: message.to_string(),
        })
    };
    let value = header
        .to_str()
        .map_err(|_| invalid("range header is not valid ASCII"))?;
    let range = value
        .strip_prefix("bytes=")
        .ok_or_else(|| invalid("unsupported range header"))?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| invalid("invalid range header"))?;

    if start.is_empty() {
        let suffix_length = end
            .parse::<u64>()
            .map_err(|_| invalid("invalid suffix range"))?;
        if suffix_length == 0 || size == 0 {
            return Err(invalid("requested range is outside the blob"));
        }
        return Ok((size.saturating_sub(suffix_length), size, true));
    }

    let start = start
        .parse::<u64>()
        .map_err(|_| invalid("invalid range start"))?;
    if start >= size {
        return Err(invalid("requested range is outside the blob"));
    }
    let end = if end.is_empty() {
        size
    } else {
        let inclusive_end = end
            .parse::<u64>()
            .map_err(|_| invalid("invalid range end"))?;
        if inclusive_end < start {
            return Err(invalid("range end precedes range start"));
        }
        inclusive_end.saturating_add(1).min(size)
    };
    Ok((start, end, true))
}

fn is_blob_field(field: &arrow_schema::Field) -> bool {
    field
        .metadata()
        .get("ARROW:extension:name")
        .is_some_and(|name| name == BLOB_EXTENSION)
}

fn is_sql_blob_field(field: &arrow_schema::Field, dataset_schema: &ArrowSchema) -> bool {
    is_blob_field(field)
        || dataset_schema
            .field_with_name(field.name())
            .is_ok_and(is_blob_field)
}

fn classify_file(path: &str) -> &'static str {
    let segments: HashSet<_> = path.split('/').collect();
    if path.ends_with(".manifest") {
        "manifest"
    } else if path.ends_with(".lance") {
        "data"
    } else if segments.contains("_deletions") {
        "deletion"
    } else if segments.contains("_indices") {
        "index"
    } else if segments.contains("_transactions") {
        "transaction"
    } else {
        "file"
    }
}

fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty()
        || path.starts_with('/')
        || path.split('/').any(|part| part == ".." || part.is_empty())
    {
        bail!("invalid dataset-relative path");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{Int32Array, RecordBatchIterator};
    use arrow_schema::{DataType, Field};

    #[test]
    fn classifies_lance_internal_files() {
        assert_eq!(classify_file("_versions/2.manifest"), "manifest");
        assert_eq!(classify_file("data/part.lance"), "data");
        assert_eq!(classify_file("_deletions/0-1-2.arrow"), "deletion");
        assert_eq!(classify_file("_indices/index/file"), "index");
    }

    #[test]
    fn rejects_paths_outside_dataset() {
        assert!(validate_relative_path("data/part.lance").is_ok());
        assert!(validate_relative_path("../secret").is_err());
        assert!(validate_relative_path("/etc/passwd").is_err());
        assert!(validate_relative_path("data//part.lance").is_err());
    }

    #[test]
    fn parses_http_byte_ranges() {
        let header = HeaderValue::from_static("bytes=10-19");
        assert_eq!(parse_range(Some(&header), 100).unwrap(), (10, 20, true));
        let open = HeaderValue::from_static("bytes=90-");
        assert_eq!(parse_range(Some(&open), 100).unwrap(), (90, 100, true));
        let suffix = HeaderValue::from_static("bytes=-20");
        assert_eq!(parse_range(Some(&suffix), 100).unwrap(), (80, 100, true));
        let oversized_suffix = HeaderValue::from_static("bytes=-500");
        assert_eq!(
            parse_range(Some(&oversized_suffix), 100).unwrap(),
            (0, 100, true)
        );
        assert_eq!(parse_range(None, 100).unwrap(), (0, 100, false));
    }

    #[test]
    fn rejects_invalid_http_byte_ranges_with_416() {
        for value in ["items=0-1", "bytes=100-", "bytes=20-10", "bytes=-0"] {
            let header = HeaderValue::from_str(value).unwrap();
            let error = parse_range(Some(&header), 100).unwrap_err();
            let response = ApiError(error).into_response();
            assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
            assert_eq!(response.headers()[CONTENT_RANGE], "bytes */100");
            assert_eq!(response.headers()[ACCEPT_RANGES], "bytes");
        }
    }

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
        let connection = ConnectedDataset {
            dataset,
            dataset_uri: uri,
            reference: "main".to_string(),
        };
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
