use std::future::Future;

use chrono::{DateTime, Utc};
use futures::{Stream, StreamExt};
use sqlx::PgPool;
use uuid::Uuid;
use zyndeck_core::{Role, User};

use crate::{Error, Result};

/// Data needed to create (or upsert) a user.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub username: String,
    pub password_hash: String,
    pub role: Role,
}

/// Fields that can be changed on an existing user.
#[derive(Debug, Clone)]
pub struct UserUpdate {
    pub username: String,
    pub role: Role,
}

/// A user's stored credentials, used to authenticate a login.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Credentials {
    pub id: Uuid,
    pub password_hash: String,
}

/// Persistence operations for [`User`].
///
/// A trait so callers can depend on the abstraction and swap in a `mockall`
/// double in unit tests. Methods return `impl Future + Send` so they stay
/// awaitable inside an `axum` handler; inject with generics, never `dyn`.
#[cfg_attr(feature = "mock", mockall::automock)]
pub trait UserRepository {
    /// Creates a user. Fails with [`Error::UsernameTaken`] if the username
    /// already exists.
    fn create(&self, user: NewUser) -> impl Future<Output = Result<User>> + Send;

    /// Inserts the user, or updates its password hash and role if the username
    /// already exists. Idempotent — used to bootstrap an admin at startup.
    fn upsert_by_username(&self, user: NewUser) -> impl Future<Output = Result<User>> + Send;

    /// Fetches a user by id, or `None` if no such user exists.
    fn find_by_id(&self, id: Uuid) -> impl Future<Output = Result<Option<User>>> + Send;

    /// Fetches a user by username, or `None` if no such user exists.
    fn find_by_username(
        &self,
        username: String,
    ) -> impl Future<Output = Result<Option<User>>> + Send;

    /// Fetches the credentials (id + password hash) for a username, for login.
    fn find_credentials_by_username(
        &self,
        username: String,
    ) -> impl Future<Output = Result<Option<Credentials>>> + Send;

    /// Applies `changes` to the user with that id, returning the updated user,
    /// or `None` if no user has that id. Fails with [`Error::UsernameTaken`] if
    /// the new username collides with another user.
    fn update(
        &self,
        id: Uuid,
        changes: UserUpdate,
    ) -> impl Future<Output = Result<Option<User>>> + Send;

    /// Streams every user, ordered by username, so callers never have to
    /// materialise the whole table at once.
    fn list(&self) -> impl Stream<Item = Result<User>> + Send;

    /// Returns the total number of users (for pagination metadata).
    fn count(&self) -> impl Future<Output = Result<i64>> + Send;

    /// Deletes a user by id, returning `true` if a row was removed.
    fn delete(&self, id: Uuid) -> impl Future<Output = Result<bool>> + Send;
}

/// A single `"user"` row, fallibly mapped to [`User`] (the role text may, in
/// principle, fail to parse).
#[derive(sqlx::FromRow)]
struct UserRow {
    id: Uuid,
    username: String,
    role: String,
    created_at: DateTime<Utc>,
}

impl TryFrom<UserRow> for User {
    type Error = Error;

    fn try_from(row: UserRow) -> Result<Self> {
        let role: Role = row
            .role
            .parse()
            .map_err(|_| Error::InvalidRole(row.role.clone()))?;
        Ok(User {
            id: row.id,
            username: row.username,
            role,
            created_at: row.created_at,
        })
    }
}

/// Postgres-backed [`UserRepository`].
#[derive(Debug, Clone)]
pub struct PgUserRepository {
    pool: PgPool,
}

impl PgUserRepository {
    /// Builds a repository over the given connection pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl UserRepository for PgUserRepository {
    async fn create(&self, user: NewUser) -> Result<User> {
        tracing::debug!(username = %user.username, role = user.role.as_str(), "inserting user");
        let row = sqlx::query_as::<_, UserRow>(include_str!("../queries/user/create.sql"))
            .bind(user.username.as_str())
            .bind(user.password_hash)
            .bind(user.role.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(db) if db.is_unique_violation() => {
                    Error::UsernameTaken(user.username)
                }
                other => Error::Query(other),
            })?;
        row.try_into()
    }

    async fn upsert_by_username(&self, user: NewUser) -> Result<User> {
        tracing::debug!(username = %user.username, role = user.role.as_str(), "upserting user");
        let row = sqlx::query_as::<_, UserRow>(include_str!("../queries/user/upsert.sql"))
            .bind(user.username.as_str())
            .bind(user.password_hash)
            .bind(user.role.as_str())
            .fetch_one(&self.pool)
            .await
            .map_err(Error::Query)?;
        row.try_into()
    }

    async fn find_by_id(&self, id: Uuid) -> Result<Option<User>> {
        tracing::debug!(user_id = %id, "fetching user by id");
        let row = sqlx::query_as::<_, UserRow>(include_str!("../queries/user/find_by_id.sql"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(Error::Query)?;
        row.map(User::try_from).transpose()
    }

    async fn update(&self, id: Uuid, changes: UserUpdate) -> Result<Option<User>> {
        tracing::debug!(user_id = %id, username = %changes.username, role = changes.role.as_str(), "updating user");
        let row = sqlx::query_as::<_, UserRow>(include_str!("../queries/user/update.sql"))
            .bind(changes.username.as_str())
            .bind(changes.role.as_str())
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| match e {
                sqlx::Error::Database(db) if db.is_unique_violation() => {
                    Error::UsernameTaken(changes.username)
                }
                other => Error::Query(other),
            })?;
        row.map(User::try_from).transpose()
    }

    async fn find_by_username(&self, username: String) -> Result<Option<User>> {
        tracing::debug!(%username, "fetching user by username");
        let row =
            sqlx::query_as::<_, UserRow>(include_str!("../queries/user/find_by_username.sql"))
                .bind(username.as_str())
                .fetch_optional(&self.pool)
                .await
                .map_err(Error::Query)?;
        row.map(User::try_from).transpose()
    }

    async fn find_credentials_by_username(&self, username: String) -> Result<Option<Credentials>> {
        tracing::debug!(%username, "fetching credentials by username");
        sqlx::query_as::<_, Credentials>(include_str!(
            "../queries/user/find_credentials_by_username.sql"
        ))
        .bind(username.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(Error::Query)
    }

    fn list(&self) -> impl Stream<Item = Result<User>> + Send {
        tracing::debug!("streaming users");
        sqlx::query_as::<_, UserRow>(include_str!("../queries/user/list.sql"))
            .fetch(&self.pool)
            .map(|row| row.map_err(Error::Query).and_then(User::try_from))
    }

    async fn count(&self) -> Result<i64> {
        tracing::debug!("counting users");
        sqlx::query_scalar(include_str!("../queries/user/count.sql"))
            .fetch_one(&self.pool)
            .await
            .map_err(Error::Query)
    }

    async fn delete(&self, id: Uuid) -> Result<bool> {
        tracing::debug!(user_id = %id, "deleting user");
        let result = sqlx::query(include_str!("../queries/user/delete.sql"))
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(Error::Query)?;
        Ok(result.rows_affected() > 0)
    }
}
