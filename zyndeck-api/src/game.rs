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
use zyndeck_core::{Game, LocalizedString};
use zyndeck_db::{GameRepository, GameUpdate, NewGame, UserRepository};

use crate::auth::{AuthUser, require_admin};
use crate::error::ApiError;
use crate::pagination::{Page, PaginationQuery};
use crate::state::AppState;

/// Request body to create a game.
#[derive(Debug, Deserialize, JsonSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct CreateGameRequest {
    #[validate(custom(function = validate_name))]
    pub name: LocalizedString,
}

/// Request body to update a game.
#[derive(Debug, Deserialize, JsonSchema, Validate)]
#[serde(rename_all = "camelCase")]
pub struct UpdateGameRequest {
    #[validate(custom(function = validate_name))]
    pub name: LocalizedString,
}

/// A game as returned on the wire.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GameResponse {
    pub id: Uuid,
    pub name: LocalizedString,
    pub created_at: DateTime<Utc>,
    pub creator_id: Uuid,
}

/// Path parameters for a single game.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GamePath {
    pub id: Uuid,
}

impl From<Game> for GameResponse {
    fn from(game: Game) -> Self {
        Self {
            id: game.id,
            name: game.name,
            created_at: game.created_at,
            creator_id: game.created_by,
        }
    }
}

fn validate_name(name: &LocalizedString) -> Result<(), validator::ValidationError> {
    if name.is_empty() {
        Err(validator::ValidationError::new("empty_localized_name"))
    } else {
        Ok(())
    }
}

/// Routes for the game resource. Reads are public; writes require an admin.
pub fn router<G, U>() -> ApiRouter<AppState<G, U>>
where
    G: GameRepository + Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    ApiRouter::new()
        .api_route(
            "/games",
            get_with(list_games::<G, U>, list_games_docs)
                .post_with(create_game::<G, U>, create_game_docs),
        )
        .api_route(
            "/games/{id}",
            get_with(get_game::<G, U>, get_game_docs)
                .put_with(update_game::<G, U>, update_game_docs)
                .delete_with(delete_game::<G, U>, delete_game_docs),
        )
}

async fn list_games<G, U>(
    State(state): State<AppState<G, U>>,
    Query(pagination): Query<PaginationQuery>,
) -> Result<Json<Page<GameResponse>>, ApiError>
where
    G: GameRepository + Send + Sync + 'static,
    U: Send + Sync + 'static,
{
    let total = state.games.count().await?;
    let items = state
        .games
        .list()
        .skip(pagination.offset() as usize)
        .take(pagination.limit() as usize)
        .map(|game| game.map(GameResponse::from).map_err(ApiError::from))
        .try_collect::<Vec<_>>()
        .await?;
    Ok(Json(Page::new(items, &pagination, total)))
}

async fn get_game<G, U>(
    State(state): State<AppState<G, U>>,
    Path(path): Path<GamePath>,
) -> Result<Json<GameResponse>, ApiError>
where
    G: GameRepository + Send + Sync + 'static,
    U: Send + Sync + 'static,
{
    let game = state
        .games
        .find_by_id(path.id)
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(game.into()))
}

async fn create_game<G, U>(
    State(state): State<AppState<G, U>>,
    AuthUser(caller): AuthUser,
    Json(body): Json<CreateGameRequest>,
) -> Result<(StatusCode, Json<GameResponse>), ApiError>
where
    G: GameRepository + Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    require_admin(&caller)?;
    body.validate()?;
    let game = state
        .games
        .create(NewGame {
            name: body.name,
            created_by: caller.id,
        })
        .await?;
    Ok((StatusCode::CREATED, Json(game.into())))
}

async fn update_game<G, U>(
    State(state): State<AppState<G, U>>,
    AuthUser(caller): AuthUser,
    Path(path): Path<GamePath>,
    Json(body): Json<UpdateGameRequest>,
) -> Result<Json<GameResponse>, ApiError>
where
    G: GameRepository + Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    require_admin(&caller)?;
    body.validate()?;
    let game = state
        .games
        .update(path.id, GameUpdate { name: body.name })
        .await?
        .ok_or(ApiError::NotFound)?;
    Ok(Json(game.into()))
}

async fn delete_game<G, U>(
    State(state): State<AppState<G, U>>,
    AuthUser(caller): AuthUser,
    Path(path): Path<GamePath>,
) -> Result<StatusCode, ApiError>
where
    G: GameRepository + Send + Sync + 'static,
    U: UserRepository + Send + Sync + 'static,
{
    require_admin(&caller)?;
    if state.games.delete(path.id).await? {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::NotFound)
    }
}

fn list_games_docs(op: TransformOperation) -> TransformOperation {
    op.tag("Games")
        .description("List games (paginated). Public.")
        .response::<200, Json<Page<GameResponse>>>()
}

fn get_game_docs(op: TransformOperation) -> TransformOperation {
    op.tag("Games")
        .description("Fetch a single game. Public.")
        .response::<200, Json<GameResponse>>()
}

fn create_game_docs(op: TransformOperation) -> TransformOperation {
    op.tag("Games")
        .description("Create a game. Admin only.")
        .security_requirement("BearerAuth")
        .response::<201, Json<GameResponse>>()
}

fn update_game_docs(op: TransformOperation) -> TransformOperation {
    op.tag("Games")
        .description("Update a game's name. Admin only.")
        .security_requirement("BearerAuth")
        .response::<200, Json<GameResponse>>()
}

fn delete_game_docs(op: TransformOperation) -> TransformOperation {
    op.tag("Games")
        .description("Delete a game. Admin only.")
        .security_requirement("BearerAuth")
        .response::<204, ()>()
}
