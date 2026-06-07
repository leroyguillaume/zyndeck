//! Database access layer for Zyndeck.
//!
//! Owns the PostgreSQL connection pool and the embedded migrations the rest of
//! the workspace runs against. Rule embeddings live in Postgres via pgvector,
//! so the migrations enable the `vector` extension.

mod config;
mod error;
mod game;
mod user;

pub use config::DbConfig;
pub use error::{Error, Result};
pub use game::{GameRepository, GameUpdate, NewGame, PgGameRepository};
pub use user::{Credentials, NewUser, PgUserRepository, UserRepository, UserUpdate};

#[cfg(feature = "mock")]
pub use game::MockGameRepository;
#[cfg(feature = "mock")]
pub use user::MockUserRepository;

use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;

/// Embedded SQL migrations, applied in order by [`Db::migrate`].
static MIGRATOR: Migrator = sqlx::migrate!("./migrations");

/// Handle to the database: a cheaply-cloneable wrapper around a [`PgPool`].
///
/// The pool is reference-counted, so clone `Db` to share it across tasks rather
/// than opening multiple pools.
#[derive(Debug, Clone)]
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Opens a connection pool from `config` without touching the schema.
    pub async fn connect(config: &DbConfig) -> Result<Self> {
        tracing::debug!(
            max_connections = config.max_connections,
            "connecting to database"
        );
        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .connect(&config.url)
            .await
            .map_err(Error::Connect)?;
        tracing::debug!("database connection pool established");
        Ok(Self::new(pool))
    }

    /// Wraps an existing pool — useful for composition and tests.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Applies any outstanding migrations. Idempotent: migrations already
    /// recorded as applied are skipped.
    pub async fn migrate(&self) -> Result<()> {
        tracing::debug!("running database migrations");
        MIGRATOR.run(&self.pool).await.map_err(Error::Migrate)?;
        tracing::debug!("database migrations up to date");
        Ok(())
    }

    /// Borrows the underlying pool for issuing queries.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Returns a [`GameRepository`] backed by this database's pool.
    pub fn games(&self) -> PgGameRepository {
        PgGameRepository::new(self.pool.clone())
    }

    /// Returns a [`UserRepository`] backed by this database's pool.
    pub fn users(&self) -> PgUserRepository {
        PgUserRepository::new(self.pool.clone())
    }
}
