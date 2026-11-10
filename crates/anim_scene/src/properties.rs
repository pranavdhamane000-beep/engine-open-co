//! Key lookup and interpolation (spec §13.6 "Interpolation").
//!
//! Rules (spec verbatim):
//!   * binary-search the ORDERED keys (track invariant: strictly increasing)
//!   * at an exact key time return THAT key
//!   * before the first key use the FIRST value; after the last use the LAST
//!   * Step properties select the PRECEDING key
//!   * numeric interpolation computes the EXACT rational interval fraction
//!     `u = (t-t0)/(t1-t0)` and converts to floating point only at the
//!     final numeric operation
//!
//! Easing algorithm is VERSIONED: `EASING_ALGORITHM_VERSION = 1`
//! (Linear = exact u as f64; Smooth = u²(3-2u) on that f64; Step =
//! preceding key). Non-interpolable values (Bool/Text) require Step
//! between distinct keys (`InvalidEasing` otherwise).

use crate::error::{SceneError, SceneResult};
use anim_core::Rational;
use anim_project::{Easing, PropertyTrack, PropertyValue};

/// Version of the numeric easing algorithm (changes are new versions, per
/// spec's "versioned numeric algorithm" rule).
pub const EASING_ALGORITHM_VERSION: u32 = 1;

/// Evaluate one property track at exact time `t`.
pub fn evaluate_property(
    track: &PropertyTrack,
    t: &Rational,
    property: &str,
) -> SceneResult<PropertyValue> {
    let keys = &track.keys;
    if keys.is_empty() {
        return track.default.clone().ok_or_else(|| {
            SceneError::InvalidPropertyKeys(format!(
                "property '{property}': empty track without authored default"
            ))
        });
    }

    // binary search the ordered keys; exact hit returns THAT key (copy)
    match keys.binary_search_by(|k| k.time.cmp(t)) {
        Ok(idx) => Ok(keys[idx].value.clone()),
        Err(0) => Ok(keys[0].value.clone()), // before first → first value
        Err(idx) if idx == keys.len() => Ok(keys[keys.len() - 1].value.clone()),
        Err(idx) => {
            // between keys[idx-1] (t0) and keys[idx] (t1)
            let left = &keys[idx - 1];
            let right = &keys[idx];
            if left.easing == Easing::Step {
                // "Step properties select the preceding key"
                return Ok(left.value.clone());
            }
            interpolate(left, right, t, property)
        }
    }
}

/// u = (t-t0)/(t1-t0) EXACTLY as a rational; float leaves only here.
fn u_exact(
    left: &anim_project::PropertyKey,
    right: &anim_project::PropertyKey,
    t: &Rational,
) -> SceneResult<Rational> {
    let span =
        right.time.clone().checked_sub(left.time.clone()).map_err(SceneError::from)?;
    if span.is_zero() {
        // duplicate times are a document invariant violation; fall back to
        // the left key rather than dividing by zero (defensive)
        return Ok(Rational::zero());
    }
    let offset = t.clone().checked_sub(left.time.clone()).map_err(SceneError::from)?;
    offset.checked_div(span).map_err(SceneError::from)
}

/// Exact rational interval fraction → f64 (single conversion point).
fn u_as_f64(u: &Rational) -> SceneResult<f64> {
    let n = u.num();
    let d = u.den();
    if d == 0 {
        return Err(SceneError::TimeOverflow("zero denominator fraction".into()));
    }
    // i128 → f64 rounds for |v| > 2^53; the EXACT boundary/clamp decisions
    // above are made in Rational domain — only the mix coefficient floats.
    let f = (n as f64) / (d as f64);
    require_finite(f, "interval fraction")
}

fn require_finite(f: f64, what: &str) -> SceneResult<f64> {
    if f.is_finite() {
        Ok(f)
    } else {
        Err(SceneError::InvalidNumericValue(format!("{what} is not finite")))
    }
}

/// Apply the DECLARED easing (versioned algorithm) to the f64 fraction.
fn apply_easing_f64(easing: Easing, u: f64) -> f64 {
    debug_assert!((0.0..=1.0).contains(&u));
    match easing {
        // EASE_v1
        Easing::Linear => u,
        Easing::Smooth => {
            // u*u*(3-2u), computed on f64 fractions only
            u * u * (3.0 - 2.0 * u)
        }
        Easing::Step => unreachable!("Step handled by the caller (preceding key)"),
    }
}

fn interpolate(
    left: &anim_project::PropertyKey,
    right: &anim_project::PropertyKey,
    t: &Rational,
    property: &str,
) -> SceneResult<PropertyValue> {
    let u = u_exact(left, right, t)?;
    match (&left.value, &right.value) {
        (PropertyValue::Float(a), PropertyValue::Float(b)) => {
            let f = u_as_f64(&u)?;
            let e = apply_easing_f64(left.easing, f);
            let mixed = a + (b - a) * e;
            require_finite(mixed, "interpolated float").map(PropertyValue::Float)
        }
        (PropertyValue::Int(a), PropertyValue::Int(b)) => {
            let f = u_as_f64(&u)?;
            let e = apply_easing_f64(left.easing, f);
            // rounds half away from zero (deterministic standard runtime f64)
            let mixed = (*a as f64) + ((*b - *a) as f64) * e;
            require_finite(mixed, "interpolated int")
                .map(|m| PropertyValue::Int(m.round() as i64))
        }
        (PropertyValue::Bool(_), _) | (_, PropertyValue::Bool(_)) => {
            Err(SceneError::InvalidEasing(
                property.to_string(),
                format!("{t}: bool keys require Step easing"),
            ))
        }
        (PropertyValue::Text(_), _) | (_, PropertyValue::Text(_)) => {
            Err(SceneError::InvalidEasing(
                property.to_string(),
                format!("{t}: text keys require Step easing"),
            ))
        }
        (a, b) => Err(SceneError::InvalidPropertyKeys(format!(
            "property '{property}': mismatched value types {a:?} → {b:?} in one track"
        ))),
    }
}
