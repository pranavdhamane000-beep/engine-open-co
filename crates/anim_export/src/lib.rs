//! anim_export - Resolved export jobs, FFmpeg, readback coordination, verification and publication (spec 4.2)
//!
//! WP00 scaffold state: compilable empty library root with NO placeholder
//! production behavior (spec 9.3). Behavior lands per work package.
#![deny(clippy::all)]
#![allow(clippy::multiple_crate_versions)] // wgpu/egui ecosystem transitive dup minors; revisit WP11
