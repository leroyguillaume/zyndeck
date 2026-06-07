use thiserror::Error;

/// Errors raised by the database layer.
#[derive(Debug, Error)]
pub enum Error {
    /// Opening the connection pool failed.
    #[error("failed to connect to the database")]
    Connect(#[source] sqlx::Error),

    /// Applying migrations failed.
    #[error("failed to run database migrations")]
    Migrate(#[source] sqlx::migrate::MigrateError),

    /// A query against the database failed.
    #[error("database query failed")]
    Query(#[source] sqlx::Error),

    /// Attempted to create a user whose username already exists.
    #[error("username {0:?} is already taken")]
    UsernameTaken(String),

    /// A role value stored in the database is not a recognised role.
    #[error("invalid role {0:?} stored in the database")]
    InvalidRole(String),
}

/// Convenience alias for fallible operations in this crate.
pub type Result<T> = std::result::Result<T, Error>;
