use std::sync::Arc;

use jsonwebtoken::{DecodingKey, EncodingKey};
use zyndeck_db::{Db, PgGameRepository, PgUserRepository};

/// Shared application state handed to every handler.
///
/// Generic over the repository implementations so production wires in the
/// Postgres-backed ones while tests inject `mockall` doubles. Repositories are
/// held behind `Arc` so the state stays cheaply cloneable without requiring the
/// repositories themselves to be `Clone`.
pub struct AppState<G, U> {
    pub games: Arc<G>,
    pub users: Arc<U>,
    /// Signs JWTs issued by the login endpoint.
    pub encoding_key: EncodingKey,
    /// Verifies JWTs presented by callers.
    pub decoding_key: DecodingKey,
    /// Lifetime, in seconds, of tokens issued by the login endpoint.
    pub token_ttl_seconds: u64,
}

// Manual `Clone` so it does not require `G: Clone` / `U: Clone` (the `Arc`s are
// what gets cloned).
impl<G, U> Clone for AppState<G, U> {
    fn clone(&self) -> Self {
        Self {
            games: Arc::clone(&self.games),
            users: Arc::clone(&self.users),
            encoding_key: self.encoding_key.clone(),
            decoding_key: self.decoding_key.clone(),
            token_ttl_seconds: self.token_ttl_seconds,
        }
    }
}

impl<G, U> AppState<G, U> {
    /// Builds the state from explicit repositories, the JWT signing secret and
    /// the lifetime (in seconds) of issued tokens.
    pub fn with_repositories(games: G, users: U, jwt_secret: &str, token_ttl_seconds: u64) -> Self {
        Self {
            games: Arc::new(games),
            users: Arc::new(users),
            encoding_key: EncodingKey::from_secret(jwt_secret.as_bytes()),
            decoding_key: DecodingKey::from_secret(jwt_secret.as_bytes()),
            token_ttl_seconds,
        }
    }
}

impl AppState<PgGameRepository, PgUserRepository> {
    /// Builds the production state, taking the repositories from the database.
    pub fn new(db: Db, jwt_secret: &str, token_ttl_seconds: u64) -> Self {
        Self::with_repositories(db.games(), db.users(), jwt_secret, token_ttl_seconds)
    }
}
