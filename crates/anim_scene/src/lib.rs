//! WP04 — evaluated scene snapshots and canonical events (spec §13).
//!
//! `anim_scene` transforms authoritative project data into immutable,
//! revision-bound evaluated snapshots. Evaluation is STATELESS: the same
//! (`ProjectDocument` at a revision, scene, `FramePos`) triple always
//! produces the same canonical snapshot bytes and the same event log —
//! G3 holds across processes (see `tests/evaluation.rs` re-exec proof).
//!
//! Dependency direction (spec §13.4):
//!   anim_scene → anim_project → anim_assets → anim_core.
//! Consumers (anim_sim/anim_render/anim_audio/editor/export) read
//! snapshots read-only; this crate depends on NONE of them.

pub mod animation;
pub mod cache;
pub mod camera;
pub mod error;
pub mod evaluator;
pub mod events;
pub mod hierarchy;
pub mod properties;
pub mod snapshot;

pub use anim_core::{
    AssetId, EventOccurrenceId, FramePos, NodeId, Rational, Revision, SceneId,
};
pub use anim_project::{EventDefinition, EventKind};
pub use cache::SceneCache;
pub use error::SceneError;
pub use evaluator::{events_for_frame, SceneEvaluator};
pub use snapshot::{
    draw_order, events_hash, snapshot_hash, AudioCue, EvaluatedCamera, EvaluatedNode,
    SceneEvent, SceneSnapshot, SimulationInput,
};
