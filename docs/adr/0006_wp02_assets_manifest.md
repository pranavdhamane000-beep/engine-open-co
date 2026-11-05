# ADR-0006 — WP02 immutable assets and content manifest decisions

**Status:** Accepted · **Date:** 2026-11-05 ·
**Evidence:** `crates/anim_assets/tests/store.rs` (18 tests), full workspace
gates fmt=0 / clippy(-D warnings)=0 / test --locked=0. Manifest digest domain
`anim_assets_manifest_v1`; BLAKE3("") pinned to the published vector
`af1349b9…e41f3262`.

## Context

WP02 (spec §11) is the content-addressed asset store. The spec fixes the
interfaces (hash, manifest, store, import, resolver, decode) but leaves
commit mechanics, digest field order, decoder registration scope, and
diagnostic semantics to implementation. These are the load-bearing choices.

## Decisions

1. **Object commit = `MoveFileExW` WITHOUT `REPLACE_EXISTING`, not
   `ReplaceFileW`.** Objects are immutable content-addressed blobs whose
   destination path is derived from the just-verified content hash, so a
   semantic "replace old version" (WP01's `ReplaceFileW` path, correct for
   named project documents) is the wrong primitive: two racing imports of
   the SAME content race onto the SAME path, and overwrite-on-collision
   could clobber a concurrent commit mid-flight. Non-overwriting rename +
   `MOVEFILE_WRITE_THROUGH` makes commit atomic and idempotent: the loser
   of a race sees ERROR_ALREADY_EXISTS and takes the dedup path (verify
   existing length+hash, then discard its temp). Parent directories are
   created BEFORE the rename attempt (spec 11.7 "create parent directories
   and rename") — the initial test run proved the failure mode (0x80070003
   on a fresh two-letter bucket).
2. **Dedup requires verification, never trust.** When the destination
   already exists the temp file is discarded ONLY after streaming-verify of
   the existing object (size then hash). A test pins: wrong bytes at the
   object path ⇒ typed rejection, never silent reuse. Reservation status
   codes treated as "destination exists": 0x800700B7, 0x80070050,
   0x80070005.
3. **`AssetError::Decode(String)` added beyond the spec's 10 variants**
   (spec deviation, documented). Spec §11.11 lists no variant for
   corrupt/undecodable CONTENT (only UnsupportedDecoder for a missing
   decoder), but a registered decoder facing garbage bytes needs a typed
   error distinct from Io. `Cancelled` is defined per spec but unassigned —
   reserved for future long-running operations.
4. **MVP decoder coverage = PNG (RGBA-8 straight) + WAV (i16
   interleaved)**, both registered under `IMPORTER_VERSION = 1`. Everything
   else errors `UnsupportedDecoder(kind, version)` — never a guessed
   decoder. PNG alpha detection parses the IHDR color-type byte directly
   (offset 25 ∈ {4, 6}) rather than a pixel scan, so a fully-opaque RGBA
   image still reports `has_alpha = true`; indexed palettes with tRNS are a
   documented limitation (not counted as alpha). WAV decode mirrors the
   WP00 media-probe symphonia path proven on this machine (ADR-0004); actual
   decoded frame counts are authoritative, not container lengths.
5. **Manifest digest field order is fixed by this ADR:** domain string
   (length-prefixed) → schema version → record count → per record
   (length-prefixed field groups in declaration order: asset_id,
   content_hash, byte_length, kind, importer_version, dependencies
   (count + ids, sorted), display_name). Records live in a `BTreeMap`
   keyed by AssetId canonical bytes, so insertion order can never change
   the digest (test-pinned under shuffled insertion). Changing any of this
   requires a schema-version bump, not an ad-hoc edit.
6. **Duplicate dependencies are REJECTED at entry construction** even
   though the list is internally sorted (a sorted dup would silently
   double-count edges). Self-dependency is rejected as a cycle at
   construction; cross-record cycles are rejected at `insert` by DFS over
   the hypothetical post-insert graph, and a rejected insert leaves the
   manifest untouched (test-pinned). Forward dependencies (id not yet in
   the manifest) are legal.
7. **Resolver `diagnostics()` reports VERIFIED state**, not existence.
   Spec 11.7 allows preview from unverified resolution but requires
   mismatched content to be reported; a corrupt object showing as "ok"
   because the file merely exists would be exactly the silent substitution
   the spec forbids. `resolve` = record + path only (cheap, preview);
   `resolve_verified` / `verify_all` stream-verify (export gate, stops at
   first typed failure). Missing record (MissingAssetId) vs missing object
   (MissingObject) are distinct typed errors — test-pinned.
8. **BLAKE3 published-vector pinning approach:** the empty-input digest is
   pinned verbatim in our suite; the full published vector set is exercised
   by the blake3 crate's own release tests. Our added value is proving
   STREAMING == one-shot across chunk boundaries (1 MiB trickle at 4 KiB
   reads vs the 64 KiB internal buffer, hash equality test-pinned). Initial
   memorized vector tails were wrong and corrected against the computed
   library value — machine-checked, never trusted by memory.
9. **Memory discipline:** imports stream through a fixed 64 KiB chunk
   (spec 11.12 "larger than memory buffers"); no path ever maps a whole
   object. Byte counts use `checked_add` everywhere — an overflow is a
   typed Io error, never a wrap.
