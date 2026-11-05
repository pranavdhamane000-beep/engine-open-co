//! Normalized rational construction, exact comparison and checked arithmetic.
//! Spec 10.5–10.8: denominator always positive, reduced by gcd, zero = 0/1,
//! BigInt intermediates, checked i128 conversion, no wrapping.

use crate::error::CoreError;
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive, Zero};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rational {
    num: i128,
    den: i128,
}

impl Rational {
    /// Normalized construction: zero den / negative den rejected and
    /// normalized; reduced by gcd; zero stored as 0/1.
    pub fn new(num: i128, den: i128) -> Result<Self, CoreError> {
        if den == 0 {
            return Err(CoreError::ZeroDenominator);
        }
        let (n, d) = if den < 0 { (-num, -den) } else { (num, den) };
        if n == 0 {
            return Ok(Self { num: 0, den: 1 });
        }
        let g = n.abs().gcd(&d).max(1);
        Ok(Self { num: n / g, den: d / g })
    }

    pub fn num(&self) -> i128 {
        self.num
    }

    pub fn den(&self) -> i128 {
        self.den
    }

    /// Exact ordering. BigInt intermediates (crate policy: never wrap),
    /// sign-safe via the den > 0 invariant.
    fn cmp_exact(&self, other: &Self) -> std::cmp::Ordering {
        let l = BigInt::from(self.num) * BigInt::from(other.den);
        let r = BigInt::from(other.num) * BigInt::from(self.den);
        l.cmp(&r)
    }
}

impl PartialOrd for Rational {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Rational {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.cmp_exact(other)
    }
}

impl std::fmt::Display for Rational {
    /// Canonical "num/den" — exactly the text `Deserialize` accepts.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

impl Rational {
    pub fn is_zero(&self) -> bool {
        self.num == 0
    }

    /// Canonical zero (0/1), for callers that need the value without a
    /// Fallible construction round trip.
    pub fn zero() -> Self {
        Self { num: 0, den: 1 }
    }

    /// Exact addition;BigInt intermediate, checked i128 result.
    pub fn checked_add(self, rhs: Self) -> Result<Self, CoreError> {
        let n = BigInt::from(self.num) * rhs.den + BigInt::from(rhs.num) * self.den;
        let d = BigInt::from(self.den) * rhs.den;
        Self::from_bigints(n, d)
    }

    /// Exact subtraction (needed by frame_containing).
    pub fn checked_sub(self, rhs: Self) -> Result<Self, CoreError> {
        let n = BigInt::from(self.num) * rhs.den - BigInt::from(rhs.num) * self.den;
        let d = BigInt::from(self.den) * rhs.den;
        Self::from_bigints(n, d)
    }

    /// Exact multiplication (spec 10.7 "Rational arithmetic").
    pub fn checked_mul(self, rhs: Self) -> Result<Self, CoreError> {
        let n = BigInt::from(self.num) * rhs.num;
        let d = BigInt::from(self.den) * rhs.den;
        Self::from_bigints(n, d)
    }

    /// Exact division (time computations divide by the frame rate).
    pub fn checked_div(self, rhs: Self) -> Result<Self, CoreError> {
        if rhs.num.is_zero() {
            return Err(CoreError::ZeroDenominator);
        }
        let n = BigInt::from(self.num) * rhs.den;
        let d = BigInt::from(self.den) * rhs.num;
        Self::from_bigints(n, d)
    }

    /// Checked i64 floor for the exact content.
    pub fn floor_to_i64(&self) -> Result<i64, CoreError> {
        floor_i128_i64(self.num, self.den)
    }

    /// Checked i64 ceiling for the exact content: ceil(a/b) = -floor(-a/b).
    pub fn ceil_to_i64(&self) -> Result<i64, CoreError> {
        let neg = self.num.checked_neg().ok_or(CoreError::ArithmeticOverflow)?;
        let floor_of_neg = floor_i128_i64(neg, self.den)?;
        floor_of_neg.checked_neg().ok_or(CoreError::ArithmeticOverflow)
    }

    /// Public BigInt-input constructor: reduce exactly, then checked-convert
    /// both parts to i128. Used by cross-type time computations.
    pub fn from_bigints(n: BigInt, d: BigInt) -> Result<Self, CoreError> {
        if d.is_zero() {
            return Err(CoreError::ZeroDenominator);
        }
        if !d.is_positive() {
            return Err(CoreError::ArithmeticOverflow);
        }
        let n = if n.is_zero() { BigInt::zero() } else { n };
        let g = n.abs().gcd(&d).max(BigInt::from(1));
        let n = (n / &g).to_i128().ok_or(CoreError::ArithmeticOverflow)?;
        let d = (d / &g).to_i128().ok_or(CoreError::ArithmeticOverflow)?;
        Ok(Self { num: n, den: d })
    }
}

/// floor division with positive divisor: div_euclid already floors.
fn floor_i128_i64(num: i128, den: i128) -> Result<i64, CoreError> {
    let floor = num.div_euclid(den);
    i64::try_from(floor).map_err(|_| CoreError::ArithmeticOverflow)
}

impl crate::hash::Canonical for Rational {
    fn append_canonical(&self, out: &mut Vec<u8>) {
        crate::hash::append_i128(out, self.num);
        crate::hash::append_i128(out, self.den);
    }
}

/// Hash is valid ONLY because of the normalization invariant (num/den are
/// reduced with den > 0, so each value has a unique representation).
impl std::hash::Hash for Rational {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.num.hash(h);
        self.den.hash(h);
    }
}

/// Canonical serialization: "num/den" text (spec 10.5 "canonical num/den
/// serialization"); zero is always "0/1".
impl Serialize for Rational {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&format!("{}/{}", self.num, self.den))
    }
}

impl<'de> Deserialize<'de> for Rational {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        let (n, den) = s
            .split_once('/')
            .ok_or_else(|| serde::de::Error::custom("expected num/den rational"))?;
        let num: i128 =
            n.parse().map_err(|_| serde::de::Error::custom("rational num parse"))?;
        let den: i128 =
            den.parse().map_err(|_| serde::de::Error::custom("rational den parse"))?;
        Self::new(num, den).map_err(|e| serde::de::Error::custom(e.to_string()))
    }
}
