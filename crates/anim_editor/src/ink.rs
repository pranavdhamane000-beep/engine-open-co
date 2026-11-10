//! Ink stroke model, normalization and commit state machine (spec 19.6-19.8).
//!
//! State machine: Idle -> Capturing -> CommitPending -> Committed -> Idle;
//! Capturing/CommitPending may become Cancelled. A stroke freezes its view
//! transform at pointer-down, captures coalesced points oldest to newest and
//! displays as an uncommitted overlay. Only valid primary pen contact
//! completion issues ONE command; duplicate up, synthesized mouse input and
//! redraw never commit again. After commit the stroke renders from project
//! snapshots, never from the transient overlay. The engine owns NO document:
//! the session builds the envelope from the pending commit spec.

use crate::error::EditorError;
use anim_core::{CommandId, NodeId, SceneId};

/// Hard cap (spec 19.7: a configured maximum; never unbounded allocation).
pub const MAX_STROKE_POINTS: u32 = 4_096;

/// Raw history point exactly as delivered by the OS (client space).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawPoint {
    pub client_x: f32,
    pub client_y: f32,
    pub pressure: f32,
    pub tilt_x: i16,
    pub tilt_y: i16,
    pub timestamp_us: u64,
}

/// Spec 19.8 `PenPoint` (validated, converted, frozen in project space).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PenPoint {
    pub client_x: f32,
    pub client_y: f32,
    pub scene_x: f32,
    pub scene_y: f32,
    pub pressure: f32,
    pub tilt_x: i16,
    pub tilt_y: i16,
    pub timestamp_us: u64,
}

/// Frozen per-stroke context (spec 19.6: stroke ids and view transform are
/// immutable after start).
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeSetup {
    /// Client -> scene affine; frozen at pointer-down.
    pub view_scale: f32,
    pub view_offset_x: f32,
    pub view_offset_y: f32,
    /// Commit target and key time (scene), frozen at pointer-down.
    pub scene_id: SceneId,
    pub node_id: NodeId,
    /// Exact rational key time numerator (denominator 1).
    pub key_time_num: i64,
}

impl StrokeSetup {
    /// Client -> scene. Deterministic f32 math; finiteness follows.
    pub fn to_scene(&self, x: f32, y: f32) -> (f32, f32) {
        (x * self.view_scale + self.view_offset_x, y * self.view_scale + self.view_offset_y)
    }
}

/// Spec 19.8 `StrokeState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrokeState {
    Idle,
    Capturing,
    CommitPending,
    Committed,
    Cancelled,
}

/// Spec 19.8 `ActiveStroke`.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveStroke {
    pub stroke_id: NodeId,
    pub command_id: CommandId,
    pub pointer_id: u32,
    pub setup: StrokeSetup,
    pub points: Vec<PenPoint>,
    pub state: StrokeState,
}

/// Incoming event surface (spec 19.6, 19.9). `history` carries coalesced
/// points ORDERED OLDEST TO NEWEST; the newest is authoritative.
#[derive(Debug, Clone, PartialEq)]
pub struct PointerEvent {
    pub kind: PointerEventKind,
    pub pointer_id: u32,
    pub is_primary: bool,
    pub is_eraser: bool,
    pub is_mouse_synthesized: bool,
    pub client_x: f32,
    pub client_y: f32,
    pub pressure: f32,
    pub tilt_x: i16,
    pub tilt_y: i16,
    pub timestamp_us: u64,
    pub history: Vec<RawPoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerEventKind {
    Down,
    Update,
    Up,
    Cancel,
    CaptureLost,
    FocusLost,
    Hover,
}

impl PointerEvent {
    pub fn is_down(&self) -> bool {
        self.kind == PointerEventKind::Down
    }
}

/// Active tool (spec 19.7: eraser input selects an explicit eraser path,
/// never a drawing stroke).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Disabled,
    Pen,
    Eraser,
}

/// Engine output per event.
#[derive(Debug, Clone, PartialEq)]
pub enum InkEffect {
    /// Event ignored (wrong tool, synthesized input, hover over idle, ...).
    Ignored,
    /// A stroke started; caller should take pointer capture + show overlay.
    Captured { stroke_id: NodeId },
    /// Overlay points updated (no project mutation).
    OverlayUpdated { point_count: u32 },
    /// Final point appended; envelope is available via `pending_commit`.
    StrokeUp { stroke_id: NodeId, command_id: CommandId },
    /// Exactly-once finalize accepted; overlay dropped, no double rendering.
    Committed { command_id: CommandId },
    /// Command validation rejected; stroke stays pending explicit cancel.
    CommitRejected { error: String },
    /// Capture-loss/cancel/focus-loss; transient state discarded, no edit.
    Cancelled,
    /// Eraser input routed to the explicit eraser path (no drawing stroke).
    EraserCommandIssued,
}

/// The exactly-once commit spec produced on pointer-up; the session builds
/// the durable envelope FROM THIS, reusing the SAME command id on retries.
#[derive(Debug, Clone, PartialEq)]
pub struct CommitSpec {
    pub stroke_id: NodeId,
    pub command_id: CommandId,
    pub setup: StrokeSetup,
    pub points: Vec<PenPoint>,
}

/// Committed command payload (versioned; deterministic serialization).
#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
pub struct StrokePayloadV1 {
    pub schema: u32,
    pub stroke_id: NodeId,
    pub points: Vec<PenPoint>,
}

/// The ink engine (single owner of the active stroke and pending commit).
#[derive(Debug, Clone)]
pub struct InkEngine {
    tool: Tool,
    stroke: Option<ActiveStroke>,
    pending: Option<CommitSpec>,
    last_committed: Option<CommandId>,
    max_points: u32,
}

impl InkEngine {
    pub fn new(tool: Tool) -> Self {
        Self {
            tool,
            stroke: None,
            pending: None,
            last_committed: None,
            max_points: MAX_STROKE_POINTS,
        }
    }

    pub fn set_tool(&mut self, tool: Tool) {
        self.tool = tool;
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    pub fn active_state(&self) -> StrokeState {
        self.stroke.as_ref().map(|s| s.state).unwrap_or(StrokeState::Idle)
    }

    pub fn active_stroke(&self) -> Option<&ActiveStroke> {
        self.stroke.as_ref()
    }

    /// Pending commit spec, if any (retries reuse it verbatim).
    pub fn pending_commit(&self) -> Option<&CommitSpec> {
        self.pending.as_ref()
    }

    pub fn last_committed_command_id(&self) -> Option<CommandId> {
        self.last_committed
    }

    fn clear(&mut self) {
        self.stroke = None;
        self.pending = None;
    }

    /// Spec 19.9 `handle_pointer`. Malformed events are REJECTED typed
    /// without state change (never crash the event loop). On Down the
    /// caller MUST pass the frozen setup; on other kinds it is ignored.
    pub fn handle_pointer(
        &mut self,
        event: PointerEvent,
        setup: Option<&StrokeSetup>,
    ) -> Result<InkEffect, EditorError> {
        match event.kind {
            PointerEventKind::Down => self.begin(event, setup),
            PointerEventKind::Update | PointerEventKind::Hover => self.update(event),
            PointerEventKind::Up => self.finish(event),
            PointerEventKind::Cancel
            | PointerEventKind::CaptureLost
            | PointerEventKind::FocusLost => self.cancel(),
        }
    }

    fn begin(
        &mut self,
        event: PointerEvent,
        setup: Option<&StrokeSetup>,
    ) -> Result<InkEffect, EditorError> {
        if self.tool == Tool::Disabled {
            return Ok(InkEffect::Ignored);
        }
        // Pen tool ignores synthesized mouse (the digitizer also feeds the
        // legacy mouse channel) and non-primary contacts (spec 19.7).
        if event.is_mouse_synthesized || !event.is_primary {
            return Ok(InkEffect::Ignored);
        }
        // One active stroke at a time.
        if self.stroke.is_some() {
            return Err(EditorError::WrongState { state: StrokeState::Capturing });
        }
        let setup = setup.ok_or(EditorError::WrongState { state: StrokeState::Idle })?;

        if self.tool == Tool::Eraser {
            // Eraser input selects an explicit eraser command, not a stroke.
            return Ok(InkEffect::EraserCommandIssued);
        }

        let point = Self::validate_and_convert(
            setup,
            event.client_x,
            event.client_y,
            event.pressure,
            event.tilt_x,
            event.tilt_y,
            event.timestamp_us,
        )?;
        let stroke_id = NodeId::new_v4();
        let command_id = CommandId::new_v4();
        self.stroke = Some(ActiveStroke {
            stroke_id,
            command_id,
            pointer_id: event.pointer_id,
            setup: setup.clone(),
            points: vec![point],
            state: StrokeState::Capturing,
        });
        Ok(InkEffect::Captured { stroke_id })
    }

    fn validate_and_convert(
        setup: &StrokeSetup,
        client_x: f32,
        client_y: f32,
        pressure: f32,
        tilt_x: i16,
        tilt_y: i16,
        timestamp_us: u64,
    ) -> Result<PenPoint, EditorError> {
        // Nonfinite coordinates rejected (spec 19.7).
        if !client_x.is_finite() || !client_y.is_finite() {
            return Err(EditorError::InvalidPointerData(
                "nonfinite client coordinate".into(),
            ));
        }
        if !pressure.is_finite() {
            return Err(EditorError::InvalidPointerData("nonfinite pressure".into()));
        }
        // Integration layer already normalized device range -> [0,1]; the
        // engine clamps defensively. Hover pressure is metadata only.
        let pressure = pressure.clamp(0.0, 1.0);
        let (scene_x, scene_y) = setup.to_scene(client_x, client_y);
        if !scene_x.is_finite() || !scene_y.is_finite() {
            return Err(EditorError::InvalidPointerData(
                "nonfinite scene coordinate".into(),
            ));
        }
        Ok(PenPoint {
            client_x,
            client_y,
            scene_x,
            scene_y,
            pressure,
            tilt_x,
            tilt_y,
            timestamp_us,
        })
    }

    fn update(&mut self, event: PointerEvent) -> Result<InkEffect, EditorError> {
        // Hover never draws, never mutates state (spec 19.7).
        if event.kind == PointerEventKind::Hover || self.stroke.is_none() {
            return Ok(InkEffect::Ignored);
        }
        {
            let stroke = self.stroke.as_ref().unwrap();
            match stroke.state {
                // Only Capturing strokes mutate points; a CommitPending
                // stroke ignores further updates (spec 19.7).
                StrokeState::Capturing => {}
                _ => return Ok(InkEffect::Ignored),
            }
            // A wrong pointer cannot inject points into the active stroke.
            if stroke.pointer_id != event.pointer_id {
                return Ok(InkEffect::Ignored);
            }
        }

        // History MUST be ordered oldest to newest (spec 19.12).
        for w in event.history.windows(2) {
            if w[1].timestamp_us < w[0].timestamp_us {
                return Err(EditorError::InvalidPointerData(
                    "coalesced history not ordered oldest to newest".into(),
                ));
            }
        }

        // Coalesced history, oldest to newest (spec 19.6), each validated.
        for (i, raw) in event.history.iter().enumerate() {
            let authoritative = i + 1 == event.history.len();
            self.append_point(RawPoint {
                client_x: raw.client_x,
                client_y: raw.client_y,
                pressure: if authoritative { event.pressure } else { raw.pressure },
                tilt_x: raw.tilt_x,
                tilt_y: raw.tilt_y,
                timestamp_us: raw.timestamp_us,
            })?;
        }
        // Authoritative current point when no history was supplied.
        if event.history.is_empty() {
            self.append_point(RawPoint {
                client_x: event.client_x,
                client_y: event.client_y,
                pressure: event.pressure,
                tilt_x: event.tilt_x,
                tilt_y: event.tilt_y,
                timestamp_us: event.timestamp_us,
            })?;
        }

        let count = self.stroke.as_ref().unwrap().points.len() as u32;
        Ok(InkEffect::OverlayUpdated { point_count: count })
    }

    /// Append ONE validated point; enforce the configured max by cancelling
    /// with a VISIBLE diagnostic error. Nothing commits in that case.
    fn append_point(&mut self, raw: RawPoint) -> Result<(), EditorError> {
        let stroke = self.stroke.as_mut().expect("append requires active stroke");
        let point = {
            let setup = &stroke.setup;
            Self::validate_and_convert(
                setup,
                raw.client_x,
                raw.client_y,
                raw.pressure,
                raw.tilt_x,
                raw.tilt_y,
                raw.timestamp_us,
            )?
        };
        if (stroke.points.len() as u32) >= self.max_points {
            let (limit, received) = (self.max_points, stroke.points.len() as u32 + 1);
            self.clear();
            return Err(EditorError::MaxPointsExceeded { limit, received });
        }
        stroke.points.push(point);
        Ok(())
    }

    /// Pointer up: final point appended, then CommitPending with ONE stable
    /// command id retained for retries (spec 19.7).
    fn finish(&mut self, event: PointerEvent) -> Result<InkEffect, EditorError> {
        let (state, pointer_id, stroke_id, command_id) = match &self.stroke {
            Some(s) => (s.state, s.pointer_id, s.stroke_id, s.command_id),
            None => return Err(EditorError::NoActiveStroke),
        };
        match state {
            // Duplicate up commits nothing again; retries go via commit path.
            StrokeState::CommitPending => {
                return Ok(InkEffect::StrokeUp { stroke_id, command_id })
            }
            StrokeState::Capturing => {}
            _ => return Ok(InkEffect::Ignored),
        }
        if pointer_id != event.pointer_id {
            return Ok(InkEffect::Ignored);
        }
        // Synthesized mouse up cannot terminate a pen stroke (spec 19.7).
        if event.is_mouse_synthesized {
            return Ok(InkEffect::Ignored);
        }

        self.append_point(RawPoint {
            client_x: event.client_x,
            client_y: event.client_y,
            pressure: event.pressure,
            tilt_x: event.tilt_x,
            tilt_y: event.tilt_y,
            timestamp_us: event.timestamp_us,
        })?;

        let stroke = self.stroke.as_mut().unwrap();
        stroke.state = StrokeState::CommitPending;
        // Freeze the commit spec (clone; points stay on the overlay stroke).
        self.pending = Some(CommitSpec {
            stroke_id,
            command_id,
            setup: stroke.setup.clone(),
            points: stroke.points.clone(),
        });
        Ok(InkEffect::StrokeUp { stroke_id, command_id })
    }

    /// Exactly-once finalize after a durable command outcome arrived.
    /// Only the pending stroke command id is accepted.
    pub fn mark_committed(
        &mut self,
        command_id: CommandId,
    ) -> Result<InkEffect, EditorError> {
        let pending_matches =
            self.pending.as_ref().map(|p| p.command_id == command_id).unwrap_or(false);
        match (self.stroke.as_ref(), pending_matches) {
            (Some(s), true) if s.state == StrokeState::CommitPending => {}
            (Some(s), _) => return Err(EditorError::WrongState { state: s.state }),
            (None, _) => return Err(EditorError::NoActiveStroke),
        }
        self.last_committed = Some(command_id);
        // Overlay is dropped; future rendering reads project snapshots
        // (spec 19.6: no double rendering of committed strokes).
        self.clear();
        Ok(InkEffect::Committed { command_id })
    }

    /// Capture-loss / cancel / focus-loss -> Cancelled. Transient state is
    /// dropped with NO project mutation (spec 19.7, 19.11).
    pub fn cancel(&mut self) -> Result<InkEffect, EditorError> {
        Ok(match self.stroke.take() {
            Some(_) => {
                self.pending = None;
                InkEffect::Cancelled
            }
            None => InkEffect::Ignored,
        })
    }

    /// After a REJECTED submission the user explicitly cancels (spec 19.11).
    pub fn cancel_pending_after_rejection(&mut self) -> InkEffect {
        self.clear();
        InkEffect::CommitRejected { error: "explicitly cancelled".into() }
    }
}
