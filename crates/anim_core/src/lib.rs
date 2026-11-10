//! WP01 — Canonical rational time and stable identity (spec §10).
//!
//! The one time model for project serialization, keyframes, events, frame
//! ranges, audio samples and simulation scheduling, plus persistent typed
//! IDs. NTSC rates are rationals, never decimal approximations.
//!
//! Error handling contract (spec 10.11): never clamp invalid user data,
//! never silently round a decimal NTSC rate, never panic on arithmetic,
//! never wrap a revision.

pub mod error;
pub mod hash;
pub mod id;
pub mod io;
pub mod rational;
pub mod sim_config;
pub mod time;

pub use error::CoreError;
pub use hash::{domain_hash, domain_hash_hex, Canonical};
pub use id::{
    AssetId, CommandId, EventDefinitionId, EventOccurrenceId, NodeId, ProjectId, Revision,
    SceneId, SpringEdgeId, TrackId,
};
pub use rational::Rational;
pub use sim_config::{EmitterSettings, SimulationSettings, SpringEdge};
pub use time::{FramePos, FrameRange, FrameRate, SampleInterval};
