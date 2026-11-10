//! Play, pause, seek, frame-step and loop state (spec 19.2, 19.5, 19.10).
//!
//! Transport owns EDITOR play state. Video advance follows preview audio
//! clock generations (WP08 `PreviewClockSnapshot`); all seeking is explicit
//! position seeking; frame-stepping is allowed ONLY while paused. No
//! timeline change, project mutation or export status lives here.

use crate::error::EditorError;
use anim_audio::PreviewClockSnapshot;

/// Canonical position reported forward to panels/viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportSnapshot {
    pub position_frame: i64,
    pub playing: bool,
    pub loop_start_frame: Option<i64>,
    pub loop_end_frame: Option<i64>,
    /// Latest audio clock generation (WP08); video never infers continuity
    /// across a generation change.
    pub audio_clock_generation: u64,
}

/// Editor transport (single-writer, non-UI state).
#[derive(Debug, Clone)]
pub struct EditorTransport {
    position_frame: i64,
    playing: bool,
    loop_range: Option<(i64, i64)>,
    audio_clock_generation: u64,
}

impl EditorTransport {
    pub fn new(start_frame: i64) -> Self {
        Self {
            position_frame: start_frame,
            playing: false,
            loop_range: None,
            audio_clock_generation: 0,
        }
    }

    pub fn play(&mut self) {
        self.playing = true;
    }

    pub fn pause(&mut self) {
        self.playing = false;
    }

    /// Explicit position seek (spec 19.6).
    pub fn seek(&mut self, position_frame: i64) {
        self.position_frame = position_frame;
    }

    pub fn set_loop(&mut self, range: Option<(i64, i64)>) {
        debug_assert!(
            range.map(|(a, b)| a <= b).unwrap_or(true),
            "loop start must not exceed loop end"
        );
        self.loop_range = range;
    }

    pub fn loop_range(&self) -> Option<(i64, i64)> {
        self.loop_range
    }

    /// Frame-step permitted ONLY when paused (spec 19.5 step 2).
    pub fn step_frames(&mut self, frames: i64) -> Result<(), EditorError> {
        if self.playing {
            return Err(EditorError::Transport(
                "frame-step requires paused transport".into(),
            ));
        }
        // Saturating at 0 so runaway steps cannot panic the event loop (19.11).
        self.position_frame = (self.position_frame + frames).max(0);
        Ok(())
    }

    /// Ingest the preview audio clock. Generation CHANGES mark audio
    /// discontinuities (pause/seek/rate/underrun/device restart); the sync
    /// policy lives at a higher layer — the transport only records the
    /// generation verbatim.
    pub fn ingest_audio_clock(&mut self, clock: &PreviewClockSnapshot) {
        self.audio_clock_generation = clock.generation;
        self.playing = clock.playing;
    }

    pub fn snapshot(&self) -> TransportSnapshot {
        TransportSnapshot {
            position_frame: self.position_frame,
            playing: self.playing,
            loop_start_frame: self.loop_range.map(|(a, _)| a),
            loop_end_frame: self.loop_range.map(|(_, b)| b),
            audio_clock_generation: self.audio_clock_generation,
        }
    }

    pub fn position_frame(&self) -> i64 {
        self.position_frame
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_requires_paused() {
        let mut t = EditorTransport::new(0);
        assert!(t.step_frames(1).is_ok());

        t.play();
        assert!(t.step_frames(1).is_err());
        assert_eq!(t.position_frame(), 1, "playing step rejected before mutation");

        t.pause();
        assert!(t.step_frames(-3).is_ok());
        assert_eq!(t.position_frame(), 0, "stepping saturates at 0 per §19.11");
    }

    #[test]
    fn seek_is_explicit_and_loop_roundtrip() {
        let mut t = EditorTransport::new(0);
        t.set_loop(Some((10, 20)));
        t.seek(15);
        let s = t.snapshot();
        assert_eq!(s.position_frame, 15);
        assert_eq!(s.loop_start_frame, Some(10));
        assert_eq!(s.loop_end_frame, Some(20));
        t.set_loop(None);
        assert!(t.snapshot().loop_start_frame.is_none());
    }

    #[test]
    fn audio_clock_generations_recorded_verbatim() {
        let mut t = EditorTransport::new(0);
        let clock = anim_audio::PreviewClockSnapshot {
            generation: 7,
            device_samples: 0,
            anchor_timeline_sample: 0,
            rate: 1.0,
            playing: true,
            discontinuity: true,
        };
        t.ingest_audio_clock(&clock);
        let s = t.snapshot();
        assert_eq!(s.audio_clock_generation, 7);
        assert!(s.playing);
    }
}
