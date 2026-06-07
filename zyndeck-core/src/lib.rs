//! Zyndeck domain model.
//!
//! Pure domain types — entities and value objects — with no I/O or persistence
//! concerns. Other crates (e.g. `zyndeck-db`) depend on these to map them to and
//! from their own representations.

mod game;
mod language_code;
mod localized_string;
mod role;
mod user;

pub use game::Game;
pub use language_code::{InvalidLanguageCode, LanguageCode};
pub use localized_string::LocalizedString;
pub use role::{ParseRoleError, Role};
pub use user::User;
