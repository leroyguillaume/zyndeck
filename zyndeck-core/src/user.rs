use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::Role;

/// A Zyndeck user account.
///
/// A pure domain entity: deliberately **not** `Serialize`/`Deserialize` (like
/// [`crate::Game`]); the boundary layer maps it to dedicated wrappers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    /// Stable identifier, assigned by the database on creation.
    pub id: Uuid,
    /// Unique login name.
    pub username: String,
    /// What the user is allowed to do.
    pub role: Role,
    /// When the account was created.
    pub created_at: DateTime<Utc>,
}
