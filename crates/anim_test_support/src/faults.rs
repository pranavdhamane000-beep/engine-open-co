//! Test-only deterministic fault injection (spec §20.3 faults.rs, §20.4, §20.7).
//!
//! CRITICAL CONSTRAINT (spec §20.4, §20.10, §20.12): fault hooks are
//! PROHIBITED in release binaries. The injection REGISTRY lives behind the
//! crate feature `test-faults` (OFF by default; `verify_release.ps1` fails
//! any release compiled with it enabled). Without the feature this module
//! compiles to a stub whose `inject` ALWAYS fails typed — the registry
//! bytes never ship.
//!
//! Injection is deterministic and scope-scoped: `inject(point, failure)`
//! registers a one-shot failure for an exact fault point and returns an
//! RAII [`FaultGuard`]; when the guard (or the test) drops, the registry
//! entry is REMOVED. Tests reset between cases by dropping their guards —
//! no global leakage (§20.8: "fault injectors are test-only and reset
//! before each case").

pub use crate::determinism::TestError;

/// Exact fault-point enumeration (spec §20.8 data structures — do not
/// reorder or rename; serialized as-is in test artifacts).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum FaultPoint {
    AssetRead,
    ReadbackMap,
    EncoderStart,
    EncoderWrite,
    MuxExit,
    DecodeVerify,
    DiskFull,
    PublishReplace,
}

impl FaultPoint {
    /// Ordered for stable test iteration (never enumeration order — BTree
    /// semantics via explicit table, spec §13.7 spirit).
    pub const ALL: [FaultPoint; 8] = [
        FaultPoint::AssetRead,
        FaultPoint::ReadbackMap,
        FaultPoint::EncoderStart,
        FaultPoint::EncoderWrite,
        FaultPoint::MuxExit,
        FaultPoint::DecodeVerify,
        FaultPoint::DiskFull,
        FaultPoint::PublishReplace,
    ];

    /// Canonical name used in artifact records.
    pub fn name(self) -> &'static str {
        match self {
            FaultPoint::AssetRead => "asset_read",
            FaultPoint::ReadbackMap => "readback_map",
            FaultPoint::EncoderStart => "encoder_start",
            FaultPoint::EncoderWrite => "encoder_write",
            FaultPoint::MuxExit => "mux_exit",
            FaultPoint::DecodeVerify => "decode_verify",
            FaultPoint::DiskFull => "disk_full",
            FaultPoint::PublishReplace => "publish_replace",
        }
    }
}

/// The deterministic failure a boundary must produce (§20.12: missing
/// assets, device loss, FFmpeg failure, disk full, cancellation and
/// publication failure must all produce their specified SAFE behavior).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Failure {
    /// Boundary fails as if the underlying OS/IO operation failed.
    Io(String),
    /// Boundary behaves as if it hit the checked timeout (kill + reap).
    Timeout,
    /// Boundary behaves as if an allocation/ENOMEM OOM occurred.
    OutOfMemory,
    /// Readback/tools return short data (e.g. short pipe read).
    ShortRead,
    /// Malformed byte stream at that boundary (type-safe rejection).
    MalformedInput,
}

/// A live injection. Dropping removes the hook (RAII reset per §20.8).
#[derive(Debug)]
pub struct FaultGuard {
    point: FaultPoint,
    active: bool,
}

impl Drop for FaultGuard {
    fn drop(&mut self) {
        if self.active {
            registry_clear(self.point);
        }
    }
}

impl FaultGuard {
    /// Test-only accessor: which point is currently armed.
    pub fn point(&self) -> FaultPoint {
        self.point
    }
}

#[cfg(feature = "test-faults")]
mod registry {
    use super::{Failure, FaultPoint};
    use std::collections::BTreeMap;
    use std::sync::{Mutex, OnceLock};

    /// Test-only registry. NOT compiled without `test-faults` — release
    /// binaries carry no hook state whatsoever (spec §20.4).
    fn hooks() -> &'static Mutex<BTreeMap<FaultPoint, Failure>> {
        static HOOKS: OnceLock<Mutex<BTreeMap<FaultPoint, Failure>>> = OnceLock::new();
        HOOKS.get_or_init(|| Mutex::new(BTreeMap::new()))
    }

    pub fn insert(point: FaultPoint, failure: Failure) {
        if let Ok(mut map) = hooks().lock() {
            map.insert(point, failure);
        }
    }

    pub fn clear(point: FaultPoint) {
        if let Ok(mut map) = hooks().lock() {
            map.remove(&point);
        }
    }

    pub fn take(point: FaultPoint) -> Option<Failure> {
        hooks().lock().ok().and_then(|mut map| map.remove(&point))
    }

    pub fn current() -> BTreeMap<FaultPoint, Failure> {
        hooks().lock().map(|m| m.clone()).unwrap_or_default()
    }
}

/// Production code compiled WITH the feature asks the registry for the
/// armed failure at this boundary. Without the feature this is a no-op
/// returning `None` (compiled out — the hookless build keeps zero bytes of
/// injection state in release binaries).
pub fn take(point: FaultPoint) -> Option<Failure> {
    #[cfg(feature = "test-faults")]
    {
        registry::take(point)
    }
    #[cfg(not(feature = "test-faults"))]
    {
        let _ = point;
        None
    }
}

fn registry_clear(point: FaultPoint) {
    #[cfg(feature = "test-faults")]
    {
        registry::clear(point);
    }
    #[cfg(not(feature = "test-faults"))]
    {
        let _ = point;
    }
}

/// Exact §20.9 interface: `inject(point, failure) -> Result<FaultGuard, TestError>`.
///
/// Without the `test-faults` feature this ALWAYS returns
/// `Err(TestError::FaultsDisabled)` — builds without hooks fail loud, not
/// silent.
pub fn inject(point: FaultPoint, failure: Failure) -> Result<FaultGuard, TestError> {
    #[cfg(feature = "test-faults")]
    {
        registry::insert(point, failure);
        Ok(FaultGuard { point, active: true })
    }
    #[cfg(not(feature = "test-faults"))]
    {
        let _ = (point, failure);
        Err(TestError::FaultsDisabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_points_cover_every_serialized_boundary_exactly() {
        // §20.8 names exactly these eight.
        assert_eq!(FaultPoint::ALL.len(), 8);
        let names: Vec<&str> = FaultPoint::ALL.iter().map(|p| p.name()).collect();
        let mut unique = names.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), names.len(), "fault point names must be unique");
    }

    #[cfg(feature = "test-faults")]
    #[test]
    fn injection_scopes_via_raii_and_resets() {
        let point = FaultPoint::EncoderWrite;
        let failure = Failure::Io("boom".into());
        {
            let guard = inject(point, failure.clone()).expect("armed");
            assert_eq!(guard.point(), point);
            assert_eq!(take(point), Some(failure), "one-shot: take removes");
            assert_eq!(take(point), None, "second take observes no hook");
            // Re-arm inside the same guard scope: bind it so Drop does not
            // clear the hook before we observe it.
            let guard2 = inject(point, Failure::Timeout).expect("armed again");
            assert_eq!(take(point), Some(Failure::Timeout));
            drop(guard2);
        }
        // Guard dropped → reset (§20.8).
        assert_eq!(take(point), None);
    }

    #[cfg(not(feature = "test-faults"))]
    #[test]
    fn without_feature_injection_fails_typed_and_takes_nothing() {
        assert!(matches!(
            inject(FaultPoint::DiskFull, Failure::OutOfMemory),
            Err(TestError::FaultsDisabled)
        ));
        assert_eq!(take(FaultPoint::DiskFull), None);
    }
}
