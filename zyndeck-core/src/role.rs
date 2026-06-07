use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Returned when a string is not a recognised [`Role`].
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid role {0:?}: expected one of super_admin, admin, user")]
pub struct ParseRoleError(pub String);

/// What a [`crate::User`] is allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Full control, including managing other administrators.
    SuperAdmin,
    /// Administrative access.
    Admin,
    /// Regular user.
    User,
}

impl Role {
    /// The wire/storage representation, matching the serde encoding.
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::SuperAdmin => "super_admin",
            Role::Admin => "admin",
            Role::User => "user",
        }
    }

    /// Whether this role has administrative privileges (admin or super admin).
    pub fn is_admin(&self) -> bool {
        matches!(self, Role::SuperAdmin | Role::Admin)
    }

    /// Whether this role is the super administrator.
    pub fn is_super_admin(&self) -> bool {
        matches!(self, Role::SuperAdmin)
    }
}

impl FromStr for Role {
    type Err = ParseRoleError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "super_admin" => Ok(Role::SuperAdmin),
            "admin" => Ok(Role::Admin),
            "user" => Ok(Role::User),
            other => Err(ParseRoleError(other.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Role; 3] = [Role::SuperAdmin, Role::Admin, Role::User];

    #[test]
    fn as_str_round_trips_through_from_str() {
        for role in ALL {
            assert_eq!(Role::from_str(role.as_str()), Ok(role));
        }
    }

    #[test]
    fn as_str_matches_the_serde_encoding() {
        for role in ALL {
            assert_eq!(
                serde_json::to_value(role).unwrap(),
                serde_json::json!(role.as_str())
            );
        }
    }

    #[test]
    fn from_str_rejects_unknown_roles() {
        assert_eq!(
            Role::from_str("root"),
            Err(ParseRoleError("root".to_owned()))
        );
    }

    #[test]
    fn admin_predicates() {
        assert!(Role::SuperAdmin.is_admin());
        assert!(Role::Admin.is_admin());
        assert!(!Role::User.is_admin());

        assert!(Role::SuperAdmin.is_super_admin());
        assert!(!Role::Admin.is_super_admin());
        assert!(!Role::User.is_super_admin());
    }
}
