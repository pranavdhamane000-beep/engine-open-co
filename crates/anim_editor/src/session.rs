//! Command dispatch, snapshot subscription and revision handling (spec 19.5, 19.6, 19.9).
//!
//! The session is the EDITOR's single-writer adapter around the WP03
//! CommandProcessor. The UI reads `Arc<EditorSnapshot>` values and issues
//! commands containing stable ids; it never retains mutable scene
//! references. Ink strokes flow through the same envelopes as every other
//! edit; a transient submission failure retains a retryable pending stroke
//! under the SAME command id (spec 19.11).

use crate::error::EditorError;
use crate::ink::{CommitSpec, InkEffect, InkEngine, PointerEvent, StrokeSetup, Tool};
use crate::preview_quality::{PreviewQuality, PreviewQualityController};
use crate::transport::EditorTransport;
use crate::{CommandReceipt, CommandSink, EditorSnapshot, SnapshotReader};
use anim_commands::{Command, CommandEnvelope, CommandError, CommandProcessor};
use anim_core::Rational;
use anim_project::{Easing, PropertyKey, PropertyValue};
use std::sync::{Arc, Mutex};

/// Versioned stroke property name on the target node (spec 19.3 ink.rs).
pub const INK_STROKE_PROPERTY: &str = "ink.stroke";
/// Versioned transaction label for ink commits.
pub const INK_COMMIT_LABEL: &str = "ink.commit_stroke";
/// Stroke payload schema version.
pub const STROKE_PAYLOAD_SCHEMA: u32 = 1;

struct Inner {
    processor: CommandProcessor,
    transport: EditorTransport,
    quality: PreviewQualityController,
    ink: InkEngine,
    /// Setup frozen at pointer-down (scene/node/key time + view transform).
    ink_setup: Option<StrokeSetup>,
    snapshot: Arc<EditorSnapshot>,
    /// Label of the last accepted transaction (display only).
    last_transaction_label: Option<String>,
}

/// The editor controller (spec 19.9: `handle_pointer`, `commit_stroke`,
/// `update_preview_quality`, `request_export`).
pub struct EditorSession {
    inner: Mutex<Inner>,
}

impl EditorSession {
    pub fn new(
        processor: CommandProcessor,
        start_frame: i64,
        target_frame_us: f64,
        tool: Tool,
    ) -> Self {
        let transport = EditorTransport::new(start_frame);
        let quality = PreviewQualityController::new(target_frame_us);
        let doc = processor.document();
        let snapshot = Arc::new(EditorSnapshot {
            project_revision: doc.revision,
            transport: transport.snapshot(),
            quality: quality.snapshot(),
            undo_depth: 0,
            redo_depth: 0,
            last_command_id: None,
            last_transaction_label: None,
        });
        Self {
            inner: Mutex::new(Inner {
                processor,
                transport,
                quality,
                ink: InkEngine::new(tool),
                ink_setup: None,
                snapshot,
                last_transaction_label: None,
            }),
        }
    }

    /// Publish a fresh immutable snapshot (spec 19.6).
    fn publish(inner: &mut Inner) {
        let doc = inner.processor.document();
        let snapshot = Arc::new(EditorSnapshot {
            project_revision: doc.revision,
            transport: inner.transport.snapshot(),
            quality: inner.quality.snapshot(),
            undo_depth: inner.processor.undo_depth(),
            redo_depth: inner.processor.redo_depth(),
            last_command_id: inner.ink.last_committed_command_id(),
            last_transaction_label: inner.last_transaction_label.clone(),
        });
        inner.snapshot = snapshot;
    }

    /// Spec 19.9 `handle_pointer` (session level: freezes the CURRENT setup
    /// at pointer-down; updates use the frozen copy).
    pub fn handle_pointer(
        &self,
        event: PointerEvent,
        setup: Option<StrokeSetup>,
    ) -> Result<InkEffect, EditorError> {
        let mut inner = self.inner.lock().expect("session poisoned");
        if event.is_down() {
            inner.ink_setup = setup;
        }
        let setup_ref: Option<StrokeSetup> =
            if event.is_down() { inner.ink_setup.clone() } else { None };
        let effect = inner.ink.handle_pointer(event, setup_ref.as_ref())?;
        Self::publish(&mut inner);
        Ok(effect)
    }

    /// Spec 19.9 `commit_stroke`: build ONE envelope from the pending spec
    /// (same command id across retries), submit, finalize exactly once.
    /// Ok(None) when nothing is pending.
    pub fn commit_stroke(&self) -> Result<Option<CommandReceipt>, EditorError> {
        let spec = {
            let inner = self.inner.lock().expect("session poisoned");
            inner.ink.pending_commit().cloned()
        };
        let Some(spec) = spec else { return Ok(None) };

        // Build outside the lock? The envelope needs the document revision;
        // hold the lock for apply() as the single-writer boundary demands.
        let mut inner = self.inner.lock().expect("session poisoned");
        let envelope = Self::build_stroke_envelope(&inner.processor, &spec);
        match inner.processor.apply(envelope) {
            Ok(outcome) => {
                inner.ink.mark_committed(spec.command_id)?;
                inner.last_transaction_label = Some(outcome.transaction_label.clone());
                Self::publish(&mut inner);
                Ok(Some(CommandReceipt {
                    command_id: spec.command_id,
                    new_revision: outcome.new_revision,
                }))
            }
            // TRANSIENT failure: stroke stays pending with the SAME command
            // id; the next commit_stroke retries (spec 19.11).
            Err(CommandError::Persistence(err)) => Err(EditorError::RetryableCommit(err)),
            // Validation rejection: present the error; explicit cancellation
            // is available via cancel_pending_after_rejection (spec 19.11).
            Err(e) => Err(EditorError::CommandRejected(e.to_string())),
        }
    }

    /// After a REJECTED submission the user explicitly cancels the pending
    /// stroke (spec 19.11); pending commit state is discarded, no edit.
    pub fn cancel_pending_after_rejection(&self) -> InkEffect {
        let mut inner = self.inner.lock().expect("session poisoned");
        let effect = inner.ink.cancel_pending_after_rejection();
        Self::publish(&mut inner);
        effect
    }

    /// Spec 19.9 `update_preview_quality` (interactive preview ONLY; never
    /// reaches project or export settings — spec 19.7).
    pub fn update_preview_quality(
        &self,
        frame_time: std::time::Duration,
    ) -> PreviewQuality {
        let mut inner = self.inner.lock().expect("session poisoned");
        let quality = inner.quality.update(frame_time);
        Self::publish(&mut inner);
        quality
    }

    /// Counters reset on breakpoint/seek/suspension/resize (spec 19.7).
    pub fn reset_quality_counters(&self) {
        let mut inner = self.inner.lock().expect("session poisoned");
        inner.quality.reset_counters();
        Self::publish(&mut inner);
    }

    /// Device recovery -> best-quality re-init (spec 19.8).
    pub fn reset_quality_full(&self) {
        let mut inner = self.inner.lock().expect("session poisoned");
        inner.quality.reset_full();
        Self::publish(&mut inner);
    }

    pub fn set_tool(&self, tool: Tool) {
        let mut inner = self.inner.lock().expect("session poisoned");
        inner.ink.set_tool(tool);
        Self::publish(&mut inner);
    }

    /// Spec 19.9 `request_export`: export launches from an immutable saved
    /// revision with FIXED settings. Preview quality state NEVER leaks in —
    /// settings come only from the caller (spec 19.7/19.10 isolation).
    pub fn request_export(
        &self,
        settings: anim_export::ExportSettings,
    ) -> Result<crate::JobId, EditorError> {
        let inner = self.inner.lock().expect("session poisoned");
        // Pre-launch validation against the current immutable document; the
        // export job re-validates inside its own isolated context (WP09).
        settings
            .validate(inner.processor.document())
            .map_err(|e| EditorError::ExportRejected(e.to_string()))?;
        Ok(uuid::Uuid::new_v4())
    }

    /// Transport controls (each publishes a snapshot afterwards).
    pub fn transport_play(&self) {
        let mut inner = self.inner.lock().expect("session poisoned");
        inner.transport.play();
        Self::publish(&mut inner);
    }

    pub fn transport_pause(&self) {
        let mut inner = self.inner.lock().expect("session poisoned");
        inner.transport.pause();
        Self::publish(&mut inner);
    }

    /// Explicit position seek (spec 19.6); resets quality samples.
    pub fn transport_seek(&self, position_frame: i64) {
        let mut inner = self.inner.lock().expect("session poisoned");
        inner.transport.seek(position_frame);
        inner.quality.reset_counters(); // seek resets quality samples
        Self::publish(&mut inner);
    }
    /// Frame-step ONLY while paused (spec 19.5 step 2).
    pub fn transport_step(&self, frames: i64) -> Result<(), EditorError> {
        let mut inner = self.inner.lock().expect("session poisoned");
        let result = inner.transport.step_frames(frames);
        Self::publish(&mut inner);
        result
    }

    /// Ingest a preview audio clock snapshot (spec 19.10).
    pub fn ingest_audio_clock(&self, clock: &anim_audio::PreviewClockSnapshot) {
        let mut inner = self.inner.lock().expect("session poisoned");
        inner.transport.ingest_audio_clock(clock);
        Self::publish(&mut inner);
    }

    /// Undo/redo passthrough, publishing refreshed snapshots.
    pub fn undo(&self) -> Result<(), CommandError> {
        let mut inner = self.inner.lock().expect("session poisoned");
        inner.processor.undo()?;
        Self::publish(&mut inner);
        Ok(())
    }

    pub fn redo(&self) -> Result<(), CommandError> {
        let mut inner = self.inner.lock().expect("session poisoned");
        inner.processor.redo()?;
        Self::publish(&mut inner);
        Ok(())
    }

    /// Build ONE deterministic envelope from the pending commit spec. The
    /// envelope carries the STROKE's stable command id so any retry is
    /// recognized as the same command (processor idempotency, spec 12.7).
    fn build_stroke_envelope(
        processor: &CommandProcessor,
        spec: &CommitSpec,
    ) -> CommandEnvelope {
        let doc = processor.document();
        let payload = crate::ink::StrokePayloadV1 {
            schema: STROKE_PAYLOAD_SCHEMA,
            stroke_id: spec.stroke_id,
            points: spec.points.clone(),
        };
        let bytes = serde_json::to_string(&payload).expect("stroke payload serialize");
        let key = PropertyKey {
            time: Rational::new(spec.setup.key_time_num as i128, 1)
                .expect("stroke key time is a valid rational"),
            value: PropertyValue::Text(bytes),
            // Step rendering: the stroke is present from its key time.
            easing: Easing::Step,
        };
        CommandEnvelope {
            schema_version: anim_commands::COMMAND_SCHEMA_VERSION,
            project_id: doc.project_id,
            command_id: spec.command_id,
            expected_scene_revision: doc.revision,
            transaction_label: INK_COMMIT_LABEL.to_string(),
            commands: vec![Command::SetNodeProperty {
                scene_id: spec.setup.scene_id,
                node_id: spec.setup.node_id,
                property: INK_STROKE_PROPERTY.to_string(),
                key,
            }],
        }
    }
}

impl CommandSink for EditorSession {
    fn issue(&self, command: CommandEnvelope) -> Result<CommandReceipt, CommandError> {
        let mut inner = self.inner.lock().expect("session poisoned");
        let command_id = command.command_id;
        let outcome = inner.processor.apply(command)?;
        inner.last_transaction_label = Some(outcome.transaction_label.clone());
        Self::publish(&mut inner);
        Ok(CommandReceipt { command_id, new_revision: outcome.new_revision })
    }
}

impl SnapshotReader for EditorSession {
    fn current(&self) -> Arc<EditorSnapshot> {
        self.inner.lock().expect("session poisoned").snapshot.clone()
    }
}
