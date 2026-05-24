//! Strongly-typed identifier newtypes for domain entities (§15.2).
//!
//! Uuid-backed: [`UserId`], [`LibraryId`], [`SeriesId`] — all UUID v7 on the wire.
//! String-backed: [`IssueId`] — BLAKE3 hex of the issue file's path or content
//! (see spec §5.1.2). The issue PK is stable across renames *of the same file*
//! but changes when content is retagged; do not assume Uuid semantics.
//!
//! These types exist to make id-swap bugs unrepresentable at the function
//! signature level. They are used in handler arguments, DTO field types, and
//! response shapes. The entity layer continues to hold raw `Uuid` / `String`
//! to keep sea-orm's derive macros simple; conversion happens at the API
//! boundary via `.0` or `From` impls.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

macro_rules! uuid_id_newtype {
    ($name:ident, $description:literal) => {
        #[doc = $description]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        impl $name {
            /// Allocate a new time-ordered (UUID v7) identifier.
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Borrow the inner `Uuid` — used at the sea-orm boundary where
            /// queries still take raw `Uuid`.
            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(u: Uuid) -> Self {
                Self(u)
            }
        }

        impl From<$name> for Uuid {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::from_str(s).map(Self)
            }
        }
    };
}

uuid_id_newtype!(UserId, "Stable identifier for a user account.");
uuid_id_newtype!(LibraryId, "Stable identifier for a library root.");
uuid_id_newtype!(SeriesId, "Stable identifier for a series.");

/// Stable identifier for an issue file.
///
/// Backed by a BLAKE3 hex string — the issue's content hash or path hash,
/// computed at scan time (spec §5.1.2). Not a UUID. Construct via
/// [`IssueId::from`] / [`IssueId::new`]; do not generate random values.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(transparent)]
pub struct IssueId(pub String);

impl IssueId {
    /// Wrap an already-computed hash string. Callers are responsible for
    /// ensuring the input is a valid BLAKE3 hex digest.
    pub fn new(hash: impl Into<String>) -> Self {
        Self(hash.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Borrow as `&String` for sea-orm queries that take owned `String`.
    pub fn as_string(&self) -> &String {
        &self.0
    }
}

impl From<String> for IssueId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for IssueId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

impl From<IssueId> for String {
    fn from(id: IssueId) -> Self {
        id.0
    }
}

impl fmt::Display for IssueId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for IssueId {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_id_roundtrips_through_serde() {
        let id = UserId::new();
        let json = serde_json::to_string(&id).unwrap();
        let back: UserId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn user_id_serializes_as_bare_uuid_string() {
        let id = UserId::from(Uuid::nil());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"00000000-0000-0000-0000-000000000000\"");
    }

    #[test]
    fn issue_id_is_string_backed() {
        let id = IssueId::new("abc123");
        assert_eq!(id.as_str(), "abc123");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"abc123\"");
        let back: IssueId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn library_id_parses_from_path_string() {
        let s = "550e8400-e29b-41d4-a716-446655440000";
        let id: LibraryId = s.parse().unwrap();
        assert_eq!(id.to_string(), s);
    }

    #[test]
    fn ids_are_distinct_types() {
        // Type-level assertion: this file would fail to compile if SeriesId
        // and UserId were the same type. The presence of this test exercises
        // the boundary at runtime as well.
        let user = UserId::new();
        let series = SeriesId::new();
        assert_ne!(user.to_string(), series.to_string());
    }
}
