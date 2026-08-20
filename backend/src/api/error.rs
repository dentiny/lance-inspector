use axum::{
    Json,
    http::{
        HeaderValue, StatusCode,
        header::{ACCEPT_RANGES, CONTENT_RANGE},
    },
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;
use uuid::Uuid;

pub(crate) struct ApiError(pub(super) anyhow::Error);

#[derive(Debug, Error)]
pub(super) enum RequestError {
    #[error("connection {0} was not found or has expired; reconnect the dataset")]
    UnknownConnection(Uuid),
    #[error("dataset discovery {0} was not found or has expired; rediscover the dataset")]
    UnknownDiscovery(Uuid),
    #[error("SQL cursor {0} was not found or has expired; rerun the query")]
    UnknownQueryCursor(Uuid),
    #[error("{0}")]
    InvalidRequest(String),
    #[error("SQL execution failed; rerun the query: {0}")]
    QueryExecutionFailed(String),
    #[error("{message}")]
    RangeNotSatisfiable { size: u64, message: String },
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let request_error = self.0.downcast_ref::<RequestError>();
        if let Some(RequestError::RangeNotSatisfiable { size, .. }) = request_error {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [
                    (ACCEPT_RANGES, HeaderValue::from_static("bytes")),
                    (
                        CONTENT_RANGE,
                        HeaderValue::from_str(&format!("bytes */{size}"))
                            .expect("blob size always produces a valid header"),
                    ),
                ],
                Json(json!({ "error": client_error_message(&self.0) })),
            )
                .into_response();
        }
        let status = match request_error {
            Some(RequestError::UnknownConnection(_) | RequestError::UnknownDiscovery(_)) => {
                StatusCode::GONE
            }
            Some(RequestError::UnknownQueryCursor(_)) => StatusCode::NOT_FOUND,
            Some(RequestError::QueryExecutionFailed(_)) => StatusCode::UNPROCESSABLE_ENTITY,
            Some(RequestError::InvalidRequest(_)) => StatusCode::BAD_REQUEST,
            Some(RequestError::RangeNotSatisfiable { .. }) => unreachable!(),
            None => StatusCode::INTERNAL_SERVER_ERROR,
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
