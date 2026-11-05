//! Rational-time property keys (spec 12.3).
//!
//! A property is a track of keys at EXACT rational times; WP04 evaluates
//! them at FramePos positions (unique canonical times per key list is a
//! schema invariant — duplicate times are validation errors).

use crate::ProjectError;
use anim_core::Rational;
use serde::{Deserialize, Serialize};

/// A property value. Floats reject nonfinite values at validation (JSON
/// cannot carry them anyway — spec WP04 InvalidNumericValue made typed here).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PropertyValue {
    Float(f64),
    Int(i64),
    Bool(bool),
    Text(String),
}

impl PropertyValue {
    pub fn validate(&self) -> Result<(), ProjectError> {
        match self {
            PropertyValue::Float(f) if !f.is_finite() => {
                Err(ProjectError::Validation("property float is not finite".into()))
            }
            _ => Ok(()),
        }
    }
}

/// Interpolation declaration. The numeric algorithm is VERSIONED (ease v1):
/// Linear = exact rational interval fraction applied at the final numeric
/// operation; Step = preceding key held; Smooth = v1 smoothstep of
/// p = u*u*(3-2u) computed on f64 fractions only after exact interval math.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Easing {
    Linear,
    Step,
    Smooth,
}

/// One key: exact rational time + value + outgoing easing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertyKey {
    pub time: Rational,
    pub value: PropertyValue,
    pub easing: Easing,
}

/// Keys sorted by time ascending, times strictly unique.
/// `default` is the authored value used BEFORE the first key when no keys
/// apply (spec WP04: empty properties use authored defaults).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct PropertyTrack {
    pub default: Option<PropertyValue>,
    pub keys: Vec<PropertyKey>,
}

impl PropertyTrack {
    pub fn new(default: Option<PropertyValue>) -> Self {
        Self { default, keys: Vec::new() }
    }

    /// Insert or replace at the key's exact time, keeping sort order.
    /// Returns true when an existing key at that time was REPLACED.
    pub fn upsert_key(&mut self, key: PropertyKey) -> bool {
        match self.keys.binary_search_by(|k| k.time.cmp(&key.time)) {
            Ok(idx) => {
                self.keys[idx] = key;
                true
            }
            Err(idx) => {
                self.keys.insert(idx, key);
                false
            }
        }
    }

    /// Remove the key at an exact time; Some(removed key) when present.
    pub fn remove_key(&mut self, time: &Rational) -> Option<PropertyKey> {
        let idx = self.keys.binary_search_by(|k| k.time.cmp(time)).ok()?;
        Some(self.keys.remove(idx))
    }

    pub fn key_at(&self, time: &Rational) -> Option<&PropertyKey> {
        self.keys.binary_search_by(|k| k.time.cmp(time)).ok().map(|i| &self.keys[i])
    }

    /// Full invariants: finite floats, strict time ordering/unique times.
    pub fn validate(&self) -> Result<(), ProjectError> {
        for (idx, k) in self.keys.iter().enumerate() {
            k.value.validate()?;
            if idx > 0 && k.time <= self.keys[idx - 1].time {
                return Err(ProjectError::Validation(format!(
                    "property keys unsorted or duplicate at index {idx}"
                )));
            }
        }
        if let Some(d) = &self.default {
            d.validate()?;
        }
        Ok(())
    }
}
