//! IO primitives: shared atomic replacement API and its Win32 implementation
//! (spec 10.3 `crates/anim_core/src/io/*`).

pub mod atomic_replace;

#[cfg(windows)]
pub mod windows;

pub use atomic_replace::{write_file_atomically, write_file_atomically_reported};
