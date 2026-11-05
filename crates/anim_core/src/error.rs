//! Typed error enum for WP01 (spec 10.11). Never clamps, never panics,
//! never wraps; variants are stable and source-chained where useful (§5.1).

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoreError {
    #[error("division by zero denominator")]
    ZeroDenominator,
    #[error("nonpositive rate: {num}/{den}")]
    NonPositiveRate { num: u32, den: u32 },
    #[error("arithmetic overflow in exact computation")]
    ArithmeticOverflow,
    #[error("invalid range: start {start} exceeds end {end}")]
    InvalidRange { start: i64, end: i64 },
    #[error("subframe out of range [0,1): {0} (canonical round trip required)")]
    SubframeOutOfRange(String),
    #[error("identifier parse error: {0}")]
    IdentifierParse(String),
    /// Rendered messages only: `std::io::Error` is not Eq/Clone, and error
    /// equality is needed by undo/diff tests. The wrapped kind is preserved
    /// in the message text.
    #[error("io: {0}")]
    Io(String),
}
