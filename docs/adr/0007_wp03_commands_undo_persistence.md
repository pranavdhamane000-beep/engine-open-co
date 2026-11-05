# ADR-0007 — WP03 commands, undo, and atomic project persistence decisions

**Status:** Accepted · **Date:** 2026-11-05 ·
**Evidence:** `crates/anim_commands/tests/atomicity.rs` (12 tests),
`crates/anim_project/tests/persistence.rs` (10 tests),
`crates/anim_commands/examples/phase1_demo.rs` (PHASE 1 EXIT gate —
save→hash→reopen→hash identical, printed live on this machine:
`6c0d1762fa08abfc…`), workspace gates fmt=0 / clippy(-D)=0 / test --locked=0.

## Context

WP03 (spec §12) is the single mutation boundary. The spec fixes envelope
shape, the revision-check ORDER, the clone-commit algorithm and the
atomic-save sequence; implementation freedom remained in command surface,
inverse encoding, recovery naming, and validation depth. Load-bearing
choices:

## Decisions

1. **Envelope-check order is spec §12.7 verbatim** with ONE interpretation
   spelled out: command-ID replay happens SECOND (before project/revision
   checks), so an exact duplicate accepted earlier returns its ORIGINAL
   result even after the document has advanced — the replay path must not
   be masked by a later revision conflict. A same-ID envelope with ANY
   content difference (even the label) is `CommandReplayMismatch`
   (test-pinned both ways).
2. **Undo/redo are transport positions, not new transactions.** A forward
   commit pushes to the undo stack and clears redo; `undo()` pops the top,
   applies inverses in REVERSE order, commits a new revision, and the SAME
   `AppliedTransaction` moves to the redo stack; `redo()` reapplies forward
   in original order and moves it back. Undo/redo pushes do NOT clear redo
   (only genuine new forward edits do). Undo committing a new revision is
   per spec §12.6 — so equality proofs are CONTENT-level (revision field
   compares unequal BY DESIGN after undo).
3. **Inverses are first-class serializable commands** (`RestoreSceneData`,
   `RestoreNodeData` added beyond the naively expected set): deleting a
   scene/object must restore it VERBATIM in one undo step. Their `apply`
   re-inserts the stored data; the returned removal is discarded by the
   undo machinery. Restore tags are deliberately NOT in the external
   registry dispatch (automation cannot reach inverse-only payloads
   directly; they arrive only through history application).
4. **Full-document validation runs after EVERY batch commit direction**
   (apply, undo, redo) — not just per-c_command checks. Cross-command
   corruption (a parent cycle only visible after several commands, dangling
   asset refs) is caught BEFORE the swap; a rejected batch/vs-direction
   never mutates. `execute_on_clone` in dry_run.rs is the SINGLE execution
   path shared by dry-run and apply (spec §12.6 validate-then-apply race
   closed), and its success implies apply's success at the same revision.
5. **`AssetManifestEntry::validate()` added to anim_assets** (documented
   WP02 addition): Deserialize bypasses the checked constructor, and the
   document load path must re-prove dependency invariants on migrated or
   handwritten JSON. Command-layer asset payloads are additionally
   REBUILT through the validated constructor (`AssetManifestEntry::new`)
   — deserialized fields never trust the wire.
6. **Asset reference integrity rules:** a node may reference an asset only
   if its manifest record already exists (payload persisted BEFORE the
   manifest command, spec §12.2); `RemoveAsset` refuses while any node
   references it; `RemoveNode` refuses while children/tracks target it —
   all typed Validation errors, no silent cascade. Cycles through UpsertNode
   re-parenting are rejected by ancestor-walk at apply time.
7. **Autosave format saves wall-clock OUTSIDE the document** (an
   `AutosaveEnvelope` wrapper). The authoritative document keeps NO
   session/editor/timestamp state (spec §12.8), so the document hash stays
   G3-reproducible; autosave serialization wraps an otherwise unmodified
   validated document. Superseded autosave backups are discarded (autosave
   lineage is not the manual-save backup lineage).
8. **Recovery selection:** valid candidates only (schema-valid + complete +
   matching project id + recorded revision). Rule: the highest-revision
   valid manual candidate (current > backup on ties) is selected; an
   autosave newer than every manual candidate yields
   `RecoveryDecision::Ambiguous` — the CHOICE is presented, never
   auto-resolved; an autosave ≤ best-manual never wins. Scan debris rules:
   only `<name>`, `<name>.<uuid>.old.tmp`, `<name>.autosave.json` are
   candidates; staging `.tmp` files of a killed process are invisible to
   recovery and parsed errors preserve every file for diagnosis.
9. **Save-stage fault injection is a first-class internal surface**
   (`save_atomic_staged(dest, SaveStage::{Serialized, TempWritten})`),
   documented as an OPS/test tool: each stop reproduces the filesystem
   state a process killed at that stage leaves (no cleanup, old dest
   complete, staging debris present). True child-process kills are the same
   observable states; tests assert recovery + next-save behavior OVER the
   crash debris (spec §12.12 kill-stage requirement satisfied by
   stage-simulation).
10. **`FrameRate`/`FrameRange` gained serde in anim_core** (WP01
    extension): document JSON needs them; encoding matches Rational's
    canonical "num/den" text and start/end objects. `Rational` gained
    `Ord` (BigInt cross-multiply — exact, never wrapping) + `Display`
    mirroring `Deserialize` ("num/den"), needed for property-key ordering
    and typed error text. No WP01 behavior changed; additions only.
11. **One revision increment per batch proven** (before/after revision,
    not ++per-command); `RevisionOverflow` is checked in OVERRIDE order
    (before cloning) so capacity exhaustion is a typed early rejection.
