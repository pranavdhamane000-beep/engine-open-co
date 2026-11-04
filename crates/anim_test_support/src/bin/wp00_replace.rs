//! WP00 replacement probe — spec 9.5 step 9, 9.7 "Replacement probe",
//! 9.9 `run_replacement_probe`. (Production code lands at WP01
//! `anim_core/src/io/*`; this is the isolated risk-retirement probe.)
//!
//! Proves on this machine:
//!   1. flush-then-replace via Win32 `ReplaceFileW` (same volume: files live
//!      beside the destination, spec 9.6) atomically swaps old → new bytes
//!      with a backup that receives the OLD contents
//!   2. a fault BEFORE commit (new file missing) leaves the destination with
//!      complete OLD bytes (hash-verified)
//!   3. after commit, destination is complete NEW bytes — never mixed
//!      (hash equality, not content inspection)
//!   4. old, new and backup reopen independently and hash-match exactly.

use serde::Serialize;
use std::io::Write;
use std::path::Path;

#[derive(Serialize)]
struct ReplaceProbeReport {
    schema_version: u32,
    capability: String,
    success: bool,
    old_bytes: Option<String>, // hex hashes (blake3)
    new_bytes: Option<String>,
    fault_before_dest_hash: Option<String>,
    after_commit_dest_hash: Option<String>,
    backup_hash: Option<String>,
    diagnostics: Vec<String>,
}

/// Print the full detailed report and exit with the prescribed status.
fn report_exit(report: &ReplaceProbeReport) -> ! {
    println!("{}", serde_json::to_string_pretty(report).unwrap_or_default());
    std::process::exit(if report.success { 0 } else { 1 });
}

const OLD_TAG: u64 = 0xAAAA_1111_2222_3333;
const NEW_TAG: u64 = 0xBBBB_4444_5555_6666;

fn payload(tag: u64) -> Vec<u8> {
    // deterministic, u64-tagged content: mixed bytes are structurally detectable
    let mut v = Vec::with_capacity(65_536);
    for i in 0..65_536 / 8 {
        v.extend_from_slice(&tag.wrapping_add(i as u64).to_le_bytes());
    }
    v
}

fn hash(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

fn write_flush(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Read;
    let mut f = std::fs::File::create(path)
        .map_err(|e| format!("create {}: {e}", path.display()))?;
    f.write_all(bytes).map_err(|e| format!("write: {e}"))?;
    f.sync_all().map_err(|e| format!("flush: {e}"))?;
    drop(f);
    // sanity: written file must reopen with the exact bytes
    let mut back = Vec::new();
    let mut f2 = std::fs::File::open(path).map_err(|e| format!("reopen: {e}"))?;
    f2.read_to_end(&mut back).map_err(|e| format!("read: {e}"))?;
    if back != bytes {
        return Err("written bytes do not reopen identically".into());
    }
    Ok(())
}

fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    let mut s: Vec<u16> = p.as_os_str().encode_wide().collect();
    s.push(0);
    s
}

fn main() {
    let mut report = ReplaceProbeReport {
        schema_version: 1,
        capability: "atomic_replacement".into(),
        success: false,
        old_bytes: None,
        new_bytes: None,
        fault_before_dest_hash: None,
        after_commit_dest_hash: None,
        backup_hash: None,
        diagnostics: Vec::new(),
    };

    let dir = std::env::temp_dir().join("wp00_replace_probe");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        report.diagnostics.push(format!("mkdir: {e}"));
        report_exit(&report);
    }
    for f in ["old.bin", "new.bin", "dest.bin", "backup.bin", "missing.bin"] {
        let _ = std::fs::remove_file(dir.join(f));
    }

    let old = payload(OLD_TAG);
    let new = payload(NEW_TAG);
    report.old_bytes = Some(hash(&old));
    report.new_bytes = Some(hash(&new));

    // same volume: everything beside the destination (spec 9.6)
    let p = |n: &str| dir.join(n);
    for (name, bytes) in [("old.bin", &old), ("new.bin", &new)] {
        if let Err(e) = write_flush(&p(name), bytes) {
            report.diagnostics.push(e);
            report_exit(&report);
        }
    }
    // destination starts as the OLD state
    if let Err(e) = write_flush(&p("dest.bin"), &old) {
        report.diagnostics.push(e);
        report_exit(&report);
    }

    // --- fault BEFORE commit: target file missing → destination keeps OLD bytes
    match unsafe {
        windows::Win32::Storage::FileSystem::ReplaceFileW(
            windows::core::PCWSTR(wide(&p("dest.bin")).as_ptr()),
            windows::core::PCWSTR(wide(&p("missing.bin")).as_ptr()),
            windows::core::PCWSTR::null(),
            windows::Win32::Storage::FileSystem::REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
    } {
        Ok(()) => {
            report
                .diagnostics
                .push("unexpected: replacement succeeded against missing file".into());
            report_exit(&report);
        }
        Err(e) => {
            report.diagnostics.push(format!("fault-before rejected as expected ({e})"))
        }
    }
    match std::fs::read(p("dest.bin")) {
        Ok(bytes) => {
            let h = hash(&bytes);
            report.fault_before_dest_hash = Some(h.clone());
            if bytes != old {
                report
                    .diagnostics
                    .push(format!("failure: fault-before corrupted destination ({h})"));
                report_exit(&report);
            }
        }
        Err(e) => {
            report.diagnostics.push(format!("reopen dest after fault: {e}"));
            report_exit(&report);
        }
    }

    // --- commit: atomic replace with a backup receiving the old contents
    match unsafe {
        windows::Win32::Storage::FileSystem::ReplaceFileW(
            windows::core::PCWSTR(wide(&p("dest.bin")).as_ptr()),
            windows::core::PCWSTR(wide(&p("new.bin")).as_ptr()),
            windows::core::PCWSTR(wide(&p("backup.bin")).as_ptr()),
            windows::Win32::Storage::FileSystem::REPLACE_FILE_FLAGS(0),
            None,
            None,
        )
    } {
        Ok(()) => {}
        Err(e) => {
            report.diagnostics.push(format!("replace failed: {e}"));
            report_exit(&report);
        }
    }

    // reopen destination and backup independently; hashes verify completeness
    let dest = match std::fs::read(p("dest.bin")) {
        Ok(b) => b,
        Err(e) => {
            report.diagnostics.push(format!("reopen dest: {e}"));
            report_exit(&report);
        }
    };
    let backup = match std::fs::read(p("backup.bin")) {
        Ok(b) => b,
        Err(e) => {
            report.diagnostics.push(format!("reopen backup: {e}"));
            report_exit(&report);
        }
    };
    let dest_h = hash(&dest);
    let backup_h = hash(&backup);
    report.after_commit_dest_hash = Some(dest_h.clone());
    report.backup_hash = Some(backup_h.clone());

    report.success = dest == new && backup == old;
    if dest != new {
        report
            .diagnostics
            .push(format!("failure: destination is neither old nor new intact ({dest_h})"));
    }
    if backup != old {
        report
            .diagnostics
            .push(format!("failure: backup is not the complete old state ({backup_h})"));
    }
    if report.success {
        report.diagnostics.push("atomic replacement verified end to end".into());
    }
    println!("{}", serde_json::to_string_pretty(&report).unwrap_or_default());
    std::process::exit(if report.success { 0 } else { 1 });
}
