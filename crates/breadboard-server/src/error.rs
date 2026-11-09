//! One error type for the whole HTTP surface.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("forbidden")]
    Forbidden,
    #[error("{0}")]
    ForbiddenMsg(String),
    #[error("conflict: {0}")]
    Conflict(String),
    /// A live agent name is already held by a different session. The response
    /// carries the holder session id and free alternatives so a registering
    /// client can rename before claiming instead of guessing.
    #[error("{message}")]
    AgentNameTaken {
        message: String,
        agent_name: String,
        session_id: Option<String>,
        alternatives: Vec<String>,
    },
    #[error("{0}")]
    TooLarge(String),
    #[error("{0}")]
    UnsupportedMediaType(String),
    #[error("expired")]
    Expired,
    #[error("upstream: {0}")]
    Upstream(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        AppError::BadRequest(msg.into())
    }
    pub fn forbidden() -> Self {
        AppError::Forbidden
    }
    pub fn forbidden_msg(msg: impl Into<String>) -> Self {
        AppError::ForbiddenMsg(msg.into())
    }
    pub fn unsupported_media_type(msg: impl Into<String>) -> Self {
        AppError::UnsupportedMediaType(msg.into())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "not found".to_string()),
            AppError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "forbidden".to_string()),
            AppError::ForbiddenMsg(m) => (StatusCode::FORBIDDEN, m.clone()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, m.clone()),
            AppError::AgentNameTaken {
                message,
                agent_name,
                session_id,
                alternatives,
            } => {
                return (
                    StatusCode::CONFLICT,
                    Json(json!({
                        "error": message,
                        "agent_name": agent_name,
                        "session_id": session_id,
                        "alternatives": alternatives,
                    })),
                )
                    .into_response();
            }
            AppError::TooLarge(m) => (StatusCode::PAYLOAD_TOO_LARGE, m.clone()),
            AppError::UnsupportedMediaType(m) => (StatusCode::UNSUPPORTED_MEDIA_TYPE, m.clone()),
            AppError::Expired => (StatusCode::GONE, "expired".to_string()),
            AppError::Upstream(m) => (StatusCode::BAD_GATEWAY, m.clone()),
            AppError::Db(sqlx::Error::RowNotFound) => {
                (StatusCode::NOT_FOUND, "not found".to_string())
            }
            AppError::Db(e) => {
                tracing::error!(error = %e, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
            AppError::Other(e) => {
                tracing::error!(error = %e, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal error".to_string(),
                )
            }
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}
