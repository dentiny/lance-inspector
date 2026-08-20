use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use arrow_array::{Array, cast::AsArray};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{
        HeaderMap, HeaderValue, Method, Response, StatusCode,
        header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE},
    },
};
use lance_core::datatypes::BlobHandling;
use serde::Deserialize;
use uuid::Uuid;

use super::{
    blob::{detect_media_type, list_value, load_blob_array, parse_index_path},
    error::{ApiError, RequestError::RangeNotSatisfiable},
    schema::{is_blob_array_field, is_blob_field},
    state::{AppState, ConnectedDataset, connected},
};

#[derive(Debug, Deserialize)]
pub(crate) struct MediaQuery {
    connection_id: Uuid,
    index: Option<String>,
}

pub(crate) async fn media(
    State(state): State<Arc<AppState>>,
    Path((field_id, row_address)): Path<(i32, u64)>,
    Query(query): Query<MediaQuery>,
    headers: HeaderMap,
    method: Method,
) -> Result<Response<Body>, ApiError> {
    let connection = connected(&state, query.connection_id)?;
    let source_field = connection
        .dataset
        .schema()
        .fields
        .iter()
        .find(|field| field.id == field_id)
        .with_context(|| format!("unknown source field id {field_id}"))?;
    let column = source_field.name.clone();
    let arrow_field = arrow_schema::Field::from(source_field);
    let blob_array = is_blob_array_field(&arrow_field);
    if !blob_array && !is_blob_field(&arrow_field) {
        return Err(ApiError(anyhow!(
            "column {column} is not a Lance blob column"
        )));
    }
    Ok(read_media(
        &connection,
        &column,
        blob_array,
        row_address,
        query,
        &headers,
        method == Method::HEAD,
    )
    .await?)
}

async fn read_media(
    connection: &ConnectedDataset,
    column: &str,
    blob_array: bool,
    row_address: u64,
    query: MediaQuery,
    headers: &HeaderMap,
    head: bool,
) -> Result<Response<Body>> {
    if blob_array {
        let index_path = parse_index_path(query.index.as_deref())?;
        return read_blob_array_media(connection, column, row_address, &index_path, headers, head)
            .await;
    }

    if query.index.is_some() {
        bail!("blob index is only valid for blob array columns");
    }

    let mut blobs = connection
        .dataset
        .take_blobs_by_addresses(&[row_address], column)
        .await?;
    let blob = blobs
        .pop()
        .flatten()
        .ok_or_else(|| anyhow!("blob row not found"))?;
    let size = blob.size();
    let (start, end, partial) = parse_range(headers.get(RANGE), size)?;
    let prefix = blob.read_range(0..size.min(32)).await?;
    let mime = detect_media_type(&prefix);
    let bytes = if head {
        Vec::new()
    } else {
        blob.read_range(start..end).await?.to_vec()
    };
    media_response(bytes, &mime, start, end, size, partial, head)
}

async fn read_blob_array_media(
    connection: &ConnectedDataset,
    column: &str,
    row_address: u64,
    index_path: &[usize],
    headers: &HeaderMap,
    head: bool,
) -> Result<Response<Body>> {
    let slot = connection.blob_array_slot(column, row_address);
    let array = slot
        .get_or_try_init(|| {
            load_blob_array(connection, column, row_address, BlobHandling::AllBinary)
        })
        .await?;
    let mut values = list_value(array.as_ref(), 0, column)?;
    for index in &index_path[..index_path.len() - 1] {
        values = list_value(values.as_ref(), *index, column)?;
    }
    let index = *index_path.last().expect("index path is non-empty");
    let blobs = values.as_binary_opt::<i64>().with_context(|| {
        format!("blob array column {column} did not materialize as binary values")
    })?;
    if index >= blobs.len() {
        bail!(
            "blob index {index} is outside column {column} with {} items",
            blobs.len()
        );
    }
    if blobs.is_null(index) {
        bail!("blob array item {index} is null");
    }
    let bytes = blobs.value(index);
    let size = bytes.len() as u64;
    let (start, end, partial) = parse_range(headers.get(RANGE), size)?;
    let mime = detect_media_type(bytes);
    let body = if head {
        Vec::new()
    } else {
        bytes[start as usize..end as usize].to_vec()
    };
    media_response(body, &mime, start, end, size, partial, head)
}

fn media_response(
    bytes: Vec<u8>,
    mime: &str,
    start: u64,
    end: u64,
    size: u64,
    partial: bool,
    head: bool,
) -> Result<Response<Body>> {
    let content_length = if head {
        end.saturating_sub(start)
    } else {
        bytes.len() as u64
    };
    let mut response = Response::builder()
        .status(if partial {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(CONTENT_TYPE, HeaderValue::from_str(mime)?)
        .header(ACCEPT_RANGES, "bytes")
        .header(CONTENT_LENGTH, content_length.to_string());
    if partial {
        response = response.header(CONTENT_RANGE, format!("bytes {start}-{}/{}", end - 1, size));
    }
    Ok(response.body(if head {
        Body::empty()
    } else {
        Body::from(bytes)
    })?)
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

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ListArray, RecordBatch, RecordBatchIterator};
    use arrow_buffer::OffsetBuffer;
    use arrow_schema::{DataType, Field, Schema};
    use axum::response::IntoResponse;
    use lance::{BlobArrayBuilder, Dataset, blob_field, dataset::write::WriteParams};
    use lance_file::version::LanceFileVersion;

    use crate::api::dataset::build_dataset_info;

    #[test]
    fn detects_inline_media_types() {
        assert_eq!(detect_media_type(b"\xff\xd8\xff"), "image/jpeg");
        assert_eq!(
            detect_media_type(b"RIFF\0\0\0\0WAVEpayload"),
            "audio/vnd.wave"
        );
        assert_eq!(
            detect_media_type(b"\0\0\0\x18ftypisom\0\0\0\0isomiso2"),
            "video/mp4"
        );
    }

    #[test]
    fn parses_nested_blob_index_paths() {
        assert_eq!(parse_index_path(Some("2,4,1")).unwrap(), vec![2, 4, 1]);
        assert!(parse_index_path(Some("2,nope")).is_err());
        assert!(parse_index_path(None).is_err());
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
    fn keeps_content_type_stable_across_ranges_and_head() {
        let first = media_response(vec![0; 10], "video/mp4", 0, 10, 100, true, false).unwrap();
        let later = media_response(vec![0; 10], "video/mp4", 50, 60, 100, true, true).unwrap();

        assert_eq!(first.headers()[CONTENT_TYPE], "video/mp4");
        assert_eq!(later.headers()[CONTENT_TYPE], "video/mp4");
        assert_eq!(later.headers()[CONTENT_LENGTH], "10");
        assert_eq!(later.status(), StatusCode::PARTIAL_CONTENT);
    }

    #[tokio::test]
    async fn serves_blob_array_items_with_backend_owned_mime() {
        let item_field = Arc::new(blob_field("item", true));
        let mut blobs = BlobArrayBuilder::new(3);
        blobs.push_bytes(b"\x89PNG\r\n\x1a\n").unwrap();
        blobs.push_null().unwrap();
        blobs.push_empty().unwrap();
        let blob_list = Arc::new(ListArray::new(
            item_field.clone(),
            OffsetBuffer::new(vec![0_i32, 3].into()),
            blobs.finish().unwrap(),
            None,
        ));
        let schema = Arc::new(Schema::new(vec![Field::new(
            "images",
            DataType::List(item_field),
            true,
        )]));
        let batch = RecordBatch::try_new(schema.clone(), vec![blob_list]).unwrap();
        let uri = format!("memory://media-array-test-{}", Uuid::new_v4());
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
        let mut headers = HeaderMap::new();
        headers.insert(RANGE, HeaderValue::from_static("bytes=1-3"));
        let response = read_media(
            &connection,
            "images",
            true,
            0,
            MediaQuery {
                connection_id: Uuid::new_v4(),
                index: Some("0".to_string()),
            },
            &headers,
            false,
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[CONTENT_TYPE], "image/png");
        assert_eq!(response.headers()[CONTENT_RANGE], "bytes 1-3/8");
        assert!(
            read_media(
                &connection,
                "images",
                true,
                0,
                MediaQuery {
                    connection_id: Uuid::new_v4(),
                    index: Some("1".to_string()),
                },
                &HeaderMap::new(),
                false,
            )
            .await
            .is_err()
        );
        let empty = read_media(
            &connection,
            "images",
            true,
            0,
            MediaQuery {
                connection_id: Uuid::new_v4(),
                index: Some("2".to_string()),
            },
            &HeaderMap::new(),
            true,
        )
        .await
        .unwrap();
        assert_eq!(empty.status(), StatusCode::OK);
        assert_eq!(empty.headers()[CONTENT_LENGTH], "0");
    }
}
