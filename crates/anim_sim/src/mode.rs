//! Simulation execution mode (spec §14.10). Policy ONLY — never changes
/// solver numerics, only checkpointing and export behavior.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SimulationMode {
    /// Normal forward tick; captures checkpoints per cache cadence.
    Live,
    /// Seeking/replaying from a restored checkpoint; captures checkpoints
    /// per cache cadence (allows overwrite/extend).
    Replay,
    /// Warm-up phase: runs `step_once` WITHOUT capturing checkpoints.
    /// Used to advance to a seek target before the "real" run.
    WarmUp,
    /// Final export run: identical to Live but guarantees checkpoint
    /// capture at EVERY frame boundary (for frame-perfect scrubbing).
    Export,
}

impl SimulationMode {
    /// Does this mode capture checkpoints into the cache?
    pub fn captures_checkpoints(self) -> bool {
        matches!(
            self,
            SimulationMode::Live | SimulationMode::Replay | SimulationMode::Export
        )
    }

    /// Does this mode force capture at frame boundaries (for Export)?
    pub fn forces_frame_capture(self) -> bool {
        matches!(self, SimulationMode::Export)
    }
}
