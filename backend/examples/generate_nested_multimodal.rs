use std::{path::PathBuf, sync::Arc};

use arrow_array::{Int32Array, ListArray, RecordBatch, RecordBatchIterator};
use arrow_buffer::{NullBuffer, OffsetBuffer};
use arrow_schema::{DataType, Field, Schema};
use lance::{BlobArrayBuilder, Dataset, blob_field, dataset::write::WriteParams};
use lance_file::version::LanceFileVersion;

const JPEG: &[u8] = b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00\xff\xd9";
const WAV: &[u8] = b"RIFF\x24\x00\x00\x00WAVEfmt \x10\x00\x00\x00\x01\x00\x01\x00\x40\x1f\x00\x00\x80\x3e\x00\x00\x02\x00\x10\x00data\x00\x00\x00\x00";
const MP4: &[u8] = b"\x00\x00\x00\x18ftypisom\x00\x00\x02\x00isomiso2";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../testdata/nested_multimodal.lance"));

    if output.exists() {
        std::fs::remove_dir_all(&output)?;
    }

    let mut image = BlobArrayBuilder::new(2);
    image.push_bytes(JPEG)?;
    image.push_null()?;

    let mut audio = BlobArrayBuilder::new(2);
    audio.push_bytes(WAV)?;
    audio.push_null()?;

    let mut video = BlobArrayBuilder::new(2);
    video.push_bytes(MP4)?;
    video.push_null()?;

    let blob_item = Arc::new(blob_field("item", true));
    let mut nested_values = BlobArrayBuilder::new(4);
    nested_values.push_bytes(JPEG)?;
    nested_values.push_bytes(WAV)?;
    nested_values.push_bytes(MP4)?;
    nested_values.push_null()?;
    let inner_field = Arc::new(Field::new("items", DataType::List(blob_item.clone()), true));
    let inner = Arc::new(ListArray::new(
        blob_item,
        OffsetBuffer::new(vec![0_i32, 2, 4].into()),
        nested_values.finish()?,
        None,
    ));
    let nested = Arc::new(ListArray::new(
        inner_field.clone(),
        OffsetBuffer::new(vec![0_i32, 2, 2].into()),
        inner,
        Some(NullBuffer::from(vec![true, false])),
    ));

    let schema = Arc::new(Schema::new(vec![
        Field::new("id", DataType::Int32, false),
        blob_field("image", true),
        blob_field("audio", true),
        blob_field("video", true),
        Field::new("nested_media", DataType::List(inner_field), true),
    ]));
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int32Array::from(vec![1, 2])),
            image.finish()?,
            audio.finish()?,
            video.finish()?,
            nested,
        ],
    )?;
    Dataset::write(
        RecordBatchIterator::new([Ok(batch)], schema),
        output.to_string_lossy().as_ref(),
        Some(WriteParams {
            data_storage_version: Some(LanceFileVersion::V2_3),
            ..Default::default()
        }),
    )
    .await?;
    println!("{}", output.display());
    Ok(())
}
