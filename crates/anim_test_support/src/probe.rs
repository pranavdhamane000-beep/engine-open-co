//! Shared WP00 probe report structures and exit handling (spec 9.3: "structured
//! probe reports"; 9.8: `ProbeReport`, `AdapterIdentity`, `BudgetSnapshot`).

//! Non-fatal findings go in `diagnostics`; prescribed struct fields stay typed.
use serde::Serialize;

#[derive(Serialize)]
pub struct ProbeReport {
    pub schema_version: u32,
    pub capability: String,
    pub success: bool,
    pub diagnostics: Vec<String>,
}

/// Adapter identity exactly as spec 9.8 declares it (signed LUID).
#[derive(Serialize, Clone, Debug)]
pub struct AdapterIdentity {
    pub luid: i64,
    pub description: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub dedicated_video_memory: u64,
}

/// DXGI budget query result (spec 9.8 `BudgetSnapshot` field order).
#[derive(Serialize, Clone, Copy, Debug, Default)]
pub struct BudgetSnapshot {
    pub budget: u64,
    pub current_usage: u64,
    pub current_reservation: u64,
    pub available_for_reservation: u64,
    pub accounted_usage: u64,
    pub effective_usage: u64,
}

impl ProbeReport {
    pub fn new(capability: &str) -> Self {
        Self {
            schema_version: 1,
            capability: capability.into(),
            success: false,
            diagnostics: Vec::new(),
        }
    }
}

/// Print the report and exit with the prescribed status (spec 9.6: nonzero on
/// unmet requirements; 0 only when every required check verified).
pub fn emit_and_exit(report: &ProbeReport) -> ! {
    println!("{}", serde_json::to_string_pretty(report).unwrap_or_default());
    std::process::exit(if report.success { 0 } else { 1 });
}
