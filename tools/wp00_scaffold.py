"""WP00 scaffold generator — writes the 13-crate workspace per spec 4.1/9.3."""
import sys
from pathlib import Path

sys.stdout.reconfigure(errors="replace")
root = Path(__file__).resolve().parent.parent

CRATES = {
    "anim_core":         "Controlled exports for time, IDs, errors, canonical hashing and atomic file replacement (spec 4.2)",
    "anim_assets":       "Asset import, verification, resolution and decoder registry (spec 4.2)",
    "anim_project":      "Versioned authoritative document, schema, migration, save and recovery (spec 4.2)",
    "anim_commands":     "Envelopes, registry, validation, transaction processing, undo and redo (spec 4.2)",
    "anim_scene":        "Immutable snapshots, property evaluation, hierarchy, cameras and events (spec 4.2)",
    "anim_sim":          "Fixed-step clock, random streams, state, replay, checkpoints, particles and springs (spec 4.2)",
    "anim_render":       "Device, graph, passes, GPU resources, memory policy and readback (spec 4.2)",
    "anim_audio":        "Offline audio planning/mixing and optional preview output (spec 4.2)",
    "anim_export":       "Resolved export jobs, FFmpeg, readback coordination, verification and publication (spec 4.2)",
    "anim_editor":       "Session, transport, panels, pen strokes and preview quality (spec 4.2)",
    "anim_app":          "Native executable composition and shutdown (spec 4.2)",
    "anim_headless":     "CLI parsing, export execution, reports and stable exit codes (spec 4.2)",
    "anim_test_support": "Scene builders, deterministic runners, golden comparison and fault injection (spec 4.2)",
}

members_lines = ",\n".join(f'    "crates/{n}"' for n in CRATES)

WORKSPACE_TOML = (
    '[workspace]\n'
    'resolver = "2"\n'
    'members = [\n'
    + members_lines
    + ',\n]\n\n'
    '[workspace.package]\n'
    + '''version = "0.1.0"
edition = "2021"
license = "UNLICENSED"
publish = false

[workspace.dependencies]
# Versions recorded per spec 5.1; each becomes referenced when its WP lands.
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
anyhow = "1"
uuid = { version = "1", features = ["v4", "serde"] }
num-bigint = "0.4"
num-integer = "0.1"
num-traits = "0.2"
blake3 = "1"
hex = "0.4"
wgpu = "27"
bytemuck = "1"
half = "2"
glam = "0.30"
winit = "0.30"
egui = "0.33"
egui-dock = "0.18"
egui-winit = { version = "0.33", default-features = false }
windows = { version = "0.62", features = [] }
symphonia = { version = "0.5", features = ["wav", "ogg", "flac"] }
rubato = "0.16"
hound = "3.5"
cpal = "0.16"
ringbuf = "0.4"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
proptest = "1"
criterion = "0.5"
tempfile = "3"

[profile.release]
lto = false
codegen-units = 256
'''
)
# normalize stray double blank lines
WORKSPACE_TOML = WORKSPACE_TOML.replace('\n\n\n', '\n\n').replace('egui-dock = "0.18"\n \n', 'egui-dock = "0.18"\n')

TOOLCHAIN = """[toolchain]
channel = "1.99.1"
components = ["rustfmt", "clippy"]
profile = "minimal"
"""

RUSTFMT = """edition = "2021"
max_width = 92
use_small_heuristics = "Max"
"""

# clippy.toml intentionally NOT generated: clippy's config schema does not
# accept lint-name table sections. The multiple-crate-versions allow is
# applied per-crate via #![allow] in generated lib.rs stubs instead.

CONFIG_TOML = """# Non-secret application defaults and explicit external-tool settings (spec 9.3).
[ffmpeg]
# Absolute paths REQUIRED for production export (spec 5.1); no PATH discovery.
ffmpeg_local = "D:\\\\ffmp\\\\ffmpeg-9.0.2-essentials_build\\\\bin\\\\ffmpeg.exe"
ffprobe_local = "D:\\\\ffmp\\\\ffmpeg-9.0.2-essentials_build\\\\bin\\\\ffprobe.exe"
"""

(root / "Cargo.toml").write_text(WORKSPACE_TOML, encoding="utf-8")
(root / "rust-toolchain.toml").write_text(TOOLCHAIN, encoding="utf-8")
(root / "rustfmt.toml").write_text(RUSTFMT, encoding="utf-8")
(root / "config").mkdir(exist_ok=True)
(root / "config" / "default.toml").write_text(CONFIG_TOML, encoding="utf-8")

for name, desc in CRATES.items():
    crate = root / "crates" / name
    (crate / "src").mkdir(parents=True, exist_ok=True)
    toml = (
        f"[package]\nname = \"{name}\"\nversion.workspace = true\n"
        f"edition.workspace = true\nlicense.workspace = true\npublish.workspace = true\n\n[dependencies]\n"
    )
    if name == "anim_test_support":
        toml += "tempfile.workspace = true\n"
    (crate / "Cargo.toml").write_text(toml, encoding="utf-8")
    if name in ("anim_app", "anim_headless"):
        (crate / "src" / "main.rs").write_text(
            "fn main() {\n"
            "    // Spec 9.3: executable entry-point stub - real logic arrives at WP09/WP10.\n"
            f"    println!(\"{name}: entry-point stub (WP00 scaffold)\");\n"
            "}\n",
            encoding="utf-8",
        )
    else:
        (crate / "src" / "lib.rs").write_text(
            f"//! {name} - {desc}\n//!\n"
            "//! WP00 scaffold state: compilable empty library root with NO placeholder\n"
            "//! production behavior (spec 9.3). Behavior lands per work package.\n"
            "#![deny(clippy::all)]\n"
            "#![allow(clippy::multiple_crate_versions)]  // wgpu/egui ecosystem transitive dup minors; revisit WP11\n",
            encoding="utf-8",
        )

(root / "crates" / "anim_test_support" / "tests").mkdir(parents=True, exist_ok=True)
(root / "crates" / "anim_test_support" / "tests" / "wp00_smoke.rs").write_text(
    """//! WP00 smoke test (spec 9.3): workspace compiles and runtime works.
#[test]
fn workspace_smoke() {
    // integer sanity in debug - no GPU, no external tools
    let a: u64 = 900 * 1001 + 1;
    assert_eq!(a, 900901);
}
""",
    encoding="utf-8",
)
print("scaffold written")
