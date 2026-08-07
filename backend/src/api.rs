use std::{collections::HashSet, sync::Arc};

use anyhow::{Context, Result, anyhow, bail};
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
use futures::TryStreamExt;
use lance::{
    Dataset,
    dataset::transaction::{Operation, Transaction},
};
use prost::Message;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::RwLock;

use crate::models::{
    BranchView, DataFileView, DatasetInfo, DeletionView, FileEntry, FilePreview, FragmentView,
    HealthResponse, ManifestView, MediaColumn, RowsResponse, SchemaField, TransactionView,
};

const FILE_PREVIEW_BYTES: usize = 64 * 1024;
const MAX_ROWS: usize = 100;
const MAX_DELETION_OFFSETS: usize = 2_000;
const BLOB_EXTENSION: &str = "lance.blob.v2";

pub struct AppState {
    pub connection: RwLock<Option<ConnectedDataset>>,
}

#[derive(Clone)]
pub struct ConnectedDataset {
    pub dataset: Arc<Dataset>,
    pub dataset_uri: String,
}

pub struct ApiError(anyhow::Error);

#[derive(Debug)]
struct NoDataset;

impl std::fmt::Display for NoDataset {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("no dataset is connected")
    }
}

impl std::error::Error for NoDataset {}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let status = if self.0.downcast_ref::<NoDataset>().is_some() {
            StatusCode::CONFLICT
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
) -> Result<Json<DatasetInfo>, ApiError> {
    let connection = connected(&state).await.map_err(ApiError)?;
    build_dataset_info(&connection)
        .await
        .map(Json)
        .map_err(ApiError)
}

#[derive(Debug, Deserialize)]
pub struct ConnectRequest {
    uri: String,
}

pub async fn connect_dataset(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ConnectRequest>,
) -> Result<Json<DatasetInfo>, ApiError> {
    connect(&state, request).await.map(Json).map_err(ApiError)
}

async fn connect(state: &AppState, request: ConnectRequest) -> Result<DatasetInfo> {
    let uri = request.uri.trim();
    if uri.is_empty() {
        bail!("dataset location cannot be empty");
    }
    let dataset = Arc::new(
        Dataset::open(uri)
            .await
            .with_context(|| format!("failed to open Lance dataset {uri}"))?,
    );
    let connection = ConnectedDataset {
        dataset,
        dataset_uri: uri.to_string(),
    };
    *state.connection.write().await = Some(connection.clone());
    build_dataset_info(&connection).await
}

async fn connected(state: &AppState) -> Result<ConnectedDataset> {
    state
        .connection
        .read()
        .await
        .clone()
        .ok_or_else(|| NoDataset.into())
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

pub async fn files(State(state): State<Arc<AppState>>) -> Result<Json<Vec<FileEntry>>, ApiError> {
    let connection = connected(&state).await.map_err(ApiError)?;
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
    path: String,
}

pub async fn file_preview(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FileQuery>,
) -> Result<Json<FilePreview>, ApiError> {
    let connection = connected(&state).await.map_err(ApiError)?;
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
    let connection = connected(&state).await.map_err(ApiError)?;
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
    let connection = connected(&state).await.map_err(ApiError)?;
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
pub struct MediaQuery {
    mime: Option<String>,
}

pub async fn media(
    State(state): State<Arc<AppState>>,
    Path((column, row_address)): Path<(String, u64)>,
    Query(query): Query<MediaQuery>,
    headers: HeaderMap,
) -> Result<Response<Body>, ApiError> {
    let connection = connected(&state).await.map_err(ApiError)?;
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
    let value = header.to_str()?;
    let range = value
        .strip_prefix("bytes=")
        .ok_or_else(|| anyhow!("unsupported range header"))?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| anyhow!("invalid range header"))?;
    let start: u64 = start.parse()?;
    let end = if end.is_empty() {
        size
    } else {
        end.parse::<u64>()?.saturating_add(1).min(size)
    };
    if start >= end || start >= size {
        bail!("requested range is outside the blob");
    }
    Ok((start, end, true))
}

fn is_blob_field(field: &arrow_schema::Field) -> bool {
    field
        .metadata()
        .get("ARROW:extension:name")
        .is_some_and(|name| name == BLOB_EXTENSION)
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
        assert_eq!(parse_range(None, 100).unwrap(), (0, 100, false));
    }
}
