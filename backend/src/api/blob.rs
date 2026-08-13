use anyhow::{Context, Result, bail};
use arrow_array::{Array, ArrayRef, StructArray, cast::AsArray};
use file_format::{FileFormat, Kind};
use futures::TryStreamExt;
use lance_core::datatypes::BlobHandling;

use super::state::ConnectedDataset;

/// Materializes one nested Blob column using the requested native Lance
/// representation. `AllBinary` loads every Blob payload in the selected row
/// because Lance does not yet expose indexed nested-Blob reads.
pub(super) async fn load_blob_array(
    connection: &ConnectedDataset,
    column: &str,
    row_address: u64,
    handling: BlobHandling,
) -> Result<ArrayRef> {
    let mut scanner = connection.dataset.scan();
    scanner.project(&[column])?;
    scanner.with_row_address();
    scanner.filter(&format!("_rowaddr = {row_address}"))?;
    scanner.blob_handling(handling);
    let mut stream = scanner.try_into_stream().await?;
    let batch = stream
        .try_next()
        .await?
        .context("blob array row not found")?;
    batch
        .column_by_name(column)
        .cloned()
        .with_context(|| format!("blob array column {column} was not returned"))
}

/// Converts native Lance descriptor arrays into the media metadata returned
/// alongside one serialized row.
pub(super) fn media_items(descriptor: &dyn Array, row_index: usize) -> Result<Vec<Vec<usize>>> {
    let mut items = Vec::new();
    collect_media_items(descriptor, row_index, &mut Vec::new(), &mut items)?;
    Ok(items)
}

fn collect_media_items(
    array: &dyn Array,
    index: usize,
    path: &mut Vec<usize>,
    output: &mut Vec<Vec<usize>>,
) -> Result<()> {
    if index >= array.len() || array.is_null(index) {
        return Ok(());
    }
    if let Some(values) = list_values(array, index) {
        for child_index in 0..values.len() {
            path.push(child_index);
            collect_media_items(values.as_ref(), child_index, path, output)?;
            path.pop();
        }
        return Ok(());
    }
    let Some(descriptor) = array.as_any().downcast_ref::<StructArray>() else {
        return Ok(());
    };
    if descriptor
        .column_by_name("kind")
        .is_some_and(|kind| kind.is_null(index))
    {
        return Ok(());
    }
    output.push(path.clone());
    Ok(())
}

pub(super) fn list_value(array: &dyn Array, index: usize, column: &str) -> Result<ArrayRef> {
    if index >= array.len() {
        bail!(
            "blob index {index} is outside column {column} with {} items",
            array.len()
        );
    }
    if array.is_null(index) {
        bail!("blob array item {index} is null");
    }
    list_values(array, index)
        .with_context(|| format!("column {column} does not contain the requested blob array depth"))
}

fn list_values(array: &dyn Array, index: usize) -> Option<ArrayRef> {
    if let Some(list) = array.as_list_opt::<i32>() {
        Some(list.value(index))
    } else if let Some(list) = array.as_list_opt::<i64>() {
        Some(list.value(index))
    } else {
        array.as_fixed_size_list_opt().map(|list| list.value(index))
    }
}

pub(super) fn parse_index_path(value: Option<&str>) -> Result<Vec<usize>> {
    let value = value.context("blob array media requests require an index")?;
    let path = value
        .split(',')
        .map(|index| {
            index
                .parse::<usize>()
                .with_context(|| format!("invalid blob array index {index:?}"))
        })
        .collect::<Result<Vec<_>>>()?;
    if path.is_empty() {
        bail!("blob array media requests require an index");
    }
    Ok(path)
}

pub(super) fn detect_media_type(bytes: &[u8]) -> String {
    let format = FileFormat::from_bytes(bytes);
    match format.kind() {
        Kind::Image | Kind::Audio | Kind::Video => format.media_type().to_string(),
        // A short prefix cannot expose MP4 track metadata. Browsers can still
        // inspect the container and reject non-video payloads safely.
        _ if format == FileFormat::Mpeg4Part14 => "video/mp4".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}
