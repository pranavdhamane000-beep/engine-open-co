//! Main simulation step and advance loop (spec §14.9, §14.10).
//!
//! `step_once` is THE canonical transition function — identical code path
//! for forward, warm-up, replay, and export. The ONLY differences are the
//! mode's checkpoint policy and the export frame-boundary capture guarantee.

use crate::clock::SimTick;
use crate::error::SimResult;
use crate::events::TickSchedule;
use crate::mode::SimulationMode;
use crate::particles::{integrate_particles, process_emitters};
use crate::springs::solve_springs;
use crate::state::{SimState, SimulationContext};

/// Execute ONE simulation tick: process emitters, integrate particles,
/// solve springs, advance the event cursor and the tick counter. Returns
/// the number of particles spawned this tick (diagnostics only).
/// NEVER captures checkpoints — the caller owns cache policy via mode.
pub fn step_once(
    state: &mut SimState,
    schedule: &TickSchedule,
    context: &mut SimulationContext,
) -> SimResult<u32> {
    // 1. Consume scheduled events up to and including this tick.
    //    The cursor points at the NEXT unconsumed event; all schedule
    //    entries with tick == state.tick fire here, in CANONICAL order.
    while state.event_cursor < schedule.events.len() as u64 {
        let ev = &schedule.events[state.event_cursor as usize];
        if ev.tick.0 > state.tick.0 {
            break;
        }
        // (ev.tick < state.tick cannot happen: the cursor never rewinds
        // inside a live run; a restored checkpoint carries its own cursor.)
        match ev.payload {
            crate::events::EventPayload::Spawn { .. } => {
                // Spawn events are per-emitter triggers; the continuous
                // emission accumulator in process_emitters is the spawn
                // mechanic (ADR-0009). State changes happen there.
            }
            crate::events::EventPayload::Cue { .. } => {
                // Audio cues — no simulation state change (WP07 owns them).
            }
            crate::events::EventPayload::Note { .. } => {
                // Notes — no simulation state change.
            }
        }
        state.event_cursor += 1;
    }

    // 2. Emitters (exact rational emission accumulator → spawns)
    let spawned = process_emitters(state, context)?;

    // 3. Particles: semi-implicit Euler
    integrate_particles(state)?;

    // 4. Springs: fixed Gauss-Seidel iterations
    solve_springs(state)?;

    // 5. Advance the absolute tick
    state.tick = SimTick(state.tick.0 + 1);

    Ok(spawned)
}

/// Advance `ticks` steps (or stop at `target_tick` if given). Checkpoint
/// capture follows the mode; capture-at-frame-boundary is exact for Export.
/// Returns the final tick reached.
pub fn advance(
    state: &mut SimState,
    schedule: &TickSchedule,
    context: &mut SimulationContext,
    ticks: u64,
    target_tick: Option<SimTick>,
    mode: SimulationMode,
) -> SimResult<SimTick> {
    let ticks_per_frame = state.clock.ticks_per_frame as i64;
    for _ in 0..ticks {
        if let Some(target) = target_tick {
            if state.tick.0 >= target.0 {
                break;
            }
        }

        let at_frame_boundary = state.tick.0 % ticks_per_frame == 0;

        // Clone the (small) schedule so the step call doesn't hold an
        // immutable borrow of `context` alongside its mutable pass.
        let schedule = schedule.clone();
        step_once(state, &schedule, context)?;

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
    Ok(state.tick)
}

/// Prepare a warm-started export run: warm-up (no checkpointing) from
/// initial state to target, then return the state for the export loop.
pub fn prepare_export(
    initial_state: &SimState,
    schedule: &TickSchedule,
    context: &mut SimulationContext,
    target_tick: Option<SimTick>,
) -> SimResult<SimState> {
    let mut state = initial_state.clone();
    if let Some(target) = target_tick {
        if target.0 > 0 {
            advance(
                &mut state,
                schedule,
                context,
                target.0 as u64,
                Some(target),
                SimulationMode::WarmUp,
            )?;
        }
    }
    Ok(state)
}
