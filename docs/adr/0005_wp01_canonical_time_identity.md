# ADR-0005 — WP01 canonical time model and stable identity decisions

**Status:** Accepted · **Date:** 2026-11-05 ·
**Evidence:** `crates/anim_core/tests/time_properties.rs` (19 tests, 5 property
suites × 512 cases), `crates/anim_core/src/*` unit tests (13), gates
fmt/clippy(-D warnings)/test --locked all exit 0.

## Context

WP01 (spec §10) is the substrate for every persisted document. The spec
fixes the interface shape but leaves encoding details and constructor
semantics to implementation. These are the load-bearing choices.

## Decisions

1. **`Rational` internals.** `i128 (num, den)` storage with den > 0, reduced
   by gcd, zero = 0/1. Cross-type arithmetic goes through public
   `Rational::from_bigints(BigInt, BigInt)` — reductions happen exactly at
   arbitrary precision, then an unchecked-range-checked convert to i128
   errors with `ArithmeticOverflow`. `floor_to_i64` uses `div_euclid`
   (floors for positive den). `ceil_to_i64` = checked-negate + floor;
   i128::MIN is an explicit overflow error instead of a debug panic.
2. **`FramePos` construction has two paths, deliberately.**
   `FramePos::new` CARRIES the subframe's whole part into the frame
   (normalizing, used by internal math — e.g. subframe 4/3 at frame 7
   becomes frame 8 + 1/3). Deserialization REQUIRES canonical [0,1) and
   rejects with `SubframeOutOfRange` — persisted data is never silently
   rewritten (spec 10.11 "never clamp"). Invariants mean the fields stay
   private; `frame()` / `subframe()` accessors are the stable API.
3. **`FrameRate.num/den` stay public** per spec 10.8, so struct-literal
   bypass cannot produce a crash, every method re-validates and returns
   `NonPositiveRate` explicitly. Test `invalid_rate_fails_explicitly`
   pins this contract (fields pub yet every call-site checked).
4. **Canonical byte encoding = fixed-width little-endian fields in
   declaration order**, no padding; variable-length data (String) is the
   only length-prefixed case. IDs hash their 16 `Uuid::as_bytes`
   (big-endian canonical) form. Hash domains are length-prefixed strings
   (`domain_hash`) so concatenation can never collide ("ab"+"c" proven
   distinct from "a"+"bc" by unit test).
5. **Typed IDs:** `Display` is the BARE hyphenated UUID, identical to what
   `FromStr` accepts, so `to_string().parse()` round trips — the earlier
   `node(uuid)` debug format broke that and is corrected.
6. **`Revision`** starts at 0 (`Revision::FIRST`), increments via
   `checked_add` only; u64 wraparound returns `ArithmeticOverflow`
   (test-pinned). No wrapping operations exist anywhere in the crate.
7. **Error variant addition (spec deviation, intentional):** `CoreError::Io`
   stores the rendered message string (`std::io::Error` itself is not
   Eq/Clone, and error equality is required by undo/diff tests to come).
   The 6 spec-listed variants are preserved verbatim. `serde` for IDs is
   the bare-hyphenated UUID string, exactly matching text form.
8. **`io::write_file_atomically`** implements the probe-proven sequence
   (ADR-0004): same-volume temp file → `sync_all` → `ReplaceFileW` with
   backup → reopen-verify destination bytes == payload; pre-commit failure
   removes our temp and leaves the destination untouched. Reopen-verify is
   part of the production function, not just the probe.

## Validation record (spec 10.12)

- Normalization: 512-case proptest, den > 0 + gcd == 1.
- Arithmetic vs arbitrary-precision reference: mul proptest + floor/ceil
  partition proptest.
- Negative floor at exact boundaries: -8/24-borne cases at frame -9/-8
  pinned.
- Adjacent sample-range partitioning: **10,001 adjacent frames at
  24000/1001 and 30000/1001 at 48 kHz, and 24000/1001 at 44.1 kHz** —
  every interval boundary equals the independent BigInt reference
  (`floor(f * sr * den / num)`), every sample occurs exactly once, zero
  accumulated drift (test-asserted per frame and in total coverage).
- NTSC rates as rationals (never decimals): 1001/30000 s and 5005/24000 s
  exact.
- UUID round trips (serde + text + canonical bytes) and stable byte order.
- Revision overflow fails explicitly.
- G1 overflow/invalid-rate tests fail explicitly (`invalid_rate_…`,
  `revision_overflow…`).

## Consequences

- Downstream crates (WP02+ schemas, project save/load, audio intervals,
  sim ticks) must import THESE types — no local seconds/timestamps/IDs.
- Any change to `Canonical` encodings is a document-schema version bump,
  never a silent hash break.
