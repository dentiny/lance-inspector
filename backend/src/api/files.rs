use std::{collections::HashSet, sync::Arc};

use anyhow::{Result, bail};
use axum::{
    Json,
    extract::{Query, State},
};
use futures::StreamExt;
use lance::dataset::transaction::{Operation, Transaction};
use prost::Message;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::models::{FileEntry, FilePreview, FilesPage, TransactionView};

use super::{ApiError, AppState, ConnectedDataset, InvalidRequest, connected, connected_session};

const FILE_PREVIEW_BYTES: usize = 64 * 1024;
const DEFAULT_FILES_PAGE_SIZE: usize = 500;
const MAX_FILES_PAGE_SIZE: usize = 1_000;

pub(super) struct FileListing {
    receiver: mpsc::Receiver<Result<FileEntry, String>>,
    pending: Option<FileEntry>,
    next_offset: usize,
    last_offset: Option<usize>,
    last_page: Option<FilesPage>,
}

pub(crate) async fn files(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FilesQuery>,
) -> Result<Json<FilesPage>, ApiError> {
    files_page(state, query).await.map(Json).map_err(ApiError)
}

#[derive(Debug, Deserialize)]
pub(crate) struct FilesQuery {
    connection_id: Uuid,
    #[serde(default)]
    offset: usize,
    limit: Option<usize>,
}

async fn files_page(state: Arc<AppState>, query: FilesQuery) -> Result<FilesPage> {
    let (session, connection) = connected_session(&state, query.connection_id)?;
    let limit = query
        .limit
        .unwrap_or(DEFAULT_FILES_PAGE_SIZE)
        .clamp(1, MAX_FILES_PAGE_SIZE);
    let mut guard = session.file_listing.lock().await;
    if guard.is_none() {
        if query.offset != 0 {
            bail!(InvalidRequest(
                "the first file page must start at offset 0".to_string()
            ));
        }
        *guard = Some(start_file_listing(&connection).await?);
    }

    let result = read_file_page(
        guard.as_mut().expect("file listing initialized"),
        query.offset,
        limit,
    )
    .await;
    if result.is_err() {
        *guard = None;
    }
    result
}

async fn start_file_listing(connection: &ConnectedDataset) -> Result<FileListing> {
    let store = connection.dataset.object_store(None).await?;
    let base = connection.dataset.branch_location().path;
    let base_string = base.as_ref().trim_end_matches('/').to_string();
    let (sender, receiver) = mpsc::channel(MAX_FILES_PAGE_SIZE * 2);

    tokio::spawn(async move {
        let mut objects = store.read_dir_all(&base, None);
        while let Some(result) = objects.next().await {
            let entry = result.map_err(|error| error.to_string()).map(|object| {
                let full_path = object.location.as_ref();
                let relative = full_path
                    .strip_prefix(&base_string)
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
            });
            match entry {
                Ok(None) => continue,
                Ok(Some(entry)) => {
                    if sender.send(Ok(entry)).await.is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error)).await;
                    break;
                }
            }
        }
    });

    Ok(FileListing {
        receiver,
        pending: None,
        next_offset: 0,
        last_offset: None,
        last_page: None,
    })
}

async fn read_file_page(
    listing: &mut FileListing,
    offset: usize,
    limit: usize,
) -> Result<FilesPage> {
    if listing.last_offset == Some(offset) {
        return Ok(listing
            .last_page
            .as_ref()
            .expect("last file page set with offset")
            .clone());
    }
    if offset != listing.next_offset {
        bail!(InvalidRequest(format!(
            "expected file offset {}, received {offset}",
            listing.next_offset
        )));
    }

    let mut entries = Vec::with_capacity(limit);
    if let Some(entry) = listing.pending.take() {
        entries.push(entry);
    }
    while entries.len() < limit {
        match listing.receiver.recv().await {
            Some(Ok(entry)) => entries.push(entry),
            Some(Err(error)) => bail!("failed to list dataset files: {error}"),
            None => break,
        }
    }

    let has_more = if entries.len() == limit {
        match listing.receiver.recv().await {
            Some(Ok(entry)) => {
                listing.pending = Some(entry);
                true
            }
            Some(Err(error)) => bail!("failed to list dataset files: {error}"),
            None => false,
        }
    } else {
        false
    };
    let page = FilesPage {
        next_offset: has_more.then_some(offset + entries.len()),
        entries,
    };
    listing.next_offset = page.next_offset.unwrap_or(offset + page.entries.len());
    listing.last_offset = Some(offset);
    listing.last_page = Some(page.clone());
    Ok(page)
}

#[derive(Debug, Deserialize)]
pub(crate) struct FileQuery {
    connection_id: Uuid,
    path: String,
}

pub(crate) async fn file_preview(
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
    let full_path = validated_dataset_path(
        connection.dataset.branch_location().path,
        path,
        |base, segment| base.join(segment),
    )?;
    let store = connection.dataset.object_store(None).await?;
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

pub(crate) async fn transaction(
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
    let full_path = validated_dataset_path(
        connection.dataset.branch_location().path,
        path,
        |base, segment| base.join(segment),
    )?;
    if !path.ends_with(".txn") {
        bail!("transaction path must end in .txn");
    }
    let store = connection.dataset.object_store(None).await?;
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

fn validated_dataset_path<T>(base: T, path: &str, join: impl FnMut(T, &str) -> T) -> Result<T> {
    if path.is_empty()
        || path.starts_with('/')
        || path.split('/').any(|part| part == ".." || part.is_empty())
    {
        bail!("invalid dataset-relative path");
    }
    Ok(path.split('/').fold(base, join))
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

    #[tokio::test]
    async fn file_listing_pages_once_and_retries_idempotently() {
        let (sender, receiver) = mpsc::channel(8);
        for index in 0..5 {
            sender
                .send(Ok(FileEntry {
                    path: format!("data/{index}.lance"),
                    size: index,
                    kind: "data",
                    modified: "now".to_string(),
                }))
                .await
                .unwrap();
        }
        drop(sender);
        let mut listing = FileListing {
            receiver,
            pending: None,
            next_offset: 0,
            last_offset: None,
            last_page: None,
        };

        let first = read_file_page(&mut listing, 0, 2).await.unwrap();
        assert_eq!(first.entries.len(), 2);
        assert_eq!(first.next_offset, Some(2));
        let retry = read_file_page(&mut listing, 0, 2).await.unwrap();
        assert_eq!(
            retry
                .entries
                .iter()
                .map(|entry| &entry.path)
                .collect::<Vec<_>>(),
            first
                .entries
                .iter()
                .map(|entry| &entry.path)
                .collect::<Vec<_>>()
        );

        let second = read_file_page(&mut listing, 2, 2).await.unwrap();
        assert_eq!(second.entries.len(), 2);
        assert_eq!(second.next_offset, Some(4));
        let last = read_file_page(&mut listing, 4, 2).await.unwrap();
        assert_eq!(last.entries.len(), 1);
        assert_eq!(last.next_offset, None);
        assert!(read_file_page(&mut listing, 7, 2).await.is_err());
    }

    #[test]
    fn rejects_paths_outside_dataset() {
        let join = |mut base: String, segment: &str| {
            if !base.is_empty() {
                base.push('/');
            }
            base.push_str(segment);
            base
        };
        assert_eq!(
            validated_dataset_path(String::new(), "data/part.lance", join).unwrap(),
            "data/part.lance"
        );
        assert!(validated_dataset_path(String::new(), "../secret", join).is_err());
        assert!(validated_dataset_path(String::new(), "/etc/passwd", join).is_err());
        assert!(validated_dataset_path(String::new(), "data//part.lance", join).is_err());
    }
}
