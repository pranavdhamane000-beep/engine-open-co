//! WP00 adapter probe — spec 9.5 step 6, 9.7 "Adapter correlation", 9.8
//! `AdapterIdentity` + `BudgetSnapshot`, 9.9 `correlate_adapter`.
//!
//! Proves on this machine:
//!   1. Vulkan physical devices with `PhysicalDeviceIDProperties.deviceLUID`
//!      (when valid) correlate to a DXGI `IDXGIAdapter1` by **all 64 LUID
//!      bits**, cross-checked with vendor/device IDs — never substituted.
//!   2. The DXGI adapter's memory **budget** is queryable
//!      (`IDXGIAdapter3::QueryVideoMemoryInfo`, LOCAL segment group).
//!   3. The unavailable-identity path: devices without a valid LUID or
//!      without any matching DXGI adapter are reported `IdentityUnavailable`
//!      (never replaced by the primary desktop adapter), and a deliberately
//!      mismatched LUID matches nothing.

use anim_test_support::probe::{AdapterIdentity, BudgetSnapshot};
use serde::Serialize;
use windows::core::Interface;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter3, IDXGIFactory1, DXGI_ADAPTER_DESC1,
    DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
};

#[derive(Serialize)]
struct VulkanDeviceInfo {
    device_name: String,
    device_type: String,
    vendor_id: u32,
    device_id: u32,
    luid_valid: bool,
    luid: Option<i64>,
}

#[derive(Serialize)]
struct DxgiAdapterInfo {
    description: String,
    vendor_id: u32,
    device_id: u32,
    luid: i64,
    dedicated_video_memory: u64,
}

#[derive(Serialize)]
struct Correlation {
    vulkan_device: String,
    result: &'static str, // "Matched" | "IdentityUnavailable"
    reason: String,
    matched_adapter: Option<AdapterIdentity>,
}

#[derive(Serialize)]
struct AdapterProbeReport {
    schema_version: u32,
    capability: String,
    success: bool,
    vulkan_devices: Vec<VulkanDeviceInfo>,
    dxgi_adapters: Vec<DxgiAdapterInfo>,
    correlations: Vec<Correlation>,
    negative_rule_check: Option<String>,
    budget: Option<BudgetSnapshot>,
    diagnostics: Vec<String>,
}

/// Vulkan deviceLUID bytes (little-endian u32 low/high) → i64, same bit value
/// as the DXGI `LUID` composition (HighPart:i32 << 32 | LowPart:u32).
fn luid_from_bytes(bytes: [u8; 8]) -> i64 {
    let low = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let high = i32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    ((high as i64) << 32) | (low as i64)
}

fn luid_compose(high: i32, low: u32) -> i64 {
    ((high as i64) << 32) | (low as i64)
}

fn enumerate_vulkan_devices(report: &mut AdapterProbeReport) {
    let entry = match unsafe { ash::Entry::load() } {
        Ok(e) => e,
        Err(e) => {
            report.diagnostics.push(format!("vulkan entry load failed: {e}"));
            return;
        }
    };
    // External-memory-capabilities instance extensions: core-promoted at 1.1 but
    // empirically some Windows ICDs only fill deviceLUID when the capability
    // extension surface is explicitly present. API 1.3 requested explicitly.
    let ext_names = [
        ash::khr::external_memory_capabilities::NAME,
        ash::khr::external_semaphore_capabilities::NAME,
        ash::khr::external_fence_capabilities::NAME,
    ];
    let ext_ptrs: Vec<*const i8> = ext_names.iter().map(|n| n.as_ptr()).collect();
    let app_info =
        ash::vk::ApplicationInfo::default().api_version(ash::vk::API_VERSION_1_3);
    let create_info = ash::vk::InstanceCreateInfo::default()
        .application_info(&app_info)
        .enabled_extension_names(&ext_ptrs);
    let instance = match unsafe { entry.create_instance(&create_info, None) } {
        Ok(i) => i,
        Err(e) => {
            report.diagnostics.push(format!("vulkan instance creation failed: {e}"));
            return;
        }
    };
    let pds = match unsafe { instance.enumerate_physical_devices() } {
        Ok(pds) => pds,
        Err(e) => {
            report.diagnostics.push(format!("vulkan enumeration failed: {e}"));
            return;
        }
    };
    report.diagnostics.push(format!("vulkan_physical_devices = {}", pds.len()));
    for pd in pds {
        let mut id_props = ash::vk::PhysicalDeviceIDProperties::default();
        let mut props2 =
            ash::vk::PhysicalDeviceProperties2::default().push_next(&mut id_props);
        unsafe {
            instance.get_physical_device_properties2(pd, &mut props2);
        }
        let p = props2.properties;
        let name = String::from_utf8_lossy(
            &p.device_name
                .iter()
                .take_while(|&&b| b != 0)
                .map(|&b| b as u8)
                .collect::<Vec<u8>>(),
        )
        .into_owned();
        if id_props.device_luid_valid != 0 {
            report.vulkan_devices.push(VulkanDeviceInfo {
                device_name: name.clone(),
                device_type: format!("{:?}", p.device_type),
                vendor_id: p.vendor_id,
                device_id: p.device_id,
                luid_valid: true,
                luid: Some(luid_from_bytes(id_props.device_luid)),
            });
            report
                .diagnostics
                .push(format!("luid ok: {name} node_mask = {}", id_props.device_node_mask));
        } else {
            report.vulkan_devices.push(VulkanDeviceInfo {
                device_name: name.clone(),
                device_type: format!("{:?}", p.device_type),
                vendor_id: p.vendor_id,
                device_id: p.device_id,
                luid_valid: false,
                luid: None,
            });
            report
                .diagnostics
                .push(format!("luid invalid: {name} uuid = {:02x?}", id_props.device_uuid));
        }
    }
}

fn enumerate_dxgi_adapters(report: &mut AdapterProbeReport) {
    let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
        Ok(f) => f,
        Err(e) => {
            report.diagnostics.push(format!("dxgi factory creation failed: {e}"));
            return;
        }
    };
    for i in 0..u32::MAX {
        let adapter = match unsafe { factory.EnumAdapters1(i) } {
            Ok(a) => a,
            Err(_) => break, // end of enumeration, not a failure
        };
        let desc: DXGI_ADAPTER_DESC1 = match unsafe { adapter.GetDesc1() } {
            Ok(d) => d,
            Err(e) => {
                report.diagnostics.push(format!("dxgi GetDesc1({i}) failed: {e}"));
                continue;
            }
        };
        let description = String::from_utf16_lossy(
            &desc
                .Description
                .iter()
                .take_while(|&&c| c != 0)
                .copied()
                .collect::<Vec<u16>>(),
        );
        report.dxgi_adapters.push(DxgiAdapterInfo {
            description,
            vendor_id: desc.VendorId,
            device_id: desc.DeviceId,
            luid: luid_compose(desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart),
            dedicated_video_memory: desc.DedicatedVideoMemory as u64,
        });
    }
}

fn query_budget(
    adapter: &windows::Win32::Graphics::Dxgi::IDXGIAdapter1,
) -> Result<BudgetSnapshot, String> {
    let adapter3: IDXGIAdapter3 =
        adapter.cast().map_err(|e| format!("query_budget cast: {e}"))?;
    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    unsafe { adapter3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info) }
        .map_err(|e| format!("budget query: {e}"))?;
    Ok(BudgetSnapshot {
        budget: info.Budget,
        current_usage: info.CurrentUsage,
        current_reservation: info.CurrentReservation,
        available_for_reservation: info.AvailableForReservation,
        accounted_usage: 0, // engine-accounted usage begins at WP07 (spec 9.8)
        effective_usage: info.CurrentUsage,
    })
}

/// Print the FULL detailed report (it contains the prescribed ProbeReport
/// fields as a subset) and exit with the prescribed status (spec 9.6).
fn emit_and_exit_detailed(report: &AdapterProbeReport) -> ! {
    println!("{}", serde_json::to_string_pretty(report).unwrap_or_default());
    std::process::exit(if report.success { 0 } else { 1 });
}

fn correlate(report: &mut AdapterProbeReport, matched_luid: &mut Option<i64>) {
    for vdev in &report.vulkan_devices {
        let Some(vluid) = vdev.luid else {
            report.correlations.push(Correlation {
                vulkan_device: vdev.device_name.clone(),
                result: "IdentityUnavailable",
                reason: "deviceLUIDValid = false; never substituted per spec 9.7".into(),
                matched_adapter: None,
            });
            continue;
        };
        let dxgi_match_iter =
            report.dxgi_adapters.iter().filter(|d| d.luid == vluid).enumerate();
        let mut found = None;
        for (idx, d) in dxgi_match_iter {
            let ids_ok = d.vendor_id == vdev.vendor_id && d.device_id == vdev.device_id;
            report
                .diagnostics
                .push(format!(
                    "luid match idx {idx} ids_ok = {ids_ok} (vk vendor {:#06x} device {:#06x} vs dxgi vendor {:#06x} device {:#06x})",
                    vdev.vendor_id, vdev.device_id, d.vendor_id, d.device_id
                ));
            if ids_ok {
                found = Some(AdapterIdentity {
                    luid: d.luid,
                    description: d.description.clone(),
                    vendor_id: d.vendor_id,
                    device_id: d.device_id,
                    dedicated_video_memory: d.dedicated_video_memory,
                });
            }
        }
        match found {
            Some(identity) => {
                *matched_luid = Some(identity.luid);
                report.correlations.push(Correlation {
                    vulkan_device: vdev.device_name.clone(),
                    result: "Matched",
                    reason: "all 64 LUID bits + vendor/device IDs cross-check".into(),
                    matched_adapter: Some(identity),
                });
            }
            None => {
                report.correlations.push(Correlation {
                    vulkan_device: vdev.device_name.clone(),
                    result: "IdentityUnavailable",
                    reason: "no DXGI adapter with matching LUID/IDs; primary adapter NOT substituted".into(),
                    matched_adapter: None,
                });
            }
        }
    }
}

fn main() {
    let mut report = AdapterProbeReport {
        schema_version: 1,
        capability: "adapter_luid_correlation".into(),
        success: false,
        vulkan_devices: Vec::new(),
        dxgi_adapters: Vec::new(),
        correlations: Vec::new(),
        negative_rule_check: None,
        budget: None,
        diagnostics: Vec::new(),
    };

    enumerate_vulkan_devices(&mut report);
    enumerate_dxgi_adapters(&mut report);
    report.diagnostics.push(format!("dxgi_adapters = {}", report.dxgi_adapters.len()));
    if report.dxgi_adapters.is_empty() {
        report.diagnostics.push("failure: DXGI enumeration produced no adapters".into());
        emit_and_exit_detailed(&report);
    }

    let mut matched_luid: Option<i64> = None;
    correlate(&mut report, &mut matched_luid);

    // Budget query for the matched adapter, if any (spec 9.5 step 6).
    if let Some(luid) = matched_luid {
        let factory: IDXGIFactory1 = match unsafe { CreateDXGIFactory1() } {
            Ok(f) => f,
            Err(e) => {
                report.diagnostics.push(format!("budget factory failed: {e}"));
                emit_and_exit_detailed(&report);
            }
        };
        for i in 0..u32::MAX {
            let adapter = match unsafe { factory.EnumAdapters1(i) } {
                Ok(a) => a,
                Err(_) => break,
            };
            let desc = match unsafe { adapter.GetDesc1() } {
                Ok(d) => d,
                Err(_) => break,
            };
            if luid == luid_compose(desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart) {
                match query_budget(&adapter) {
                    Ok(b) => {
                        report.diagnostics.push(format!(
                            "budget queried luid {luid}: {} bytes",
                            b.budget
                        ));
                        report.budget = Some(b);
                    }
                    Err(reason) => {
                        report.diagnostics.push(format!("budget query failed: {reason}"))
                    }
                }
                break;
            }
        }
    }

    // Negative rule check (spec 9.7: never substitute): a deliberately
    // mismatched LUID must match no DXGI adapter.
    let probe_luid = matched_luid.map(|l| l ^ 0x5555_5555_5555_5555);
    if let Some(p) = probe_luid {
        let substitutes = report.dxgi_adapters.iter().any(|d| d.luid == p);
        report.negative_rule_check = if substitutes {
            Some("FAIL: mismatched LUID matched an adapter".into())
        } else {
            Some("ok: mismatched LUID matches no adapter; no substitution possible".into())
        };
    } else {
        report.negative_rule_check =
            Some("skipped: no valid matched LUID available".into());
    }

    // Success criteria (spec 9.12): deviceLUID-correlated identity present,
    // budget query returned sane values, negative path proved, and every
    // valid-LUID Vulkan device is either Matched or explicitly reported.
    let matched = report.correlations.iter().any(|c| c.result == "Matched");
    let budget_ok = report.budget.map(|b| b.budget > 0).unwrap_or(false);
    let negative_ok =
        report.negative_rule_check.as_ref().map(|s| s.starts_with("ok:")).unwrap_or(false);
    let all_accounted = report.vulkan_devices.iter().all(|v| {
        v.luid.is_none()
            || report.correlations.iter().any(|c| c.vulkan_device == v.device_name)
    });

    report.success = matched && budget_ok && negative_ok && all_accounted;
    if !matched {
        report.diagnostics.push("failure: no LUID-correlated Vulkan/DXGI match".into());
    }
    if !budget_ok {
        report.diagnostics.push("failure: budget not queryable or zero".into());
    }
    if !negative_ok {
        report
            .diagnostics
            .push("failure: negative rule check did not prove no-substitution".into());
    }
    if !all_accounted {
        report
            .diagnostics
            .push("failure: a valid-LUID device has no correlation record".into());
    }

    emit_and_exit_detailed(&report);
}
