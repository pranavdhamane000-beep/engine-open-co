//! Real DXGI adapter telemetry (spec §16.3 dxgi.rs, §16.6 correlation
//! strategy from ADR-0003). Only the CORRELATED adapter is ever queried.
//!
//! Correlation recipe (ADR-0003, wp00_adapter.rs proof):
//! 1. Vulkan physical device with valid deviceLUID (all 64 bits) → match
//!    exactly one DXGI adapter with same LUID + vendor/device ID cross-check.
//! 2. If LUID invalid/absent → NO substitution (IdentityUnavailable).
//! 3. If multiple DXGI adapters share the same (vendor, device, LUID) →
//!    ambiguity → None (never substitute).
//!
//! The engine NEVER requests/selects a device (D-11); the caller provides
//! the Vulkan device info and wgpu device; we only correlate.

use super::identity::{correlate, AdapterIdentity, IdentityHints, IdentityState};
use super::BudgetTelemetry;
use super::MemoryError;
// (no import needed)
use std::sync::Arc;
use windows::core::Interface;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIAdapter3, IDXGIFactory1, DXGI_ADAPTER_DESC1,
    DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
};

/// One enumerated DXGI adapter (all fields needed for correlation + budget).
#[derive(Debug, Clone)]
struct DxgiAdapter {
    description: String,
    vendor_id: u32,
    device_id: u32,
    luid: i64,
    dedicated_video_memory: u64,
    inner: IDXGIAdapter1,
}

/// Live telemetry source backed by a CORRELATED DXGI adapter.
pub struct DxgiTelemetry {
    adapter: Arc<DxgiAdapter>,
}

impl DxgiTelemetry {
    /// Resolve and construct a live telemetry source for the caller's
    /// rendering device. `hints` must contain the Vulkan device's LUID
    /// (when available) + vendor/device/description for correlation
    /// against the DXGI enumeration (§16.6, ADR-0003).
    ///
    /// Returns:
    /// - `Ok(Some(telemetry))` — correlated adapter found, telemetry live.
    /// - `Ok(None)` — no confident correlation (identity becomes
    ///   `Unavailable`; manager continues with conservative estimates).
    /// - `Err` — DXGI factory creation failed (fatal for this path).
    pub fn resolve(hints: &IdentityHints) -> Result<Option<Self>, MemoryError> {
        let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }
            .map_err(|e| MemoryError::QueryFailed(format!("CreateDXGIFactory1: {e}")))?;

        // 1. Enumerate all DXGI adapters with their LUID + ids.
        let mut adapters: Vec<DxgiAdapter> = Vec::new();
        for i in 0..u32::MAX {
            let adapter: IDXGIAdapter1 = match unsafe { factory.EnumAdapters1(i) } {
                Ok(a) => a,
                Err(_) => break, // end of enumeration
            };
            let desc: DXGI_ADAPTER_DESC1 = unsafe { adapter.GetDesc1() }
                .map_err(|e| MemoryError::QueryFailed(format!("GetDesc1({i}): {e}")))?;
            let description = String::from_utf16_lossy(
                &desc
                    .Description
                    .iter()
                    .take_while(|&&c| c != 0)
                    .copied()
                    .collect::<Vec<u16>>(),
            );
            let luid = ((desc.AdapterLuid.HighPart as i64) << 32)
                | (desc.AdapterLuid.LowPart as u64 as i64);
            adapters.push(DxgiAdapter {
                description,
                vendor_id: desc.VendorId,
                device_id: desc.DeviceId,
                luid,
                dedicated_video_memory: desc.DedicatedVideoMemory as u64,
                inner: adapter,
            });
        }

        if adapters.is_empty() {
            return Err(MemoryError::QueryFailed(
                "DXGI enumeration produced zero adapters".into(),
            ));
        }

        // 2. Correlate using the identity module's pure logic (tests hit
        // this same path via synthetic candidates).
        let candidates: Vec<AdapterIdentity> = adapters
            .iter()
            .map(|a| AdapterIdentity {
                luid: a.luid,
                description: a.description.clone(),
                vendor_id: a.vendor_id,
                device_id: a.device_id,
                dedicated_video_memory: a.dedicated_video_memory,
            })
            .collect();

        let correlated = correlate(hints, &candidates);

        let Some(found) = correlated else {
            return Ok(None); // no confident match — not an error
        };

        // 3. Find the matching inner adapter (there is exactly one by
        // correlate's guarantees — LUID unique when correlated).
        let inner =
            adapters.into_iter().find(|a| a.luid == found.luid).ok_or_else(|| {
                MemoryError::StaleIdentity(
                    "correlated LUID vanished from enumeration".into(),
                )
            })?;

        Ok(Some(Self { adapter: Arc::new(inner) }))
    }

    /// Build the identity state from a successful resolution (chunk 2
    /// caller feeds this back to the manager).
    pub fn identity_state(&self) -> IdentityState {
        IdentityState::Resolved(AdapterIdentity {
            luid: self.adapter.luid,
            description: self.adapter.description.clone(),
            vendor_id: self.adapter.vendor_id,
            device_id: self.adapter.device_id,
            dedicated_video_memory: self.adapter.dedicated_video_memory,
        })
    }
}

impl BudgetTelemetry for DxgiTelemetry {
    fn budget_and_usage(&mut self) -> Result<(u64, u64), MemoryError> {
        // IDXGIAdapter3::QueryVideoMemoryInfo on the LOCAL segment
        // (spec §16.6: "local segment group"; DXGI_MEMORY_SEGMENT_GROUP_LOCAL = 0).
        let adapter3: IDXGIAdapter3 = self.adapter.inner.cast().map_err(|e| {
            MemoryError::QueryFailed(format!("IDXGIAdapter1 -> IDXGIAdapter3 cast: {e}"))
        })?;
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        unsafe {
            adapter3.QueryVideoMemoryInfo(
                0, // node index 0
                DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
                &mut info,
            )
        }
        .map_err(|e| MemoryError::QueryFailed(format!("QueryVideoMemoryInfo: {e}")))?;

        Ok((info.Budget, info.CurrentUsage))
    }
}

/// Helper to extract correlation hints from a wgpu Adapter (returned by
/// `request_adapter`). The caller provides the adapter info directly.
pub fn hints_from_adapter(adapter: &wgpu::Adapter) -> IdentityHints {
    let info = adapter.get_info();
    IdentityHints {
        luid: None, // caller fills if available (Vulkan deviceLUID)
        vendor_id: Some(info.vendor),
        device_id: Some(info.device),
        description: Some(info.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runtime check: the live telemetry struct is Send + Sync for
    /// the manager (single-threaded render thread, but trait object
    /// requires it).
    #[test]
    fn telemetry_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DxgiTelemetry>();
    }
}
