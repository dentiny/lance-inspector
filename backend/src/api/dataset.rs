use std::{collections::HashSet, sync::Arc};

use anyhow::{Context, Result, bail};
use arrow_schema::Schema as ArrowSchema;
use axum::{
    Json,
    extract::{Query, State},
};
use lance::{Dataset, index::DatasetIndexExt};
use serde::Deserialize;
use uuid::Uuid;

use crate::models::{
    BranchHistory, ConnectResponse, DataFileView, DatasetInfo, DeletionView, FragmentView,
    IndexView, ManifestView, ReferenceCatalog, SchemaField, TagView, VersionView,
};

use super::{
    error::ApiError,
    schema::is_blob_field,
    state::{AppState, ConnectionQuery, DiscoveryEntry, SessionEntry, connected, discovered},
};

// Maximum number of deleted row offsets included in the UI preview.
const MAX_DELETION_OFFSETS: usize = 2_000;

pub(crate) async fn dataset_info(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ConnectionQuery>,
) -> Result<Json<Arc<DatasetInfo>>, ApiError> {
    let connection = connected(&state, query.connection_id)?;
    Ok(Json(connection.info))
}

#[derive(Debug, Deserialize)]
pub(crate) struct ConnectRequest {
    discovery_id: Uuid,
    reference: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DiscoverRequest {
    uri: String,
}

pub(crate) async fn discover_dataset(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DiscoverRequest>,
) -> Result<Json<ReferenceCatalog>, ApiError> {
    Ok(Json(discover(&state, request).await?))
}

async fn discover(state: &AppState, request: DiscoverRequest) -> Result<ReferenceCatalog> {
    let uri = request.uri.trim();
    if uri.is_empty() {
        bail!("dataset location cannot be empty");
    }
    let root = Arc::new(
        Dataset::open(uri)
            .await
            .with_context(|| format!("failed to open Lance dataset {uri}"))?,
    );
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
            root.as_ref().clone()
        } else {
            root.checkout_branch(&name).await?
        };
        let loaded_version = dataset.version();
        let versions = dataset
            .version_refs()
            .await?
            .into_iter()
            .map(|version| VersionView {
                version: version.version,
                total_rows: (version.version == loaded_version.version)
                    .then(|| {
                        loaded_version
                            .metadata
                            .get("total_rows")
                            .and_then(|value| value.parse().ok())
                    })
                    .flatten(),
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

    let discovery_id = Uuid::new_v4();
    state
        .discoveries
        .insert(discovery_id, DiscoveryEntry::new((root, uri.to_string())));
    Ok(ReferenceCatalog {
        discovery_id,
        uri: uri.to_string(),
        branches,
        tags,
    })
}

pub(crate) async fn connect_dataset(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ConnectRequest>,
) -> Result<Json<ConnectResponse>, ApiError> {
    Ok(Json(connect(&state, request).await?))
}

async fn connect(state: &AppState, request: ConnectRequest) -> Result<ConnectResponse> {
    let (root, uri) = discovered(state, request.discovery_id)?;
    let reference = request.reference.as_deref().unwrap_or("main").trim();
    let reference = if reference.is_empty() {
        "main"
    } else {
        reference
    };
    let dataset = resolve_reference(root, reference).await?;
    let dataset_info = Arc::new(build_dataset_info(&dataset, &uri, reference).await?);
    let connection = state.connected_dataset(dataset, dataset_info.clone());
    let connection_id = Uuid::new_v4();
    state
        .connections
        .insert(connection_id, SessionEntry::new(connection));
    Ok(ConnectResponse {
        connection_id,
        dataset: dataset_info,
    })
}

#[derive(Clone, Copy)]
enum DatasetReference<'a> {
    Main,
    Version(u64),
    Branch(&'a str),
    Tag(&'a str),
    BranchVersion(&'a str, u64),
}

impl std::fmt::Display for DatasetReference<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Main => formatter.write_str("main"),
            Self::Version(version) => write!(formatter, "version {version}"),
            Self::Branch(branch) => write!(formatter, "branch {branch}"),
            Self::Tag(tag) => write!(formatter, "tag {tag}"),
            Self::BranchVersion(branch, version) => write!(formatter, "{branch} version {version}"),
        }
    }
}

async fn resolve_reference(root: Arc<Dataset>, reference: &str) -> Result<Arc<Dataset>> {
    let target = if reference.eq_ignore_ascii_case("main") {
        DatasetReference::Main
    } else if let Ok(version) = reference.parse() {
        DatasetReference::Version(version)
    } else if let Some(version) = reference.strip_prefix("version:") {
        DatasetReference::Version(
            version
                .parse()
                .with_context(|| format!("invalid version reference {reference}"))?,
        )
    } else if let Some(branch) = reference.strip_prefix("branch:") {
        DatasetReference::Branch(branch)
    } else if let Some(tag) = reference.strip_prefix("tag:") {
        DatasetReference::Tag(tag)
    } else if let Some((branch, version)) = reference.rsplit_once(':')
        && let Ok(version) = version.parse()
    {
        DatasetReference::BranchVersion(branch, version)
    } else if root.list_branches().await?.contains_key(reference) {
        DatasetReference::Branch(reference)
    } else if root.tags().list().await?.contains_key(reference) {
        DatasetReference::Tag(reference)
    } else {
        bail!(
            "reference '{reference}' was not found as a branch or tag; use a numeric version, branch:<name>, or tag:<name>"
        )
    };

    if matches!(target, DatasetReference::Main) {
        return Ok(root);
    }
    let dataset = match target {
        DatasetReference::Main => unreachable!(),
        DatasetReference::Version(version) => root.checkout_version(version).await,
        DatasetReference::Branch(branch) => root.checkout_branch(branch).await,
        DatasetReference::Tag(tag) => root.checkout_version(tag).await,
        DatasetReference::BranchVersion(branch, version) => {
            root.checkout_version((branch, version)).await
        }
    }
    .with_context(|| format!("failed to check out {target}"))?;
    Ok(Arc::new(dataset))
}
pub(super) async fn build_dataset_info(
    dataset: &Dataset,
    dataset_uri: &str,
    reference: &str,
) -> Result<DatasetInfo> {
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
    let indices = dataset
        .describe_indices(None)
        .await?
        .into_iter()
        .map(|index| IndexView {
            name: index.name().to_string(),
            index_type: index.index_type().to_string(),
            type_url: index.type_url().to_string(),
            fields: index
                .field_ids()
                .iter()
                .map(|field_id| {
                    manifest
                        .schema
                        .field_by_id(*field_id as i32)
                        .map(|field| field.name.clone())
                        .unwrap_or_else(|| format!("#{field_id}"))
                })
                .collect(),
            rows_indexed: index.rows_indexed(),
            segment_count: index.segments().len(),
            total_size_bytes: index.total_size_bytes(),
            details: index
                .details()
                .ok()
                .and_then(|details| serde_json::from_str(&details).ok()),
        })
        .collect();

    let mut fragments = Vec::with_capacity(manifest.fragments.len());
    let mut rows = 0usize;
    for file_fragment in dataset.get_fragments() {
        let physical_rows = file_fragment.physical_rows().await?;
        let metadata = file_fragment.metadata();
        let (deletion, deleted_rows) = if let Some(deletion_file) = &metadata.deletion_file {
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
            (
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
                }),
                count,
            )
        } else {
            (None, 0)
        };
        let visible_rows = physical_rows.checked_sub(deleted_rows).with_context(|| {
            format!(
                "fragment {} reports {deleted_rows} deleted rows but only {physical_rows} physical rows",
                metadata.id
            )
        })?;
        rows += visible_rows;

        fragments.push(FragmentView {
            id: metadata.id,
            physical_rows: Some(physical_rows),
            visible_rows: Some(visible_rows),
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

    let manifest_location = dataset.manifest_location();
    Ok(DatasetInfo {
        uri: dataset_uri.to_string(),
        reference: reference.to_string(),
        version: manifest.version,
        branch: manifest
            .branch
            .clone()
            .unwrap_or_else(|| "main".to_string()),
        rows,
        schema,
        indices,
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
    })
}

#[cfg(test)]
mod tests {
    use arrow_array::{Int32Array, RecordBatch, RecordBatchIterator};
    use arrow_schema::{DataType, Field, Schema as TestArrowSchema};

    use super::*;
    use crate::api::state::{AppState, connected, discovered};

    #[tokio::test]
    async fn connect_reuses_the_discovered_dataset_root() {
        let schema = Arc::new(TestArrowSchema::new(vec![Field::new(
            "value",
            DataType::Int32,
            false,
        )]));
        let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(Int32Array::from(vec![1]))])
            .unwrap();
        let reader = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let path =
            std::env::temp_dir().join(format!("lance-inspector-discovery-{}", Uuid::new_v4()));
        let uri = path.to_string_lossy().into_owned();
        Dataset::write(reader, &uri, None).await.unwrap();

        let state = AppState::new();
        let catalog = discover(&state, DiscoverRequest { uri }).await.unwrap();
        let (discovered_root, _) = discovered(&state, catalog.discovery_id).unwrap();

        let response = connect(
            &state,
            ConnectRequest {
                discovery_id: catalog.discovery_id,
                reference: Some("main".to_string()),
            },
        )
        .await
        .unwrap();
        let connection = connected(&state, response.connection_id).unwrap();

        assert!(Arc::ptr_eq(&discovered_root, &connection.dataset));
        std::fs::remove_dir_all(path).unwrap();
    }
}
