use aide::OperationInput;
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use jsonwebtoken::{Algorithm, Validation, decode};
use serde::Deserialize;
use uuid::Uuid;
use zyndeck_core::User;
use zyndeck_db::UserRepository;

use crate::error::ApiError;
use crate::state::AppState;

/// Claims we read from the JWT. `exp` is validated by `jsonwebtoken` itself, so
/// it does not need to appear here; we only need the subject (the user id).
#[derive(Debug, Deserialize)]
struct Claims {
    sub: String,
}

/// The authenticated caller, resolved from a verified `Bearer` JWT.
///
/// The token only carries identity (`sub`); the role is loaded fresh from the
/// database so authorization always reflects the current role.
pub struct AuthUser(pub User);

impl<G, U> FromRequestParts<AppState<G, U>> for AuthUser
where
    G: Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState<G, U>,
    ) -> Result<Self, ApiError> {
        let header = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .ok_or(ApiError::Unauthorized)?;
        let token = header
            .strip_prefix("Bearer ")
            .ok_or(ApiError::Unauthorized)?;

        let mut validation = Validation::new(Algorithm::HS256);
        // Tokens carry no audience; only the signature and expiry matter here.
        validation.validate_aud = false;
        let data = decode::<Claims>(token, &state.decoding_key, &validation)
            .map_err(|_| ApiError::Unauthorized)?;

        let id = Uuid::parse_str(&data.claims.sub).map_err(|_| ApiError::Unauthorized)?;

        let user = state
            .users
            .find_by_id(id)
            .await?
            .ok_or(ApiError::Unauthorized)?;

        Ok(AuthUser(user))
    }
}

// No request body/params to document for authentication.
impl OperationInput for AuthUser {}

/// Rejects the caller unless they have administrative privileges.
pub fn require_admin(user: &User) -> Result<(), ApiError> {
    if user.role.is_admin() {
        Ok(())
    } else {
        Err(ApiError::Forbidden)
    }
}
