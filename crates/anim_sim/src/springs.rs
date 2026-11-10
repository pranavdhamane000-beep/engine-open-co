//! Spring solver (spec §14.8, §14.10): gravity + spring-edge forces on
//! bodies, then a FIXED number of Gauss-Seidel relaxation iterations over
//! the edges, then ONE velocity integration. The iteration count is part
//! of the settings (validated ≥ 1) and is a SUPER HARD-ish requirement:
//! it must NEVER change with cache cadence, seek or mode — determinism
//! depends on it (§14.2, G4).
//!
//! WP05 MVP: uniform inverse_mass = 1.0 (ADR-0009), sequential (Gauss-
//! Seidel) edge relaxation in canonical edge-id order.

use crate::error::{SimError, SimResult};
use crate::state::SimState;
use anim_core::NodeId;

/// Solve springs for one tick: gravity accumulates into velocity, the
/// spring constraints relax `solver_iterations` times (Gauss-Seidel, in
/// canonical edge order), then ONE position integration. All state f32
/// products are checked — nonfinite fails LOUDLY.
pub fn solve_springs(state: &mut SimState) -> SimResult<()> {
    if state.solver_iterations == 0 {
        return Err(SimError::InvalidSolverCount(0));
    }
    let dt = state.clock.step_duration();
    let gx = state.gravity[0] * dt;
    let gy = state.gravity[1] * dt;
    let gz = state.gravity[2] * dt;

    // 1. gravity into velocity (explicit Euler on velocity)
    for b in &mut state.bodies {
        b.velocity[0] += gx;
        b.velocity[1] += gy;
        b.velocity[2] += gz;
    }

    // 2. FIXED Gauss-Seidel iterations over edges (canonical order:
    //    state.springs is sorted by id at all times).
    for _ in 0..state.solver_iterations {
        relax_edges_once(state)?;
    }

    // 3. ONE integration: x += v·dt (semi-implicit)
    integrate_bodies(state)
}

/// One Gauss-Seidel pass over every edge: sequential in-place relaxation
/// (later edges see earlier updates — that is what makes it Gauss-Seidel
/// and order-sensitive; the edge order is canonical and pinned).
fn relax_edges_once(state: &mut SimState) -> SimResult<()> {
    // index bodies by NodeId (bodies are sorted; lookup map rebuilt here —
    // edge count ≪ body count is NOT assumed; map is O(B) to rebuild)
    let mut index: std::collections::BTreeMap<NodeId, usize> =
        std::collections::BTreeMap::new();
    for (i, b) in state.bodies.iter().enumerate() {
        index.insert(b.id, i);
    }

    for spring in &state.springs {
        let ia = *index.get(&spring.a).ok_or_else(|| {
            SimError::UnknownEventTarget(format!("spring endpoint missing: {}", spring.a))
        })?;
        let ib = *index.get(&spring.b).ok_or_else(|| {
            SimError::UnknownEventTarget(format!("spring endpoint missing: {}", spring.b))
        })?;

        // measure current error along the edge
        let (ax, ay, az) = {
            let a = &state.bodies[ia];
            let b = &state.bodies[ib];
            (
                a.position[0] - b.position[0],
                a.position[1] - b.position[1],
                a.position[2] - b.position[2],
            )
        };
        let dist_sq = ax * ax + ay * ay + az * az;
        if dist_sq <= 1e-12 {
            // coincident points: direction undefined → skip this edge for
            // this pass (deterministic: distance is 0 exactly in BOTH runs)
            continue;
        }
        let dist = dist_sq.sqrt();
        let err = dist - spring.rest_length;

        // positional relaxation, split by inverse masses (MVP: 1.0, 1.0
        // → each endpoint moves half the error)
        // stiffness ∈ [0,∞) maps to a stable per-pass correction factor
        // k_pass = stiffness / (1 + stiffness) ∈ [0,1) — documented ADR-0009
        let k_pass = spring.stiffness / (1.0 + spring.stiffness);
        if k_pass == 0.0 {
            continue; // inert edge (authored stiffness 0)
        }
        let inv_sum = state.bodies[ia].inverse_mass + state.bodies[ib].inverse_mass;
        if inv_sum <= 0.0 {
            continue; // both endpoints immovable: nothing relaxes
        }
        let cx = ax / dist;
        let cy = ay / dist;
        let cz = az / dist;
        let corr = err * k_pass;
        let (wa, wb) = (
            state.bodies[ia].inverse_mass / inv_sum,
            state.bodies[ib].inverse_mass / inv_sum,
        );
        // A moves opposite B along the edge
        state.bodies[ia].position[0] -= cx * corr * wa;
        state.bodies[ia].position[1] -= cy * corr * wa;
        state.bodies[ia].position[2] -= cz * corr * wa;
        state.bodies[ib].position[0] += cx * corr * wb;
        state.bodies[ib].position[1] += cy * corr * wb;
        state.bodies[ib].position[2] += cz * corr * wb;
    }

    // finite check each pass (cheap; catches blowups at the pass they occur)
    for b in &state.bodies {
        if !b.position.iter().all(|v| v.is_finite()) {
            return Err(SimError::NonfiniteState {
                tick: state.tick.0,
                what: format!("body {} nonfinite during relaxation", b.id),
            });
        }
    }
    Ok(())
}

/// ONE semi-implicit integration of bodies (previous_position written
/// before the move for exact two-frame renderer pairing); clears `force`.
fn integrate_bodies(state: &mut SimState) -> SimResult<()> {
    let dt = state.clock.step_duration();
    for b in &mut state.bodies {
        b.previous_position = b.position;
        b.position[0] += b.velocity[0] * dt;
        b.position[1] += b.velocity[1] * dt;
        b.position[2] += b.velocity[2] * dt;
        b.force = [0.0; 3];
        if !b.position.iter().all(|v| v.is_finite())
            || !b.velocity.iter().all(|v| v.is_finite())
        {
            return Err(SimError::NonfiniteState {
                tick: state.tick.0,
                what: format!("body {} nonfinite after integrate", b.id),
            });
        }
    }
    Ok(())
}
