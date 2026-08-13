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
pub(super) struct UnknownDiscovery(pub(super) Uuid);

#[derive(Debug)]
pub(super) struct UnknownQueryCursor(pub(super) Uuid);

#[derive(Debug)]
pub(super) struct InvalidRequest(pub(super) String);

#[derive(Debug)]
pub(super) struct QueryExecutionFailed(pub(super) String);

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

impl std::fmt::Display for UnknownDiscovery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "dataset discovery {} was not found or has expired; rediscover the dataset",
            self.0
        )
    }
}

impl std::error::Error for UnknownDiscovery {}

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
        let status = if self.0.downcast_ref::<UnknownConnection>().is_some()
            || self.0.downcast_ref::<UnknownDiscovery>().is_some()
        {
            StatusCode::GONE
        } else if self.0.downcast_ref::<UnknownQueryCursor>().is_some() {
            StatusCode::NOT_FOUND
        } else if self.0.downcast_ref::<QueryExecutionFailed>().is_some() {
            StatusCode::UNPROCESSABLE_ENTITY
        } else if self.0.downcast_ref::<InvalidRequest>().is_some() {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        if status.is_server_error() {
            eprintln!("request failed: {:#}", self.0);
        }
        (
            status,
            Json(json!({ "error": client_error_message(&self.0) })),
        )
            .into_response()
    }
}

/// Formats an error for an API response without repeating every intermediate
/// error wrapper. The outer context explains the failed operation, while the
/// root cause contains the actionable detail.
fn client_error_message(error: &anyhow::Error) -> String {
    let context = error.to_string();
    let root_cause = error.root_cause().to_string();
    if context == root_cause {
        context
    } else {
        format!("{context}: {root_cause}")
    }
}
