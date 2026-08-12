use std::sync::Arc;

use anyhow::{Context, Result, anyhow, bail};
use arrow_schema::Schema as ArrowSchema;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{
        HeaderMap, HeaderValue, Response, StatusCode,
        header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE},
    },
};
use serde::Deserialize;
use uuid::Uuid;

use super::{
    error::{ApiError, BlobUnavailable, RangeNotSatisfiable},
    schema::is_blob_field,
    state::{AppState, ConnectedDataset, connected},
};

#[derive(Debug, Deserialize)]
pub(crate) struct MediaQuery {
    connection_id: Uuid,
    mime: Option<String>,
}

pub(crate) async fn media(
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
    let blob = blobs.pop().flatten().ok_or_else(|| BlobUnavailable {
        column: column.to_string(),
        row_address,
    })?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;

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
    fn reports_null_or_missing_blobs_as_not_found() {
        let error = anyhow!(BlobUnavailable {
            column: "image".to_string(),
            row_address: 42,
        });
        assert_eq!(
            ApiError(error).into_response().status(),
            StatusCode::NOT_FOUND
        );
    }
}
