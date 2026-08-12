use axum::{
    Json,
    http::{
        HeaderValue, StatusCode,
        header::{ACCEPT_RANGES, CONTENT_RANGE},
    },
    response::{IntoResponse, Response},
};
use serde_json::json;
use uuid::Uuid;

pub(crate) struct ApiError(pub(super) anyhow::Error);

#[derive(Debug)]
pub(super) struct UnknownConnection(pub(super) Uuid);

#[derive(Debug)]
pub(super) struct UnknownQueryCursor(pub(super) Uuid);

#[derive(Debug)]
pub(super) struct InvalidRequest(pub(super) String);

#[derive(Debug)]
pub(super) struct QueryExecutionFailed(pub(super) String);

#[derive(Debug)]
pub(super) struct BlobUnavailable {
    pub(super) column: String,
    pub(super) row_address: u64,
}

#[derive(Debug)]
pub(super) struct RangeNotSatisfiable {
    pub(super) size: u64,
    pub(super) message: String,
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

impl std::fmt::Display for BlobUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "blob column '{}' is null or unavailable at row address {}",
            self.column, self.row_address
        )
    }
}

impl std::error::Error for BlobUnavailable {}

impl std::fmt::Display for RangeNotSatisfiable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for RangeNotSatisfiable {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
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
        } else if self.0.downcast_ref::<UnknownQueryCursor>().is_some()
            || self.0.downcast_ref::<BlobUnavailable>().is_some()
        {
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
