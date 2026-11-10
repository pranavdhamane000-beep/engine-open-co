//! FrameRate, FramePos, frame ranges and sample intervals (spec 10.7–10.9).
//!
//! One overflow-aware time model: NTSC rates are rationals (24000/1001),
//! Frame  n starts at exactly n/fps, positions are (frame + subframe)/fps
//! with subframe normalized to [0,1) (overflow carries into the frame),
//! and the timeline supports negative positions for stateless evaluation.

use crate::error::CoreError;
use crate::hash::Canonical;
use crate::rational::Rational;
use num_bigint::BigInt;
use num_traits::{Signed, ToPrimitive, Zero};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Positive rational frame rate. NTSC 29.97 is `FrameRate::new(30000, 1001)`,
/// never a decimal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameRate {
    pub num: u32,
    pub den: u32,
}

impl FrameRate {
    /// Positive-only construction (spec 10.11: reject nonpositive rates).
    pub fn new(num: u32, den: u32) -> Result<Self, CoreError> {
        if num == 0 || den == 0 {
            return Err(CoreError::NonPositiveRate { num, den });
        }
        Ok(Self { num, den })
    }

    /// Guards used by every method: a rate constructed via struct literal
    /// (fields are public per spec 10.8) is still checked at call time.
    fn validate(&self) -> Result<(), CoreError> {
        self.check().map(|_| ())
    }

    fn check(&self) -> Result<(i128, i128), CoreError> {
        if self.num == 0 || self.den == 0 {
            return Err(CoreError::NonPositiveRate { num: self.num, den: self.den });
        }
        Ok((self.num as i128, self.den as i128))
    }

    /// Frame n starts at exactly n/fps; an arbitrary position
    /// (frame + subframe)/fps. Exact rational time:
    /// t = (frame*sub_den + sub_num) * fps_den / (sub_den * fps_num).
    pub fn time_of(&self, position: FramePos) -> Result<Rational, CoreError> {
        self.validate()?;
        let sd = BigInt::from(position.subframe.den());
        let sn = BigInt::from(position.subframe.num());
        let n = BigInt::from(position.frame) * &sd * (self.den as i128)
            + &sn * (self.den as i128);
        let d = &sd * (self.num as i128);
        Rational::from_bigints(n, d)
    }

    /// Frame containing rational seconds `t`: whole = floor(t*fps),
    /// fraction = t*fps - whole. At a negative exact boundary floor selects
    /// the mathematically preceding frame, not truncation toward zero.
    pub fn frame_containing(&self, time: Rational) -> Result<FramePos, CoreError> {
        self.validate()?;
        let product = time
            .checked_mul(Rational::new(self.num as i128, self.den as i128)?)
            .map_err(|_| CoreError::ArithmeticOverflow)?;
        let whole = product.floor_to_i64()?;
        let fraction = product
            .checked_sub(
                Rational::new(whole as i128, 1)
                    .map_err(|_| CoreError::ArithmeticOverflow)?,
            )
            .map_err(|_| CoreError::ArithmeticOverflow)?;
        FramePos::new(whole, fraction)
    }

    /// Frame-start audio interval (spec 10.7): start = floor(f*sr*den/num),
    /// end = floor((f+1)*sr*den/num); adjacent intervals share an endpoint
    /// but no sample.
    pub fn frame_samples(
        &self,
        frame: i64,
        sample_rate: u32,
    ) -> Result<SampleInterval, CoreError> {
        self.validate()?;
        if sample_rate == 0 {
            return Err(CoreError::NonPositiveRate { num: sample_rate, den: 1 });
        }
        let inner = BigInt::from(frame)
            * BigInt::from(sample_rate as i128)
            * BigInt::from(self.den as i128);
        let start = floor_bigint_i64(&inner, self.num as i128)?;
        let one_frame = BigInt::from(sample_rate as i128) * BigInt::from(self.den as i128);
        let end = floor_bigint_i64(&(inner + one_frame), self.num as i128)?;
        SampleInterval::new(start, end, sample_rate)
    }

    /// Sample index containing an arbitrary position: floor(time * sr).
    pub fn sample_at(
        &self,
        position: FramePos,
        sample_rate: u32,
    ) -> Result<i64, CoreError> {
        if sample_rate == 0 {
            return Err(CoreError::NonPositiveRate { num: sample_rate, den: 1 });
        }
        let t = self.time_of(position)?;
        let at = t
            .checked_mul(
                Rational::new(sample_rate as i128, 1)
                    .map_err(|_| CoreError::ArithmeticOverflow)?,
            )
            .map_err(|_| CoreError::ArithmeticOverflow)?;
        at.floor_to_i64()
    }
}

/// floor of a nonneg-or-signed BigInt numerator over a positive i128 divisor,
/// checked into i64. BigInt division rounds toward zero, so negative values
/// need the Euclid adjust: floor(q) = q - 1 when not divisible and sign < 0.
fn floor_bigint_i64(n: &BigInt, d: i128) -> Result<i64, CoreError> {
    let dv = BigInt::from(d);
    let mut q = n / &dv;
    let r = n % &dv;
    if !r.is_zero() && !n.is_positive() {
        q -= BigInt::from(1);
    }
    q.to_i64().ok_or(CoreError::ArithmeticOverflow)
}

/// A position on the canonical timeline: frame `n` spans [n, n+1)/fps and
/// `subframe ∈ [0,1)`; overflow carries into the frame. Invariants require
/// normalization, so fields are private and construction goes through
/// `FramePos::new`.
/// WP04 addition (ADR-0008): `PartialOrd`/`Ord` over (frame, subframe) —
/// the canonical event sort key needs exact FramePos ordering; rational
/// field ordering makes the derived ordering mathematically correct
/// (frame-major, then exact subframe).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FramePos {
    frame: i64,
    subframe: Rational,
}

impl FramePos {
    /// Normalized construction: `subframe` must already lie in [0,1) minus
    /// its whole part; any whole part is carried into the frame with
    /// checked arithmetic.
    pub fn new(frame: i64, subframe: Rational) -> Result<Self, CoreError> {
        // floor of the subframe (subframe denominator is positive by
        // construction, so div_euclid floors).
        let whole = subframe.floor_to_i64()?;
        let frac = subframe
            .checked_sub(
                Rational::new(whole as i128, 1)
                    .map_err(|_| CoreError::ArithmeticOverflow)?,
            )
            .map_err(|_| CoreError::ArithmeticOverflow)?;
        let carried = frame.checked_add(whole).ok_or(CoreError::ArithmeticOverflow)?;
        Ok(Self {
            frame: carried,
            subframe: if frac.is_zero() {
                Rational::new(0, 1).map_err(|_| CoreError::ArithmeticOverflow)?
            } else {
                frac
            },
        })
    }

    /// A zero position (frame 0, subframe 0).
    pub fn zero() -> Self {
        Self { frame: 0, subframe: Rational::zero() }
    }

    pub fn frame(&self) -> i64 {
        self.frame
    }

    /// Subframe normalized to [0,1) — canonical round trip guaranteed.
    pub fn subframe(&self) -> &Rational {
        &self.subframe
    }
}

impl Canonical for FramePos {
    fn append_canonical(&self, out: &mut Vec<u8>) {
        crate::hash::append_i64(out, self.frame);
        self.subframe.append_canonical(out);
    }
}

impl Serialize for FramePos {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("FramePos", 2)?;
        st.serialize_field("frame", &self.frame)?;
        st.serialize_field("subframe", &self.subframe)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for FramePos {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Raw {
            frame: i64,
            subframe: Rational,
        }
        let raw = Raw::deserialize(d)?;
        // Canonical round trip required: subframe must be in [0,1); a
        // non-canonical encoding is rejected rather than silently carried
        // (spec 10.11: never clamp invalid user data).
        let in_range = raw.subframe.is_zero() || {
            raw.subframe.num() >= 0
                && raw.subframe.den() >= raw.subframe.num()
                && raw.subframe.num() > 0
        };
        if !in_range {
            return Err(serde::de::Error::custom(CoreError::SubframeOutOfRange(format!(
                "{}+{}",
                raw.frame,
                raw.subframe.num()
            ))));
        }
        FramePos::new(raw.frame, raw.subframe)
            .map_err(|e| serde::de::Error::custom(e.to_string()))
    }
}

/// Half-open frame range `[start, end)` for export clipping and simulation
/// scheduling. Zero-length ranges are valid; inverted ranges are not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameRange {
    pub start: i64,
    pub end: i64,
}

impl FrameRange {
    pub fn new(start: i64, end: i64) -> Result<Self, CoreError> {
        if start > end {
            return Err(CoreError::InvalidRange { start, end });
        }
        Ok(Self { start, end })
    }

    pub fn contains(&self, frame: i64) -> bool {
        frame >= self.start && frame < self.end
    }
}

impl Canonical for FrameRange {
    fn append_canonical(&self, out: &mut Vec<u8>) {
        crate::hash::append_i64(out, self.start);
        crate::hash::append_i64(out, self.end);
    }
}

/// Half-open sample interval `[start, end)` at a declared sample rate;
/// adjacent intervals share an endpoint but no sample (spec 10.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SampleInterval {
    pub start: i64,
    pub end: i64,
    pub sample_rate: u32,
}

impl SampleInterval {
    pub fn new(start: i64, end: i64, sample_rate: u32) -> Result<Self, CoreError> {
        if sample_rate == 0 {
            return Err(CoreError::NonPositiveRate { num: sample_rate, den: 1 });
        }
        if start > end {
            return Err(CoreError::InvalidRange { start, end });
        }
        Ok(Self { start, end, sample_rate })
    }

    /// Count of samples half-owned by this interval (end exclusive).
    pub fn len(&self) -> i64 {
        self.end - self.start
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Canonical for SampleInterval {
    fn append_canonical(&self, out: &mut Vec<u8>) {
        crate::hash::append_i64(out, self.start);
        crate::hash::append_i64(out, self.end);
        crate::hash::append_u32(out, self.sample_rate);
    }
}

/// Integer gcd helper retained for tests: rates are stored unnormalized.
pub fn gcd_u32(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd_u32(b, a % b)
    }
}

impl Serialize for FrameRate {
    /// Canonical text form "num/den" (unnormalized, exactly like Rational).
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{}/{}", self.num, self.den))
    }
}

impl<'de> Deserialize<'de> for FrameRate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let (num, den) = s.split_once('/').ok_or_else(|| {
            serde::de::Error::custom(format!("invalid frame rate text: {s}"))
        })?;
        FrameRate::new(
            num.parse().map_err(serde::de::Error::custom)?,
            den.parse().map_err(serde::de::Error::custom)?,
        )
        .map_err(|e| serde::de::Error::custom(e.to_string()))
    }
}

impl Serialize for FrameRange {
    /// Canonical object form with start/end in declaration order.
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("FrameRange", 2)?;
        st.serialize_field("start", &self.start)?;
        st.serialize_field("end", &self.end)?;
        st.end()
    }
}

impl<'de> Deserialize<'de> for FrameRange {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct Raw {
            start: i64,
            end: i64,
        }
        let raw = Raw::deserialize(d)?;
        FrameRange::new(raw.start, raw.end)
            .map_err(|e| serde::de::Error::custom(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntsc_is_a_rational_not_a_decimal() {
        let ntsc = FrameRate::new(24000, 1001).unwrap();
        let t =
            ntsc.time_of(FramePos::new(1, Rational::new(0, 1).unwrap()).unwrap()).unwrap();
        assert_eq!(t.num(), 1001);
        assert_eq!(t.den(), 24000);
    }

    #[test]
    fn negative_boundary_floors_to_mathematical_frame() {
        // 24 fps: time -1/3 s is exactly frame -8's end → belongs to frame -1
        // at 24 fps? floor(-8) = -8; the boundary itself floors to -8.
        let fps = FrameRate::new(24, 1).unwrap();
        let pos = fps.frame_containing(Rational::new(-1, 3).unwrap()).unwrap();
        assert_eq!(pos.frame(), -8);
        assert_eq!(pos.subframe().num(), 0);
    }

    #[test]
    fn sample_intervals_partition_exactly() {
        let fps = FrameRate::new(24000, 1001).unwrap();
        let mut prev_end: Option<i64> = None;
        for f in 0..10_000 {
            let iv = fps.frame_samples(f, 48_000).unwrap();
            assert_eq!(iv.start, if let Some(pe) = prev_end { pe } else { iv.start });
            prev_end = Some(iv.end);
        }
    }
}
