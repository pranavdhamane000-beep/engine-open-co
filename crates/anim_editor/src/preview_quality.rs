//! Hysteretic preview-only quality controller (spec 19.7, 19.8).
//!
//! Starts at scale 1.0 with full preview effects. Downgrades ONE level
//! after eight consecutive frame-times above target; upgrades ONE level
//! only after 120 consecutive frames below 70 percent of target. Allowed
//! scales are exactly 1.0, 0.75 and 0.5. Counters reset after any
//! breakpoint, seek, suspension, resize or device recovery. The state is
//! session-local and NEVER serialized into project or export settings.

use std::time::Duration;

/// Allowed preview render scales (spec 19.7). Ordered best->worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QualityScale {
    Full,
    ThreeQuarter,
    Half,
}

impl QualityScale {
    pub const ALL: [QualityScale; 3] =
        [QualityScale::Full, QualityScale::ThreeQuarter, QualityScale::Half];

    pub fn as_f32(self) -> f32 {
        match self {
            QualityScale::Full => 1.0,
            QualityScale::ThreeQuarter => 0.75,
            QualityScale::Half => 0.5,
        }
    }

    fn one_better(self) -> Option<QualityScale> {
        match self {
            QualityScale::Half => Some(QualityScale::ThreeQuarter),
            QualityScale::ThreeQuarter => Some(QualityScale::Full),
            QualityScale::Full => None,
        }
    }

    fn one_worse(self) -> Option<QualityScale> {
        match self {
            QualityScale::Full => Some(QualityScale::ThreeQuarter),
            QualityScale::ThreeQuarter => Some(QualityScale::Half),
            QualityScale::Half => None,
        }
    }
}

/// Spec 19.8 `PreviewQuality` (the published snapshot).
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewQuality {
    pub scale: QualityScale,
    pub effect_level: u8,
    pub ema_frame_us: f64,
    pub over_budget_frames: u16,
    pub under_budget_frames: u16,
}

/// Live controller (never serialized; session-local).
#[derive(Debug, Clone)]
pub struct PreviewQualityController {
    target_frame_us: f64,
    /// 0..=2 index into QualityScale::ALL.
    level: usize,
    effect_level: u8,
    ema_frame_us: f64,
    consecutive_over: u16,
    consecutive_under: u16,
    /// EMA smoothing factor (locked; not part of the spec surface but
    /// versioned here for determinism of controller behavior).
    ema_alpha: f64,
}

/// Spec constants.
pub const OVER_BUDGET_DOWNGRADE_FRAMES: u16 = 8;
pub const UNDER_BUDGET_UPGRADE_FRAMES: u16 = 120;
pub const UNDER_BUDGET_FRACTION: f64 = 0.70;
pub const EMA_ALPHA: f64 = 0.125;

impl PreviewQualityController {
    pub fn new(target_frame_us: f64) -> Self {
        debug_assert!(target_frame_us > 0.0);
        Self {
            target_frame_us,
            level: 0,
            effect_level: 2, // full preview effects
            ema_frame_us: 0.0,
            consecutive_over: 0,
            consecutive_under: 0,
            ema_alpha: EMA_ALPHA,
        }
    }

    /// One measured frame-time step. Returns the published snapshot.
    /// A single step can move at most one level in EITHER direction, so a
    /// measured burst can never skip a scale.
    pub fn update(&mut self, frame_time: Duration) -> PreviewQuality {
        let us = frame_time.as_secs_f64() * 1_000_000.0;
        self.ema_frame_us = if self.ema_frame_us == 0.0 {
            us
        } else {
            self.ema_alpha * us + (1.0 - self.ema_alpha) * self.ema_frame_us
        };

        if self.ema_frame_us > self.target_frame_us {
            self.consecutive_over = self.consecutive_over.saturating_add(1);
            self.consecutive_under = 0;
            if self.consecutive_over >= OVER_BUDGET_DOWNGRADE_FRAMES {
                if let Some(worse) = self.level_scale().one_worse() {
                    self.level += 1;
                    // Effects drop with scale; each downgrade removes one level.
                    self.effect_level = self.effect_level.saturating_sub(1);
                    let _ = worse;
                }
                self.consecutive_over = 0;
            }
        } else if self.ema_frame_us < self.target_frame_us * UNDER_BUDGET_FRACTION {
            self.consecutive_under = self.consecutive_under.saturating_add(1);
            self.consecutive_over = 0;
            if self.consecutive_under >= UNDER_BUDGET_UPGRADE_FRAMES {
                if let Some(better) = self.level_scale().one_better() {
                    self.level -= 1;
                    // Effects return only up to the initial level.
                    self.effect_level = (self.effect_level + 1).min(2);
                    let _ = better;
                }
                self.consecutive_under = 0;
            }
        } else {
            // Inside the hysteresis band: neither counter grows and both
            // counters reset only on the explicit reset conditions below.
            self.consecutive_over = 0;
            self.consecutive_under = 0;
        }

        self.snapshot()
    }

    /// Reset sample counters midway (breakpoint, seek, suspension, resize).
    /// Level and effects persist; only measurement history clears.
    pub fn reset_counters(&mut self) {
        self.consecutive_over = 0;
        self.consecutive_under = 0;
    }

    /// Full re-init to best quality (device recovery / new session).
    pub fn reset_full(&mut self) {
        self.level = 0;
        self.effect_level = 2;
        self.ema_frame_us = 0.0;
        self.consecutive_over = 0;
        self.consecutive_under = 0;
    }

    fn level_scale(&self) -> QualityScale {
        QualityScale::ALL[self.level]
    }

    pub fn snapshot(&self) -> PreviewQuality {
        PreviewQuality {
            scale: self.level_scale(),
            effect_level: self.effect_level,
            ema_frame_us: self.ema_frame_us,
            over_budget_frames: self.consecutive_over,
            under_budget_frames: self.consecutive_under,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c() -> PreviewQualityController {
        PreviewQualityController::new(16_666.0) // ~60 fps budget
    }

    fn ms(v: f64) -> Duration {
        Duration::from_secs_f64(v / 1000.0)
    }

    #[test]
    fn starts_full() {
        let s = c().snapshot();
        assert_eq!(s.scale, QualityScale::Full);
        assert_eq!(s.effect_level, 2);
        assert_eq!(s.over_budget_frames, 0);
        assert_eq!(s.under_budget_frames, 0);
    }

    #[test]
    fn eight_over_frames_downgrade_one_level() {
        let mut q = c();
        for _ in 0..7 {
            let s = q.update(ms(20.0));
            assert_eq!(s.scale, QualityScale::Full);
        }
        let s = q.update(ms(20.0));
        assert_eq!(s.scale, QualityScale::ThreeQuarter);
        assert_eq!(s.over_budget_frames, 0, "counter resets at breakpoint");
    }

    #[test]
    fn band_between_target_and_target70_never_moves_level() {
        let mut q = c();
        // 15 ms sits under target but above 70% of target: the hysteresis
        // band. Neither counter may ever reach its breakpoint there.
        for i in 0..200 {
            let s = q.update(ms(15.0));
            assert_eq!(s.scale, QualityScale::Full, "i={i}");
            assert_eq!(s.over_budget_frames, 0);
            assert_eq!(s.under_budget_frames, 0, "band resets both counters");
        }
    }

    #[test]
    fn upgrade_needs_120_consecutive_below_70pct() {
        let mut q = c();
        for _ in 0..OVER_BUDGET_DOWNGRADE_FRAMES {
            q.update(ms(30.0)); // Full -> ThreeQuarter
        }
        // Sustained 5 ms feed: the EMA must decay before the consecutive-
        // under streak can start, so the upgrade lands AFTER the raw
        // 120-frame floor. The only reachable intermediate level is
        // ThreeQuarter (single-level steps, no skips).
        let mut up_index = None;
        for i in 1..=300 {
            let s = q.update(ms(5.0));
            if s.scale == QualityScale::Full {
                up_index = Some(i);
                break;
            }
            assert_eq!(s.scale, QualityScale::ThreeQuarter, "i={i}");
        }
        let up_index = up_index.expect("upgrade must occur within 300 frames");
        assert!(
            up_index >= UNDER_BUDGET_UPGRADE_FRAMES as usize,
            "upgrade before 120 under-frames would violate hysteresis (index={up_index})"
        );
    }

    #[test]
    fn half_is_floor_and_reset_counters_works() {
        let mut q = c();
        for _ in 0..8 {
            q.update(ms(30.0));
        } // -> ThreeQuarter
        for _ in 0..8 {
            q.update(ms(30.0));
        } // -> Half
        let s = q.update(ms(30.0));
        assert_eq!(s.scale, QualityScale::Half, "stays at floor");
        q.reset_counters();
        assert_eq!(q.snapshot().over_budget_frames, 0);
        q.reset_full();
        assert_eq!(
            q.snapshot(),
            PreviewQuality {
                scale: QualityScale::Full,
                effect_level: 2,
                ema_frame_us: 0.0,
                over_budget_frames: 0,
                under_budget_frames: 0,
            }
        );
    }
}
