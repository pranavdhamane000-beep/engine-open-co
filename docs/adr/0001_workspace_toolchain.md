# ADR-0001 — Workspace layout, toolchain pin, and dependency policy

**Status:** Accepted · **Date:** 2026-11-04 · **Gate:** Phase 1.1 PASS (STATUS.md)

## Context

Spec §4.1 mandates a 13-crate dependency-ordered workspace; §5.1 mandates a
pinned compiler profile; §9.5 step 2 mandates locked dependencies and
warnings-as-errors; §9.12 fixes the validation gate commands.

## Decision

1. **Layout:** all crates under `crates/<name>/` per spec §9.3 exact paths;
   workspace root declares members as quoted `crates/<name>` entries.
2. **Toolchain:** `rust-toolchain.toml` pins channel `1.99.1` with
   `rustfmt` + `clippy` (minimal profile). Toolchain upgrades are gated
   changes (D-08): ADR + revalidation of all probe reports.
3. **Lockfile:** `Cargo.lock` is committed authority (spec §9.5 step 2);
   tests run with `--locked` (§9.12).
4. **Lint baseline:** `-D warnings` via crate atttributes
   `#![deny(clippy::all)]` / per-crate allows only with a written
   justification comment (e.g. `clippy::multiple_crate_versions` for the
   wgpu/egui ecosystems, logged for revisit at WP11).
   `clippy.toml` is NOT used — its schema rejects lint-name table sections
   (learned in Phase 1.1; scaffold generator updated to never emit it).
5. **Build cache:** `target-dir` lives outside OneDrive via committed
   `.cargo/config.toml` (D-09). First wgpu build: 1 min 36 s warm, cached.
6. **Validation gate (final, matching spec §9.12):**
   `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace --locked`.

## Consequences

- Empty-crate baseline passes all three gates (log in STATUS.md gate log).
- Every future WP starts from this baseline; gates re-run at every
  checkpoint; failures block the checkpoint.
