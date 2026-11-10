//! Simulation configuration shared by the authoritative document
//! (`anim_project`) and the simulator (`anim_sim`) (spec 14.2/14.8).
//!
//! WHY THIS LIVES IN anim_core: spec 14.4 pins the dependency set of
//! `anim_sim` to {anim_core, anim_scene, read-only anim_assets} — the
//! settings type must be reachable by BOTH the document crate and the
//! simulator, and the only substrate that neither violates is anim_core.
//! These are plain validated data; no solver behavior lives here.

use crate::error::CoreError;
use crate::NodeId;
use serde::{Deserialize, Serialize};

/// One stochastic emitter on a scene node (WP05 MVP authoring surface).
///
/// Spawn scheduling: deterministic "expected count" adoption per tick —
/// exact_emitted_index advances in exact rational steps of rate*dt and the
/// integer part drives counts; the Philox rate-decision draw adopts the
/// fractional carry (`anim_sim` owns that mechanic; this struct only
/// carries validated data).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EmitterSettings {
    /// Particles per second (finite, ≥ 0). 0 = never spawns from rate.
    pub spawn_rate: f64,
    /// Initial speed magnitude in units/second (finite, ≥ 0).
    pub initial_speed: f64,
    /// Lifetime in exact simulation ticks (≥ 1).
    pub lifetime_ticks: u64,
    /// Base particle size in units (finite, > 0).
    pub size: f64,
}

impl EmitterSettings {
    pub fn validate(&self) -> Result<(), CoreError> {
        if !self.spawn_rate.is_finite() || self.spawn_rate < 0.0 {
            return Err(CoreError::Validation(
                "emitter: spawn_rate must be finite and nonnegative".into(),
            ));
        }
        if !self.initial_speed.is_finite() || self.initial_speed < 0.0 {
            return Err(CoreError::Validation(
                "emitter: initial_speed must be finite and nonnegative".into(),
            ));
        }
        if self.lifetime_ticks == 0 {
            return Err(CoreError::Validation(
                "emitter: lifetime_ticks must be at least 1".into(),
            ));
        }
        if !self.size.is_finite() || self.size <= 0.0 {
            return Err(CoreError::Validation(
                "emitter: size must be finite and positive".into(),
            ));
        }
        Ok(())
    }
}

/// One authored spring edge between two scene nodes (spec 14.8 springs).
///   a ←→ b rest at `rest_length` units with `stiffness ≥ 0`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SpringEdge {
    /// Endpoint A (must exist in the owning scene).
    pub a: NodeId,
    /// Endpoint B (must exist in the owning scene; MUST differ from `a`).
    pub b: NodeId,
    /// Rest length in units (finite, > 0).
    pub rest_length: f64,
    /// Stiffness coefficient (finite, ≥ 0). 0 = inert edge.
    pub stiffness: f64,
}

impl SpringEdge {
    pub fn validate(&self) -> Result<(), CoreError> {
        if !self.rest_length.is_finite() || self.rest_length <= 0.0 {
            return Err(CoreError::Validation(
                "spring edge: rest_length must be finite and positive".into(),
            ));
        }
        if !self.stiffness.is_finite() || self.stiffness < 0.0 {
            return Err(CoreError::Validation(
                "spring edge: stiffness must be finite and nonnegative".into(),
            ));
        }
        Ok(())
    }
}

/// Complete simulation settings (WP05). The document carries ONE instance.
/// Invariants (spec 14.2): ticks_per_frame is a NONZERO u32; everything
/// float is finite; solver iterations ≥ 1 (a 0-iteration spring solver is
/// an invalid count, not an optimization).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SimulationSettings {
    /// Fixed ticks per animation frame (spec 14.2: nonzero; rate =
    /// frame_rate * ticks_per_frame).
    pub ticks_per_frame: u32,
    /// Project seed for all Philox stream derivations (never silently
    /// substituted — a missing seed is a validation error upstream).
    pub project_seed: [u8; 32],
    /// World gravity in units/second² (finite components; [0,0,0] fine).
    pub gravity: [f64; 3],
    /// Fixed Gauss-Seidel iterations per tick for the spring solver
    /// (never changes with cache cadence; ≥ 1).
    pub solver_iterations: u16,
}

impl Default for SimulationSettings {
    fn default() -> Self {
        // ticks_per_frame = 1 keeps DEFAULT documents validation-valid
        // (nonzero invariant); seeded docs are authored, not defaulted.
        Self {
            ticks_per_frame: 1,
            project_seed: [0u8; 32],
            gravity: [0.0; 3],
            solver_iterations: 8,
        }
    }
}

impl SimulationSettings {
    pub fn validate(&self) -> Result<(), CoreError> {
        if self.ticks_per_frame == 0 {
            return Err(CoreError::Validation(
                "simulation: ticks_per_frame must be nonzero".into(),
            ));
        }
        if !self.gravity.iter().all(|g| g.is_finite()) {
            return Err(CoreError::Validation(
                "simulation: gravity components must be finite".into(),
            ));
        }
        if self.solver_iterations == 0 {
            return Err(CoreError::Validation(
                "simulation: solver_iterations must be at least 1".into(),
            ));
        }
        Ok(())
    }
}
