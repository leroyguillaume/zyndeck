use aide::axum::ApiRouter;
use aide::axum::routing::post_with;
use aide::transform::TransformOperation;
use axum::Json;
use axum::extract::State;
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;
use zyndeck_db::UserRepository;

use crate::error::ApiError;
use crate::password::verify_password;
use crate::state::AppState;

/// Credentials submitted to log in.
#[derive(Debug, Deserialize, JsonSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct LoginRequest {
    #[validate(length(min = 1))]
    pub username: String,
    #[validate(length(min = 1))]
    pub password: String,
}

/// The issued bearer token. Self-contained (the claims travel in the JWT), so
/// only the token itself is returned.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LoginResponse {
    pub access_token: String,
}

/// JWT claims we sign.
#[derive(Serialize)]
struct Claims {
    sub: String,
    exp: usize,
}

/// The login route (public).
pub fn router<G, U>() -> ApiRouter<AppState<G, U>>
where
    G: Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    ApiRouter::new().api_route("/auth/login", post_with(login::<G, U>, login_docs))
}

async fn login<G, U>(
    State(state): State<AppState<G, U>>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, ApiError>
where
    G: Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    body.validate()?;

    // Same 401 whether the username is unknown or the password is wrong, so the
    // endpoint does not reveal which usernames exist.
    let credentials = state
        .users
        .find_credentials_by_username(body.username)
        .await?
        .ok_or(ApiError::Unauthorized)?;
    if !verify_password(&body.password, &credentials.password_hash) {
        return Err(ApiError::Unauthorized);
    }

    let access_token = issue_token(&state.encoding_key, credentials.id, state.token_ttl_seconds)?;
    Ok(Json(LoginResponse { access_token }))
}

fn issue_token(key: &EncodingKey, user_id: Uuid, ttl_seconds: u64) -> Result<String, ApiError> {
    let exp = (Utc::now() + Duration::seconds(ttl_seconds as i64)).timestamp() as usize;
    let claims = Claims {
        sub: user_id.to_string(),
        exp,
    };
    encode(&Header::new(Algorithm::HS256), &claims, key).map_err(|error| {
        tracing::error!(%error, "failed to issue token");
        ApiError::Internal
    })
}

fn login_docs(op: TransformOperation) -> TransformOperation {
    op.tag("Authentication")
        .description("Exchange a username and password for a bearer token.")
        .response::<200, Json<LoginResponse>>()
}
