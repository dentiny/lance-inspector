use axum::Json;

use crate::models::HealthResponse;

pub(crate) async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}
