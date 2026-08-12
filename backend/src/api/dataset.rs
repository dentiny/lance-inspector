use std::{collections::HashSet, sync::Arc};

use anyhow::{Context, Result, bail};
use arrow_schema::Schema as ArrowSchema;
use axum::{
    Json,
    extract::{Query, State},
};
use lance::Dataset;
use serde::Deserialize;
use uuid::Uuid;

use crate::models::{
    BranchHistory, BranchView, ConnectResponse, DataFileView, DatasetInfo, DeletionView,
    FragmentView, ManifestView, ReferenceCatalog, SchemaField, TagView, VersionView,
};

use super::{
    ApiError, AppState, ConnectedDataset, ConnectionQuery, SessionEntry, connected, is_blob_field,
};

const MAX_DELETION_OFFSETS: usize = 2_000;

pub(crate) async fn dataset_info(
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
pub(crate) struct ConnectRequest {
    uri: String,
    reference: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiscoverRequest {
    uri: String,
}

pub(crate) async fn discover_dataset(
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

pub(crate) async fn connect_dataset(
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
            let (count, offsets) = if let Some(vector) = vector.as_deref() {
                let count = vector.len();
                let mut offsets = Vec::new();
                if count > 0 {
                    offsets.reserve(count.min(MAX_DELETION_OFFSETS));
                    offsets.extend(vector.iter().take(MAX_DELETION_OFFSETS));
                }
                (count, offsets)
            } else {
                (0, Vec::new())
            };
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
                count,
                offsets,
                offsets_truncated: count > MAX_DELETION_OFFSETS,
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
