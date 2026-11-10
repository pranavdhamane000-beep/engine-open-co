//! anim_sim — deterministic fixed-step simulation, random streams, seeking
//! (spec §14).
//!
//! Everything here is reproducible WITHOUT wall-clock time, platform RNG,
//! cache cadence or random-call history: state is addressed by ABSOLUTE
//! SimTick, draws are address-based (Philox4x32-10), collections are
//! sorted vectors/BTreeMaps, and checkpoint restore replays through the
//! SAME `step_once` the forward path uses.
//!
//! Dependency direction (spec §14.4): anim_sim → anim_core, anim_scene,
//! read-only anim_assets. NO anim_commands, renderer, audio, editor or
//! wall-clock dependency.

pub mod checkpoint;
pub mod clock;
pub mod error;
pub mod events;
pub mod mode;
pub mod particles;
pub mod random;
pub mod seek;
pub mod springs;
pub mod state;
pub mod step;

pub use anim_core::{FrameRate, NodeId, Rational, SimulationSettings};
pub use checkpoint::{capture_checkpoint, restore_checkpoint, Checkpoint, CheckpointCache};
pub use clock::{SimTick, SimulationClock};
pub use error::SimError;
pub use events::{TickSchedule, SIMULATION_INPUT_VERSION};
pub use mode::SimulationMode;
pub use random::{channel, purpose, random_f32, random_u32, StreamKey, ROUNDS};
pub use seek::seek;
pub use state::{
    derive_particle_id, extract_frame, initial_state, Particle, SimState,
    SimulationContext, SimulationFrame, Spring, SpringBody, PARTICLE_HASH_DOMAIN,
    SIM_ALGORITHM_VERSION,
};
pub use step::{advance, prepare_export, step_once};
