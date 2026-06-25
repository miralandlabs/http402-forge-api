use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("payment required")]
    PaymentRequired(Value),

    #[error("validation error")]
    Validation(Vec<FieldError>),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("not found")]
    NotFound,

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("payment config: {0}")]
    PaymentConfig(String),

    #[error("{0}")]
    Internal(#[from] anyhow::Error),
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl AppError {
    pub fn validation(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Validation(vec![FieldError {
            field: field.into(),
            message: message.into(),
        }])
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            AppError::PaymentRequired(v) => (StatusCode::PAYMENT_REQUIRED, v),
            AppError::Validation(details) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({ "error": "Validation error", "details": details }),
            ),
            AppError::Storage(msg) => (
                StatusCode::BAD_GATEWAY,
                json!({ "error": "Storage error", "message": msg }),
            ),
            AppError::NotFound => (StatusCode::NOT_FOUND, json!({ "error": "Not found" })),
            AppError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                json!({ "error": "Bad request", "message": msg }),
            ),
            AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, json!({ "error": msg })),
            AppError::PaymentConfig(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                json!({ "error": "Payment requirements unavailable", "message": msg }),
            ),
            AppError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Internal server error" }),
            ),
        };
        (status, Json(body)).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
