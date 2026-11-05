//! anim_app entry point (spec 9.3 stub until WP09/WP10 fills it).
//!
//! Per decision D-11: NO automatic GPU selection happens here. At startup
//! the engine prints a plain reminder listing the GPUs it can see by name
//! and tells the user to make sure the discrete GPU is the one rendering.
//! The enumeration is display-only: it never requests a device and never
//! reorders or picks by device type.

/// Display-only adapter listing for the startup reminder. Returns the
/// human-readable message; the caller prints it.
fn gpu_startup_reminder() -> Vec<String> {
    let mut lines = Vec::new();

    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapters = instance.enumerate_adapters(wgpu::Backends::all());

    if adapters.is_empty() {
        lines.push("GPU startup notice: no graphics adapters enumerated.".into());
        return lines;
    }

    let mut names: Vec<String> = adapters
        .iter()
        .map(|a| {
            let info = a.get_info();
            format!("{} ({:?}, {})", info.name, info.device_type, info.backend)
        })
        .collect();
    names.sort(); // display order only — never used to pick anything

    lines.push("=== GPU reminder (no automatic selection, decision D-11) ===".into());
    for (i, n) in names.iter().enumerate() {
        lines.push(format!("  [{}] {}", i + 1, n));
    }
    lines.push(
        "REMINDER: make sure the DISCRETE GPU (e.g. your NVIDIA GeForce RTX) is the one rendering:".into(),
    );
    lines.push("  - Windows: Settings > Display > Graphics > set this app / global preference to the dGPU".into());
    lines.push(
        "  - NVIDIA Control Panel: Manage 3D Settings > Preferred graphics processor"
            .into(),
    );
    lines.push(
        "The engine will simply use whatever adapter is active; the choice stays yours."
            .into(),
    );
    lines
}

fn main() {
    for line in gpu_startup_reminder() {
        println!("{line}");
    }
    // Spec 9.3: executable entry-point stub - real logic arrives at WP09/WP10.
    println!("anim_app: entry-point stub (WP00 scaffold)");
}
