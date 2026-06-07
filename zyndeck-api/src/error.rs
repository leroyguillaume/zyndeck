use aide::OperationOutput;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use schemars::JsonSchema;
use serde::Serialize;
use zyndeck_core::InvalidLanguageCode;
use zyndeck_db as db;

/// An API-level error, rendered as an HTTP status plus a JSON body.
#[derive(Debug)]
pub enum ApiError {
    /// 401 — no/invalid credentials.
    Unauthorized,
    /// 403 — authenticated but not allowed.
    Forbidden,
    /// 404 — the resource does not exist.
    NotFound,
    /// 409 — a uniqueness conflict (e.g. username taken).
    Conflict(String),
    /// 422 — the request body failed validation.
    Validation(String),
    /// 500 — an unexpected server-side fault.
    Internal,
}

/// Wire shape of an error response.
#[derive(Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    message: String,
}

impl ApiError {
    fn parts(self) -> (StatusCode, String) {
        match self {
            ApiError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "authentication required".to_owned(),
            ),
            ApiError::Forbidden => (
                StatusCode::FORBIDDEN,
                "you are not allowed to perform this action".to_owned(),
            ),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "resource not found".to_owned()),
            ApiError::Conflict(message) => (StatusCode::CONFLICT, message),
            ApiError::Validation(message) => (StatusCode::UNPROCESSABLE_ENTITY, message),
            ApiError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_owned(),
            ),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = self.parts();
        (status, Json(ErrorBody { message })).into_response()
    }
}

// Lets `Result<T, ApiError>` be a documented handler return for aide.
impl OperationOutput for ApiError {
    type Inner = ();
}

impl From<db::Error> for ApiError {
    fn from(error: db::Error) -> Self {
        match error {
            db::Error::UsernameTaken(name) => {
                ApiError::Conflict(format!("username {name:?} is already taken"))
            }
            // Connect / Migrate / Query / InvalidRole are server-side faults.
            other => {
                tracing::error!(error = %other, "database error");
                ApiError::Internal
            }
        }
    }
}

impl From<validator::ValidationErrors> for ApiError {
    fn from(error: validator::ValidationErrors) -> Self {
        ApiError::Validation(error.to_string())
    }
}

impl From<InvalidLanguageCode> for ApiError {
    fn from(error: InvalidLanguageCode) -> Self {
        ApiError::Validation(error.to_string())
    }
}
