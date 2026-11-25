use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

pub enum ApiError {
    BadRequest(&'static str),
    Unauthorized,
    /// Logged in, but this account may not do that (a cashier on an admin-only action).
    Forbidden,
    NotFound,
    TooMany,
    /// The action is allowed, but only after the user confirms (e.g. calling out of turn).
    Conflict(String),
    /// A print job failed; the message is safe to show to the cashier.
    PrintFailed(String),
    /// A needed service (payment checking) is not available right now.
    Unavailable(&'static str),
    Internal(anyhow::Error),
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(err: E) -> Self {
        Self::Internal(err.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.to_string()),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "กรุณาเข้าสู่ระบบ".to_string()),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "บัญชีนี้ไม่มีสิทธิ์ทำรายการนี้ (ต้องเป็นแอดมิน)".to_string(),
            ),
            Self::NotFound => (StatusCode::NOT_FOUND, "ไม่พบข้อมูล".to_string()),
            Self::TooMany => (
                StatusCode::TOO_MANY_REQUESTS,
                "ทำรายการถี่เกินไป ลองใหม่ภายหลัง".to_string(),
            ),
            Self::Conflict(msg) => (StatusCode::CONFLICT, msg),
            Self::PrintFailed(msg) => (StatusCode::BAD_GATEWAY, format!("พิมพ์ไม่สำเร็จ: {msg}")),
            Self::Unavailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg.to_string()),
            Self::Internal(err) => {
                tracing::error!(error = %err, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "เกิดข้อผิดพลาดในระบบ".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
