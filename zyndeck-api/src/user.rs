use aide::axum::ApiRouter;
use aide::axum::routing::get_with;
use aide::transform::TransformOperation;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use futures::{StreamExt, TryStreamExt};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;
use zyndeck_core::{Role, User};
use zyndeck_db::{NewUser, UserRepository, UserUpdate};

use crate::auth::{AuthUser, require_admin};
use crate::error::ApiError;
use crate::hash_password;
use crate::pagination::{Page, PaginationQuery};
use crate::state::AppState;

/// Request body to create a user.
#[derive(Debug, Deserialize, JsonSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct CreateUserRequest {
    #[validate(length(min = 1, max = 100))]
    pub username: String,
    #[validate(length(min = 8, max = 128))]
    pub password: String,
    pub role: Role,
}

/// Request body to update a user.
#[derive(Debug, Deserialize, JsonSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct UpdateUserRequest {
    #[validate(length(min = 1, max = 100))]
    pub username: String,
    pub role: Role,
}

/// A user as returned on the wire.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserResponse {
    pub id: Uuid,
    pub username: String,
    pub role: Role,
    pub created_at: DateTime<Utc>,
}

/// Path parameters for a single user.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct UserPath {
    pub id: Uuid,
}

impl From<User> for UserResponse {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            username: user.username,
            role: user.role,
            created_at: user.created_at,
        }
    }
}

/// Routes for the user resource. Every operation requires authentication.
pub fn router<G, U>() -> ApiRouter<AppState<G, U>>
where
    G: Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    ApiRouter::new()
        .api_route(
            "/users",
            get_with(list_users::<G, U>, list_users_docs)
                .post_with(create_user::<G, U>, create_user_docs),
        )
        .api_route(
            "/users/{id}",
            get_with(get_user::<G, U>, get_user_docs)
                .put_with(update_user::<G, U>, update_user_docs)
                .delete_with(delete_user::<G, U>, delete_user_docs),
        )
}

async fn list_users<G, U>(
    State(state): State<AppState<G, U>>,
    AuthUser(caller): AuthUser,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Json<Page<UserResponse>>, ApiError>
where
    G: Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    // Only admins may list every user; a plain user can only fetch themselves.
    require_admin(&caller)?;
    let total = state.users.count().await?;
    let items = state
        .users
        .list()
        .skip(pagination.offset() as usize)
        .take(pagination.limit() as usize)
        .map(|user| user.map(UserResponse::from).map_err(ApiError::from))
        .try_collect::<Vec<_>>()
        .await?;
    Ok(Json(Page::new(items, &pagination, total)))
}

async fn get_user<G, U>(
    State(state): State<AppState<G, U>>,
    AuthUser(caller): AuthUser,
    Path(path): Path<UserPath>,
) -> Result<Json<UserResponse>, ApiError>
where
    G: Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    // A plain user may only read their own account; admins may read anyone.
    if !caller.role.is_admin() && caller.id != path.id {
        return Err(ApiError::Forbidden);
    }
    let user = state
        .users
        .find_by_id(path.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(user.into()))
}

async fn create_user<G, U>(
    State(state): State<AppState<G, U>>,
    AuthUser(caller): AuthUser,
    Json(body): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserResponse>), ApiError>
where
    G: Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    require_admin(&caller)?;
    body.validate()?;
    let password_hash = hash_password(&body.password).map_err(|error| {
        tracing::error!(%error, "password hashing failed");
        ApiError::Internal
    })?;
    let user = state
        .users
        .create(NewUser {
            username: body.username,
            password_hash,
            role: body.role,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(user.into())))
}

async fn update_user<G, U>(
    State(state): State<AppState<G, U>>,
    AuthUser(caller): AuthUser,
    Path(path): Path<UserPath>,
    Json(body): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>, ApiError>
where
    G: Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    require_admin(&caller)?;
    body.validate()?;
    let user = state
        .users
        .update(
            path.id,
            UserUpdate {
                username: body.username,
                role: body.role,
            },
        )
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(user.into()))
}

async fn delete_user<G, U>(
    State(state): State<AppState<G, U>>,
    AuthUser(caller): AuthUser,
    Path(path): Path<UserPath>,
) -> Result<StatusCode, ApiError>
where
    G: Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    require_admin(&caller)?;
    let target = state
        .users
        .find_by_id(path.id)
        .await?
        .ok_or(ApiError::NotFound)?;

    // An admin may delete plain users only; deleting an admin (or super admin)
    // requires being a super admin.
    if !caller.role.is_super_admin() && target.role.is_admin() {
        return Err(ApiError::Forbidden);
    }

    if state.users.delete(path.id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

fn list_users_docs(op: TransformOperation) -> TransformOperation {
    op.tag("Users")
        .description("List users (paginated). Admin only.")
        .security_requirement("BearerAuth")
        .response::<200, Json<Page<UserResponse>>>()
}

fn get_user_docs(op: TransformOperation) -> TransformOperation {
    op.tag("Users")
        .description("Fetch a user. Admins may fetch anyone; a plain user only themselves.")
        .security_requirement("BearerAuth")
        .response::<200, Json<UserResponse>>()
}

fn create_user_docs(op: TransformOperation) -> TransformOperation {
    op.tag("Users")
        .description("Create a user. Admin only.")
        .security_requirement("BearerAuth")
        .response::<201, Json<UserResponse>>()
}

fn update_user_docs(op: TransformOperation) -> TransformOperation {
    op.tag("Users")
        .description("Update a user. Admin only.")
        .security_requirement("BearerAuth")
        .response::<200, Json<UserResponse>>()
}

fn delete_user_docs(op: TransformOperation) -> TransformOperation {
    op.tag("Users")
        .description(
            "Delete a user. Admins may delete plain users; super admins may delete anyone.",
        )
        .security_requirement("BearerAuth")
        .response::<204, ()>()
}
