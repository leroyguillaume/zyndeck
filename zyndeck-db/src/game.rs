use std::future::Future;

use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;
use zyndeck_core::{Game, LocalizedString};

use crate::{Error, Result};

/// Data needed to create a game.
#[derive(Debug, Clone)]
pub struct NewGame {
    pub name: LocalizedString,
    pub created_by: Uuid,
}

/// Fields that can be changed on an existing game.
#[derive(Debug, Clone)]
pub struct GameUpdate {
    pub name: LocalizedString,
}

/// Persistence operations for [`Game`].
///
/// A trait so callers can depend on the abstraction and swap in a `mockall`
/// double in unit tests. Methods return `impl Future + Send` (not bare
/// `async fn`) so the futures stay `Send` when awaited inside an `axum` handler;
/// the trait is therefore not object-safe — inject it with generics, never
/// `dyn`.
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait GameRepository {
    /// Inserts a game, returning it with its database-assigned id and creation
    /// timestamp.
    fn create(&self, game: NewGame) -> impl Future<Output = Result<Game>> + Send;

    /// Fetches a game by id, or `None` if no such game exists.
    fn find_by_id(&self, id: Uuid) -> impl Future<Output = Result<Option<Game>>> + Send;

    /// Streams every game, ordered by creation time, so callers never have to
    /// materialise the whole table at once.
    fn list(&self) -> impl Stream<Item = Result<Game>> + Send;

    /// Returns the total number of games (for pagination metadata).
    fn count(&self) -> impl Future<Output = Result<i64>> + Send;

    /// Applies `changes` to the game with that id, returning the updated game,
    /// or `None` if no game has that id.
    fn update(
        &self,
        id: Uuid,
        changes: GameUpdate,
    ) -> impl Future<Output = Result<Option<Game>>> + Send;

    /// Deletes a game by id, returning `true` if a row was removed.
    fn delete(&self, id: Uuid) -> impl Future<Output = Result<bool>> + Send;
}

/// A single `game` row, mapped to [`Game`] via [`From`].
#[derive(sqlx::FromRow)]
struct GameRow {
    id: Uuid,
    name: Json<LocalizedString>,
    created_at: DateTime<Utc>,
    created_by: Uuid,
}

impl From<GameRow> for Game {
    fn from(row: GameRow) -> Self {
        Game {
            id: row.id,
            name: row.name.0,
            created_at: row.created_at,
            created_by: row.created_by,
        }
    }
}

/// Postgres-backed [`GameRepository`].
#[derive(Debug, Clone)]
pub struct PgGameRepository {
    pool: PgPool,
}

impl PgGameRepository {
    /// Builds a repository over the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl GameRepository for PgGameRepository {
    async fn create(&self, game: NewGame) -> Result<Game> {
        tracing::debug!(created_by = %game.created_by, "inserting game");
        let row = sqlx::query_as::<_, GameRow>(include_str!("../queries/game/create.sql"))
            .bind(Json(game.name))
            .bind(game.created_by)
            .fetch_one(&self.pool)
            .await
            .map_err(Error::Query)?;
        tracing::debug!(game_id = %row.id, "game inserted");
        Ok(row.into())
    }

    async fn find_by_id(&self, id: Uuid) -> Result<Option<Game>> {
        tracing::debug!(game_id = %id, "fetching game by id");
        let row = sqlx::query_as::<_, GameRow>(include_str!("../queries/game/find_by_id.sql"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::Query)?;
        Ok(row.map(Game::from))
    }

    fn list(&self) -> impl Stream<Item = Result<Game>> + Send {
        tracing::debug!("streaming games");
        sqlx::query_as::<_, GameRow>(include_str!("../queries/game/list.sql"))
            .fetch(&self.pool)
            .map(|row| row.map_err(Error::Query).map(Game::from))
    }

    async fn count(&self) -> Result<i64> {
        tracing::debug!("counting games");
        sqlx::query_scalar(include_str!("../queries/game/count.sql"))
            .fetch_one(&self.pool)
            .await
            .map_err(Error::Query)
    }

    async fn update(&self, id: Uuid, changes: GameUpdate) -> Result<Option<Game>> {
        tracing::debug!(game_id = %id, "updating game");
        let row = sqlx::query_as::<_, GameRow>(include_str!("../queries/game/update.sql"))
            .bind(Json(changes.name))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::Query)?;
        Ok(row.map(Game::from))
    }

    async fn delete(&self, id: Uuid) -> Result<bool> {
        tracing::debug!(game_id = %id, "deleting game");
        let result = sqlx::query(include_str!("../queries/game/delete.sql"))
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(Error::Query)?;
        Ok(result.rows_affected() > 0)
    }
}
