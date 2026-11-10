//! anim_test_support - Scene builders, deterministic runners, golden comparison and fault injection (spec 4.2)
//!
//! WP00 scaffold state: compilable empty library root with NO placeholder
//! production behavior (spec 9.3). Behavior lands per work package.
#![deny(clippy::all)]
#![allow(clippy::multiple_crate_versions)] // wgpu/egui ecosystem transitive dup minors; revisit WP11

pub mod benchmark;
pub mod determinism;
pub mod faults;
pub mod golden;
pub mod gpu_memory;
pub mod probe;
pub mod scenes;

pub use determinism::{RunArtifacts, RunConfig, TestError};
pub use scenes::{
    build, fixed_id, manifest_json, test_a_full_body, SceneBundle, SceneManifest,
    TestSceneId,
};
