//! SimTick and the fixed-rate clock (spec §14.6/14.8).
//!
//! * `SimTick(0)` is the initialized state BEFORE any step.
//! * Whole frame f begins at tick f * ticks_per_frame (CHECKED).
//! * The step duration is computed ONCE from the exact configuration
//!   (1 / (fps·N)) and converted to the working type f32; mutable seconds
//!   are NEVER accumulated — ages are integer tick counts.
//! * rate = frame_rate × ticks_per_frame, an exact FrameRate (rational).

use crate::error::{SimError, SimResult};
use anim_core::{FramePos, FrameRate, Rational};

/// Absolute simulation tick (signed for timeline arithmetic; stateful
/// replay supports ticks at or above zero — §14.7 "SimTick(0) is the
/// initialized state").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SimTick(pub i64);

impl SimTick {
    pub const ZERO: SimTick = SimTick(0);
}

/// Fixed-rate simulation clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimulationClock {
    /// Canonical project frame rate (exact rational).
    pub frame_rate: FrameRate,
    /// Nonzero fixed ticks per animation frame.
    pub ticks_per_frame: u32,
    /// rate = frame_rate × ticks_per_frame (exact, positive).
    pub rate: FrameRate,
}

impl SimulationClock {
    /// Validate-and-construct: rejects zero ticks_per_frame (spec §14.2)
    /// and carries ONLY exact arithmetic — no wall clock anywhere.
    pub fn new(frame_rate: FrameRate, ticks_per_frame: u32) -> SimResult<Self> {
        if ticks_per_frame == 0 {
            return Err(SimError::InvalidRate(format!(
                "ticks_per_frame {ticks_per_frame} must be nonzero"
            )));
        }
        let rate = FrameRate::new(
            frame_rate.num.checked_mul(ticks_per_frame).ok_or_else(|| {
                SimError::InvalidRate(format!(
                    "rate {0}×{ticks_per_frame} overflows",
                    frame_rate.num
                ))
            })?,
            frame_rate.den,
        )
        .map_err(|e| SimError::InvalidRate(e.to_string()))?;
        Ok(Self { frame_rate, ticks_per_frame, rate })
    }

    /// Whole frame f → its first tick (checked f·N; spec §14.6).
    pub fn frame_to_tick(&self, frame: i64) -> SimResult<SimTick> {
        let n = self.ticks_per_frame as i64;
        frame
            .checked_mul(n)
            .map(SimTick)
            .ok_or_else(|| SimError::TickOverflow(format!("frame {frame} × {n} ticks")))
    }

    /// Tick → owning frame: floor division (negative ticks floor DOWN,
    /// matching exact rational ownership [time_of(f), time_of(f+1))).
    pub fn tick_to_frame(&self, tick: SimTick) -> i64 {
        let n = self.ticks_per_frame as i64;
        if n == 1 {
            return tick.0;
        }
        // floor(t / n) via division correction (tick.0.min i64 safe; n>1)
        let q = tick.0 / n;
        let r = tick.0 % n;
        if r < 0 {
            q - 1
        } else {
            q
        }
    }

    /// EXACT rational time of tick t (seconds): t / rate = t·den/num.
    /// Exact even for negative ticks (timeline arithmetic is signed).
    pub fn time_of_tick(&self, tick: SimTick) -> SimResult<Rational> {
        let n = num_bigint::BigInt::from(tick.0) * num_bigint::BigInt::from(self.rate.den);
        Rational::from_bigints(n, num_bigint::BigInt::from(self.rate.num))
            .map_err(|_| SimError::TickOverflow(format!("tick {tick:?} rational time")))
    }

    /// The step duration in f32 — computed ONCE from exact configuration
    /// (1 / (fps·N)); it never mutates and never accumulates (§14.6).
    pub fn step_duration(&self) -> f32 {
        1.0_f32 / ((self.rate.num as f32) / (self.rate.den as f32))
    }

    /// The whole-frame position of frame f at its start (ties to WP04
    /// evaluation; frame f owns ticks [f·N, (f+1)·N)).
    pub fn frame_start_position(&self, frame: i64) -> SimResult<FramePos> {
        FramePos::new(frame, Rational::zero())
            .map_err(|e| SimError::TickOverflow(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_frame_conversion_is_exact_and_checked() {
        let clock = SimulationClock::new(FrameRate::new(24000, 1001).unwrap(), 5).unwrap();
        assert_eq!(clock.rate, FrameRate::new(120000, 1001).unwrap());
        assert_eq!(clock.frame_to_tick(0).unwrap().0, 0);
        assert_eq!(clock.frame_to_tick(3).unwrap().0, 15);
        assert_eq!(clock.frame_to_tick(-2).unwrap().0, -10);
        assert_eq!(clock.tick_to_frame(SimTick(14)), 2);
        assert_eq!(clock.tick_to_frame(SimTick(15)), 3);
        assert_eq!(clock.tick_to_frame(SimTick(-1)), -1);
        // overflow type-checked
        assert!(clock.frame_to_tick(i64::MAX).is_err());
        // zero ticks is an invalid rate
        assert!(SimulationClock::new(FrameRate::new(24, 1).unwrap(), 0).is_err());
    }

    #[test]
    fn step_duration_is_fixed_from_exact_config() {
        let clock = SimulationClock::new(FrameRate::new(30000, 1001).unwrap(), 1).unwrap();
        let dt = clock.step_duration();
        let exact = 1001.0_f32 / 30000.0_f32;
        assert_eq!(dt, exact, "computed once from the exact config");
        let again = clock.step_duration();
        assert_eq!(dt, again);
    }

    #[test]
    fn time_of_tick_is_exact() {
        let clock = SimulationClock::new(FrameRate::new(24, 1).unwrap(), 2).unwrap();
        // tick 3 = 3/48 s
        let t = clock.time_of_tick(SimTick(3)).unwrap();
        assert_eq!(t, Rational::new(3, 48).unwrap());
    }
}
