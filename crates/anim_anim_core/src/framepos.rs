//! FramePos - frame position type (spec §27 Data Structures).
//!
//! FramePos represents a position in the animation timeline with both
//! frame number and sub-frame offset.

use anim_core::FramePos;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Frame position - whole frame plus sub-frame offset.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FramePos {
    pub frame: u64,
    pub sub_frame: f64,
}

impl FramePos {
    pub fn new(frame: u64, sub_frame: f64) -> Self {
        Self { frame, sub_frame }
    }

    pub fn as_f64(&self) -> f64 {
        self.frame as f64 + self.sub_frame
    }
}

impl From<(u64, f64)> for FramePos {
    fn from((frame, sub_frame): (u64, f64)) -> Self {
        Self { frame, sub_frame }
    }
}

impl std::ops::Add<FramePos> for FramePos {
    type Output = Self;

    fn add(self, rhs: FramePos) -> Self {
        Self {
            frame: self.frame + rhs.frame,
            sub_frame: self.sub_frame + rhs.sub_frame,
        }
    }
}

impl std::ops::AddAssign<FramePos> for FramePos {
    fn add_assign(&mut self, rhs: FramePos) {
        self.frame += rhs.frame;
        self.sub_frame += rhs.sub_frame;
        // Normalize
        while self.sub_frame >= 1.0 {
            self.frame += 1;
            self.sub_frame -= 1.0;
        }
    }
}

impl fmt::Display for FramePos {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "FramePos({} @ {:.3})", self.frame, self.sub_frame)
    }
}