use crate::models::lifecycle::LifecycleError;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

pub(crate) struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}
impl ApiError {
    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }
    pub(crate) fn message(&self) -> &str {
        &self.message
    }
    pub(crate) fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
    pub(crate) fn timeout() -> Self {
        Self::new(
            StatusCode::GATEWAY_TIMEOUT,
            "request_timeout",
            "request deadline exceeded",
        )
    }
}
impl From<LifecycleError> for ApiError {
    fn from(error: LifecycleError) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "backend_unavailable",
            error.to_string(),
        )
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let kind = if self.status.is_client_error() {
            "invalid_request_error"
        } else {
            "server_error"
        };
        (
            self.status,
            Json(
                json!({"error":{"message":self.message,"type":kind,"code":self.code,"param":null}}),
            ),
        )
            .into_response()
    }
}
