use std::{collections::BTreeMap, sync::Arc};

use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
}

#[derive(Debug, Serialize)]
pub struct DatasetInfo {
    pub uri: String,
    pub reference: String,
    pub version: u64,
    pub branch: String,
    pub rows: usize,
    pub schema: Vec<SchemaField>,
    pub manifest: ManifestView,
    pub fragments: Vec<FragmentView>,
}

#[derive(Debug, Serialize)]
pub struct ConnectResponse {
    pub connection_id: Uuid,
    pub dataset: Arc<DatasetInfo>,
}

#[derive(Debug, Serialize)]
pub struct SchemaField {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub media: bool,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct ManifestView {
    pub version: u64,
    pub path: String,
    pub size: Option<u64>,
    pub writer_version: Option<String>,
    pub timestamp_nanos: String,
    pub reader_feature_flags: u64,
    pub writer_feature_flags: u64,
    pub next_row_id: u64,
    pub max_fragment_id: Option<u32>,
    pub transaction_file: Option<String>,
    pub data_storage_format: String,
    pub config: BTreeMap<String, String>,
    pub table_metadata: BTreeMap<String, String>,
    pub base_paths: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub struct FragmentView {
    pub id: u64,
    pub physical_rows: Option<usize>,
    pub visible_rows: Option<usize>,
    pub files: Vec<DataFileView>,
    pub deletion: Option<DeletionView>,
}

#[derive(Debug, Serialize)]
pub struct DataFileView {
    pub path: String,
    pub fields: Vec<i32>,
    pub column_indices: Vec<i32>,
    pub format: String,
    pub size_bytes: Option<u64>,
    pub base_id: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct DeletionView {
    pub path: String,
    pub file_type: String,
    pub read_version: u64,
    pub id: u64,
    pub count: usize,
    pub offsets: Vec<u32>,
    pub offsets_truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct ReferenceCatalog {
    pub uri: String,
    pub branches: Vec<BranchHistory>,
    pub tags: Vec<TagView>,
}

#[derive(Debug, Serialize)]
pub struct BranchHistory {
    pub name: String,
    pub parent_branch: Option<String>,
    pub parent_version: Option<u64>,
    pub versions: Vec<VersionView>,
}

#[derive(Debug, Serialize)]
pub struct VersionView {
    pub version: u64,
    pub timestamp: String,
    pub total_rows: Option<u64>,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TagView {
    pub name: String,
    pub branch: String,
    pub version: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct FileEntry {
    pub path: String,
    pub size: u64,
    pub kind: &'static str,
    pub modified: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct FilesPage {
    pub entries: Vec<FileEntry>,
    pub next_offset: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct FilePreview {
    pub path: String,
    pub size: usize,
    pub format: &'static str,
    pub content: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
pub struct TransactionView {
    pub path: String,
    pub read_version: u64,
    pub uuid: String,
    pub tag: Option<String>,
    pub properties: BTreeMap<String, String>,
    pub operation_type: String,
    pub operation: Value,
}

#[derive(Debug, Serialize)]
pub struct RowsResponse {
    pub offset: usize,
    pub limit: usize,
    pub total: usize,
    pub columns: Vec<String>,
    pub media_columns: Vec<MediaColumn>,
    pub rows: Vec<Value>,
}

#[derive(Debug, Serialize)]
pub struct MediaColumn {
    pub name: String,
    pub mime_column: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SqlCursorResponse {
    pub cursor_id: Uuid,
    pub columns: Vec<String>,
    pub media_columns: Vec<MediaColumn>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SqlPageResponse {
    pub sequence: u64,
    pub rows: Vec<Value>,
    pub done: bool,
    pub truncated: bool,
}
