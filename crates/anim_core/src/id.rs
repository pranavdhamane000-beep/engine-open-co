//! Typed UUID wrappers and Revision (spec 10.5, 10.8, 10.9).
//!
//! IDs are generated once at creation and copied through save/load, undo and
//! evaluation; they are never re-derived. Type wrappers prevent mixing up a
//! NodeId with a TrackId even though both wrap a UUID.

use crate::error::CoreError;
use crate::hash::Canonical;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;
use std::str::FromStr;
use uuid::Uuid;

macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident, $domain:literal) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new_v4() -> Self {
                Self(Uuid::new_v4())
            }

            pub fn from_u128(v: u128) -> Self {
                Self(Uuid::from_u128(v))
            }
        }

        impl fmt::Display for $name {
            /// Bare UUID form — the same text `FromStr` accepts, so
            /// `id.to_string().parse()` round trips.
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = CoreError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Uuid::from_str(s)
                    .map(Self)
                    .map_err(|_| CoreError::IdentifierParse(format!("{}: {s}", $domain)))
            }
        }

        impl Canonical for $name {
            /// Stable byte ordering: the UUID's 16 canonical bytes.
            fn append_canonical(&self, out: &mut Vec<u8>) {
                crate::hash::append_uuid(out, &self.0);
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                self.0.serialize(s)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                Ok(Self(Uuid::deserialize(d)?))
            }
        }
    };
}

typed_id!(
    /// A whole project document.
    ProjectId, "project"
);
typed_id!(
    /// A scene within a project's timeline graph.
    SceneId, "scene"
);
typed_id!(
    /// A node in the scene graph.
    NodeId, "node"
);
typed_id!(
    /// A track attached to a node (or the scene root).
    TrackId, "track"
);
typed_id!(
    /// A reference to an immutable asset (WP02 manifest).
    AssetId, "asset"
);
typed_id!(
    /// A command in the undo history (WP04).
    CommandId, "command"
);
typed_id!(
    /// A named event definition (WP05 scene events).
    EventDefinitionId, "eventdef"
);

/// Document revision counter. Zero is valid for a new document; wrapping is
/// never allowed (spec 10.11) — overflow is an explicit error.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Revision(pub u64);

impl Revision {
    pub const FIRST: Revision = Revision(0);

    pub fn as_u64(self) -> u64 {
        self.0
    }

    /// Checked increment by `n`; u64 overflow returns an error, never wraps.
    pub fn checked_add(self, n: u64) -> Result<Self, CoreError> {
        self.0.checked_add(n).map(Revision).ok_or(CoreError::ArithmeticOverflow)
    }
}

impl Canonical for Revision {
    fn append_canonical(&self, out: &mut Vec<u8>) {
        crate::hash::append_u64(out, self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_overflow_is_an_error_not_a_wrap() {
        let r = Revision(u64::MAX);
        assert!(r.checked_add(1).is_err());
        assert!(r.checked_add(0).is_ok());
        assert_eq!(Revision::FIRST.checked_add(3).unwrap().as_u64(), 3);
    }

    #[test]
    fn parse_round_trip_and_rejection() {
        let id = NodeId::new_v4();
        let parsed: NodeId = id.to_string().parse().unwrap();
        assert_eq!(id, parsed);
        assert!("not-a-uuid".parse::<NodeId>().is_err());
    }

    #[test]
    fn ids_survive_serde_round_trip() {
        let id = ProjectId::new_v4();
        let json = serde_json::to_string(&id).unwrap();
        let back: ProjectId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn canonical_bytes_are_stable_across_instances() {
        let a = TrackId::from_u128(42);
        let b = TrackId::from_u128(42);
        assert_eq!(a.canonical_bytes(), b.canonical_bytes());
        assert_eq!(a.canonical_bytes().len(), 16);
    }
}
