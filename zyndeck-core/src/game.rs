use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::LocalizedString;

/// A deckbuilding game catalogued by Zyndeck.
///
/// A pure domain entity: deliberately **not** `Serialize`/`Deserialize`. The
/// wire format is the concern of the boundary layer, which will map `Game` to
/// and from dedicated request/response wrappers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    /// Stable identifier, assigned by the database on creation.
    pub id: Uuid,
    /// Display name, localised per language (see [`LocalizedString`]).
    pub name: LocalizedString,
    /// When the game was added.
    pub created_at: DateTime<Utc>,
    /// Id of the user who added the game.
    pub created_by: Uuid,
}
