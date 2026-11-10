//! Particle spawning + semi-implicit Euler integration (spec §14.9,
//! §14.10). Draws are ADDRESS-BASED (Philox): (StreamKey, absolute
//! spawn tick, semantic draw address, lane) — no cursor exists, so any
//! process can reproduce any particle's properties. Insertion keeps the
//! particles vector SORTED by derived stable id; removal is a stable
//! filter, preserving order (insertion stability for the renderer).

use crate::clock::SimTick;
use crate::error::{SimError, SimResult};
use crate::random::{channel, purpose, random_f32, StreamKey};
use crate::state::{derive_particle_id, Particle, SimState};
use anim_core::{NodeId, Rational};
use anim_scene::SceneCache;
use num_bigint::BigInt;
use std::collections::BTreeMap;

/// Produce ALL property draws for ONE spawned particle. Each channel has
/// its own (draw_address, lane); the address space is ordinal × SPAN so
/// no two particles share an address.
fn particle_spawn_draws(
    key: &StreamKey,
    tick: SimTick,
    ordinal: u32,
) -> SimResult<[f32; channel::SPAN as usize]> {
    let base = ordinal as u64 * channel::SPAN as u64;
    let d = |ch: u32, lane: u8| -> SimResult<f32> {
        random_f32(key, tick, (base + ch as u64) as u32, lane)
    };
    Ok([
        d(channel::ANGLE, 0)?,
        d(channel::SPEED, 1)?,
        d(channel::LIFETIME_BAND, 2)?,
        d(channel::SIZE_BAND, 3)?,
        d(channel::ROTATION, 0)?,
        d(channel::JITTER_X, 1)?,
        d(channel::JITTER_Y, 2)?,
        d(channel::JITTER_Z, 3)?,
    ])
}

/// EXACTLY convert an authored f64 spawn_rate × dt (seconds) into a
/// Rational. IEEE-754 binary64 normal values are m · 2^(e−1075) with an
/// implicit 53rd mantissa bit; dt = rate.den / rate.num exactly, so the
/// product is an exact BigInt fraction. Denormals and zero handled.
fn rate_times_dt(
    rate_f64: f64,
    clock: &crate::clock::SimulationClock,
) -> SimResult<Rational> {
    if rate_f64 == 0.0 {
        return Ok(Rational::zero());
    }
    let bits = rate_f64.to_bits();
    let exp_field = ((bits >> 52) & 0x7ff) as i64;
    let (mantissa, exp2) = if exp_field == 0 {
        // denormal: NO implicit bit; value = frac · 2^(1−bias−52) = frac·2^-1074
        ((bits & ((1u64 << 52) - 1)) as u128, -1074i64)
    } else {
        (((bits & ((1u64 << 52) - 1)) as u128) | (1u128 << 52), exp_field - 1075)
    };
    let mant = BigInt::from(mantissa as i128);
    let (num, den) = if exp2 >= 0 {
        (mant * BigInt::from(2).pow(exp2 as u32), BigInt::from(1))
    } else {
        (mant, BigInt::from(2).pow((-exp2) as u32))
    };
    // dt seconds = rate.den / rate.num (exact — clock invariants)
    let product_num = num * BigInt::from(clock.rate.den as i64);
    let product_den = den * BigInt::from(clock.rate.num as i64);
    Rational::from_bigints(product_num, product_den)
        .map_err(|e| SimError::InvalidRate(format!("emission rational: {e}")))
}

/// Emitter world origins: evaluate the frame-0 snapshot (via the context's
/// cache — performance only, never state) and collect emitter nodes'
/// world translations. ADR-0009 documents the MVP limitation that emitter
/// motion is NOT tracked mid-run (origins are frame-0 positions).
fn emitter_origins(
    context: &mut crate::state::SimulationContext,
) -> SimResult<BTreeMap<NodeId, [f32; 3]>> {
    let fp = anim_core::FramePos::new(0, Rational::zero())
        .map_err(|e| SimError::TickOverflow(e.to_string()))?;
    let mut cache = std::mem::replace(&mut context.frame_cache, SceneCache::new(0));
    let snapshot = match cache.get_or_evaluate(&context.document, context.scene, fp) {
        Ok(s) => s,
        Err(e) => {
            context.frame_cache = cache;
            return Err(SimError::MissingInput(format!("snapshot: {e}")));
        }
    };
    context.frame_cache = cache;
    let scene = context
        .document
        .scenes
        .get(&context.scene)
        .cloned()
        .ok_or_else(|| SimError::MissingInput(format!("scene {}", context.scene)))?;

    let mut map = BTreeMap::new();
    for node in snapshot.nodes.iter() {
        if scene.emitters.contains_key(&node.id) {
            map.insert(
                node.id,
                [node.world_transform[0][2] as f32, node.world_transform[1][2] as f32, 0.0],
            );
        }
    }
    Ok(map)
}

/// Process ALL emitters at the current tick (spec §14.8 spawn mechanic):
/// 1. Advance the EXACT rational emission accumulator by spawn_rate·dt.
/// 2. Spawn the integer part exactly; the Philox rate-decision draw at
///    p = frac adopts ONE extra particle (the fractional carry).
/// 3. Per spawned particle: derive the stable id, draw properties
///    address-wise, insert sorted-by-id.
/// 4. Age all particles; stably remove expired ones.
///
/// Returns spawned count (diagnostics only — NOT part of the state hash).
pub fn process_emitters(
    state: &mut SimState,
    context: &mut crate::state::SimulationContext,
) -> SimResult<u32> {
    let project_seed = context.settings.project_seed;
    let system_id: u16 = 1; // "particle_system" — pinned in ADR-0009

    let origins = emitter_origins(context)?;
    let mut total_spawned = 0u32;

    let scene = context
        .document
        .scenes
        .get(&context.scene)
        .cloned()
        .ok_or_else(|| SimError::MissingInput(format!("scene {}", context.scene)))?;
    let emitters = scene.emitters; // owned snapshot of settings

    for (emitter_id, settings) in &emitters {
        let origin = origins.get(emitter_id).copied().unwrap_or([0.0; 3]);

        // ---- exact emission accumulator: emission += spawn_rate · dt ----
        let add = rate_times_dt(settings.spawn_rate, &state.clock)?;
        let entry = state.emission.entry(*emitter_id).or_insert(Rational::zero());
        let current = entry.clone();
        *entry =
            current.checked_add(add).map_err(|e| SimError::InvalidRate(e.to_string()))?;

        // Integer part spawns (integer_part ≥ 0: accumulator monotone).
        let int_part = entry
            .floor_to_i64()
            .map_err(|e| SimError::InvalidRate(format!("emission floor: {e}")))?
            .max(0) as u32;
        let remainder = entry
            .clone()
            .checked_sub(
                Rational::new(int_part as i128, 1)
                    .map_err(|e| SimError::InvalidRate(e.to_string()))?,
            )
            .map_err(|e| SimError::InvalidRate(e.to_string()))?;
        // frac = remainder ∈ [0,1) as f32 from EXACT integer num/den
        let frac = remainder.num() as f32 / remainder.den().max(1) as f32;

        let mut spawn_count = int_part;
        let rate_key = StreamKey {
            project_seed,
            system_id,
            entity_id: *emitter_id,
            purpose: purpose::EMITTER_RATE,
            stream_version: 1,
        };
        // rate-decision draw: tick-scoped, channel 0, lane 0 — the same
        // address ALWAYS yields the same adoption decision for a given
        // (emitter, tick, accumulated frac).
        let adopt_draw = random_f32(&rate_key, state.tick, 0, 0)?;
        if adopt_draw < frac.clamp(0.0, 1.0) {
            spawn_count += 1;
        }

        // Subtract EVERYTHING adopted, exactly — the accumulator carries
        // only the un-adopted fractional remainder into later ticks.
        let adopted = Rational::new(spawn_count as i128, 1)
            .map_err(|e| SimError::InvalidRate(e.to_string()))?;
        let current = entry.clone();
        *entry = current
            .checked_sub(adopted)
            .map_err(|e| SimError::InvalidRate(e.to_string()))?;

        if spawn_count == 0 {
            continue;
        }

        // ---- spawn loop: sorted-stable inserts ----
        let generation = *state.next_generation.get(emitter_id).unwrap_or(&0);
        let ordinal_base = *state.next_ordinal.get(emitter_id).unwrap_or(&0);

        let key = StreamKey {
            project_seed,
            system_id,
            entity_id: *emitter_id,
            purpose: purpose::PARTICLE_SPAWN,
            stream_version: 1,
        };

        for i in 0..spawn_count {
            let ordinal = ordinal_base.checked_add(i).ok_or_else(|| {
                SimError::TickOverflow(format!("emitter {emitter_id} ordinal overflow"))
            })?;
            let draws = particle_spawn_draws(&key, state.tick, ordinal)?;

            // semantic channels (pinned table in random.rs):
            let angle = draws[0] * 2.0 * std::f32::consts::PI;
            let speed = settings.initial_speed as f32 * (0.5 + draws[1]); // 0.5×..1.5×
            let size = (settings.size as f32) * (0.6 + draws[3] * 0.8); // 0.6×..1.4×
            let rotation = draws[4] * 2.0 * std::f32::consts::PI;
            let jx = draws[5] - 0.5;
            let jy = draws[6] - 0.5;
            let jz = (draws[7] - 0.5) * 0.25; // small z depth (2D scenes)

            // LIFETIME_BAND: ±20% of the authored tick lifetime (≥ 1 tick)
            let band = 0.8f64 + draws[2] as f64 * 0.4;
            let lifetime = ((settings.lifetime_ticks as f64) * band) as u64;
            let lifetime = lifetime.max(1);

            let id = derive_particle_id(*emitter_id, generation, ordinal);
            let particle = Particle {
                id,
                emitter: *emitter_id,
                generation,
                spawn_ordinal: ordinal,
                position: [origin[0] + jx * 0.2, origin[1] + jy * 0.2, origin[2] + jz],
                previous_position: [
                    origin[0] + jx * 0.2,
                    origin[1] + jy * 0.2,
                    origin[2] + jz,
                ],
                velocity: [angle.cos() * speed, angle.sin() * speed, jz],
                age_ticks: 0,
                lifetime_ticks: lifetime,
                size,
                rotation,
            };

            // sorted insert (stable id order — insertion stability §14.8)
            let idx = state.particles.partition_point(|p| p.id < id);
            state.particles.insert(idx, particle);
        }

        *state.next_generation.get_mut(emitter_id).unwrap() = generation + 1;
        *state.next_ordinal.get_mut(emitter_id).unwrap() = ordinal_base + spawn_count;
        total_spawned += spawn_count;
    }

    // ---- aging + stable removal ----
    // age_ticks advances EVERY tick (spawn tick ages immediately); a
    // particle dies when age REACHES lifetime (removal = stable filter:
    // survivor id order untouched).
    for p in state.particles.iter_mut() {
        p.age_ticks += 1;
    }
    state.particles.retain(|p| p.age_ticks < p.lifetime_ticks);

    Ok(total_spawned)
}

/// Semi-implicit Euler: v += a·dt; x += v·dt (§14.10). previous_position
/// is written BEFORE the move so the renderer gets exact two-frame pairs.
/// Any nonfinite product fails LOUDLY (SimError::NonfiniteState).
pub fn integrate_particles(state: &mut SimState) -> SimResult<()> {
    let dt = state.clock.step_duration();
    let gx = state.gravity[0] * dt;
    let gy = state.gravity[1] * dt;
    let gz = state.gravity[2] * dt;

    for p in &mut state.particles {
        p.previous_position = p.position;
        p.velocity[0] += gx;
        p.velocity[1] += gy;
        p.velocity[2] += gz;
        p.position[0] += p.velocity[0] * dt;
        p.position[1] += p.velocity[1] * dt;
        p.position[2] += p.velocity[2] * dt;
        if !p.position.iter().all(|v| v.is_finite())
            || !p.velocity.iter().all(|v| v.is_finite())
        {
            return Err(SimError::NonfiniteState {
                tick: state.tick.0,
                what: format!("particle {} nonfinite after integrate", p.id),
            });
        }
    }
    Ok(())
}
