//! Seeking: greatest verified checkpoint ≤ target, else replay from
//! tick 0. Corrupted cache entries are discarded and rebuilt lower;
//! a replay budget is enforced (ReplayBudgetExhausted) per §14.11.

use crate::checkpoint::CheckpointCache;
use crate::clock::SimTick;
use crate::error::{SimError, SimResult};
use crate::mode::SimulationMode;
use crate::state::{SimState, SimulationContext};

/// Default replay budget (ticks) for a single seek.
pub const DEFAULT_REPLAY_BUDGET: u64 = 10_000_000;

/// Seek to `target` tick and return the state AT that tick (before any
/// step at `target+1`). Strategy (§14.7, §14.11):
/// 1. Find the greatest cache checkpoint ≤ target.
/// 2. Verify newest→oldest; corrupted entries are DISCARDED (and everything
///    newer with them, since they depend on the corrupted prefix).
/// 3. Restore from the verified checkpoint if any, else from tick 0.
/// 4. Replay forward to target through the SAME step_once path.
/// 5. Enforce the replay budget.
pub fn seek(
    context: &mut SimulationContext,
    target: SimTick,
    budget: Option<u64>,
) -> SimResult<SimState> {
    let budget = budget.unwrap_or(DEFAULT_REPLAY_BUDGET);
    if target.0 < 0 {
        return Err(SimError::NegativeTick(format!("seek target {target:?}")));
    }

    // 1–2. Newest-to-oldest verified candidate
    let verified_tick = find_verified_checkpoint(&mut context.checkpoint_cache, target);

    // 3. Restore start state
    let (mut state, start_tick) = match verified_tick {
        Some(tick) => {
            let chk = context
                .checkpoint_cache
                .get_latest(target)
                .filter(|c| c.tick.0 == tick)
                .ok_or_else(|| SimError::CacheCorrupt(tick, "candidate vanished".into()))?
                .clone();
            let clock = crate::clock::SimulationClock::new(
                context.document.frame_rate,
                context.settings.ticks_per_frame,
            )?;
            (crate::checkpoint::restore_checkpoint(&chk, clock)?, SimTick(tick))
        }
        None => (context.initial()?, SimTick::ZERO),
    };

    // 4. Replay forward through step_once
    if start_tick.0 < target.0 {
        let replay_ticks = (target.0 - start_tick.0) as u64;
        if replay_ticks > budget {
            return Err(SimError::ReplayBudgetExhausted { tick: target.0 });
        }
        advance_next_stop(&mut state, context, target, SimulationMode::Replay)?;
    }

    Ok(state)
}

/// Advance EXACTLY to target (not ticks-relative): steps while tick < target.
/// Used by seek to land the state precisely at `target`.
fn advance_next_stop(
    state: &mut SimState,
    context: &mut SimulationContext,
    target: SimTick,
    mode: SimulationMode,
) -> SimResult<()> {
    let ticks_per_frame = state.clock.ticks_per_frame as i64;

    while state.tick.0 < target.0 {
        let at_frame_boundary = state.tick.0 % ticks_per_frame == 0;

        let schedule = context.schedule.clone();
        crate::step::step_once(state, &schedule, context)?;

        if mode.captures_checkpoints() {
            let should_capture = if mode.forces_frame_capture() {
                at_frame_boundary
            } else {
                context.checkpoint_cache.should_capture(state.tick)
            };
            if should_capture {
                let chk = crate::checkpoint::capture_checkpoint(state)?;
                context.checkpoint_cache.insert(state.tick, chk);
            }
        }
    }
    if state.tick.0 > target.0 {
        return Err(SimError::MalformedCheckpoint(format!(
            "overshot seek target {target:?} at tick {}",
            state.tick.0
        )));
    }
    Ok(())
}

/// Find the greatest tick ≤ target whose checkpoint verifies. Corrupted
/// entries are removed (along with everything newer — later checkpoints
/// depend on the same algorithm version and cannot be trusted past a
/// corrupted prefix). Returns the surviving tick, or None.
fn find_verified_checkpoint(cache: &mut CheckpointCache, target: SimTick) -> Option<i64> {
    loop {
        let candidate = cache.get_latest(target)?;
        match crate::checkpoint::verify_checkpoint(candidate) {
            Ok(()) => return Some(candidate.tick.0),
            Err(_) => {
                let corrupted = candidate.tick.0;
                cache.remove_before_strict(corrupted);
            }
        }
    }
}
