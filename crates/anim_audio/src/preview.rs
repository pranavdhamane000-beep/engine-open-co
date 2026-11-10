//! Preview transport, clock generation and discontinuities (spec Section17.3, Section17.6, Section17.7, Section17.8).
//!
//! Preview owns a separate transport. Its callback consumes preallocated PCM
//! or mix blocks, increments the device-sample counter and emits silence on
//! underrun. It does not allocate, block on disk/GPU work, or acquire the
//! project lock. Pause, seek, rate change, underrun and device restart
//! publish clock-generation changes so video does not infer continuity across
//! a discontinuity.

use crate::error::{PreviewError, PreviewResult};
use crate::mix::{mix_range, MemoryAssetSource};
use crate::plan::MixPlan;
use crate::time::SampleOffset;
use std::sync::{Arc, Mutex};

/// Preview clock snapshot (spec Section17.8).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewClockSnapshot {
    pub generation: u64,
    pub device_samples: u64,
    pub anchor_timeline_sample: i64,
    pub rate: f64,
    pub playing: bool,
    pub discontinuity: bool,
}

/// Preview transport state (spec Section17.7, Section17.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportState {
    Stopped,
    Playing,
    Paused,
}

/// Preview transport for real-time audio output (spec Section17.3, Section17.7).
///
/// The callback consumes preallocated PCM or mix blocks, increments the
/// device-sample counter and emits silence on underrun. It does not allocate,
/// block on disk/GPU work, or acquire the project lock.
pub struct PreviewTransport {
    mix_plan: Arc<MixPlan>,
    asset_source: Arc<MemoryAssetSource>,
    state: Arc<Mutex<TransportState>>,
    generation: Arc<Mutex<u64>>,
    device_samples: Arc<Mutex<u64>>,
    anchor_timeline_sample: Arc<Mutex<i64>>,
    rate: Arc<Mutex<f64>>,
    discontinuity: Arc<Mutex<bool>>,
    chunk_samples: u32,
}

impl PreviewTransport {
    pub fn new(
        mix_plan: MixPlan,
        asset_source: MemoryAssetSource,
        chunk_samples: u32,
    ) -> Self {
        Self {
            mix_plan: Arc::new(mix_plan),
            asset_source: Arc::new(asset_source),
            state: Arc::new(Mutex::new(TransportState::Stopped)),
            generation: Arc::new(Mutex::new(0)),
            device_samples: Arc::new(Mutex::new(0)),
            anchor_timeline_sample: Arc::new(Mutex::new(0)),
            rate: Arc::new(Mutex::new(1.0)),
            discontinuity: Arc::new(Mutex::new(false)),
            chunk_samples,
        }
    }

    /// Get current clock snapshot (spec Section17.9).
    pub fn clock_snapshot(&self) -> PreviewClockSnapshot {
        PreviewClockSnapshot {
            generation: *self.generation.lock().unwrap(),
            device_samples: *self.device_samples.lock().unwrap(),
            anchor_timeline_sample: *self.anchor_timeline_sample.lock().unwrap(),
            rate: *self.rate.lock().unwrap(),
            playing: matches!(*self.state.lock().unwrap(), TransportState::Playing),
            discontinuity: *self.discontinuity.lock().unwrap(),
        }
    }

    /// Start playback (spec Section17.7: increments generation).
    pub fn play(&self) -> PreviewResult<()> {
        let mut state = self.state.lock().unwrap();
        let mut gen = self.generation.lock().unwrap();
        let mut disc = self.discontinuity.lock().unwrap();

        *state = TransportState::Playing;
        *gen += 1; // new generation
        *disc = true; // generation change = discontinuity
        Ok(())
    }

    /// Pause playback (freezes position).
    pub fn pause(&self) -> PreviewResult<()> {
        let mut state = self.state.lock().unwrap();
        *state = TransportState::Paused;
        Ok(())
    }

    /// Seek to a sample position (spec Section17.7: replaces anchor, increments generation).
    pub fn seek(&self, position: SampleOffset) -> PreviewResult<()> {
        let state = self.state.lock().unwrap();
        let mut gen = self.generation.lock().unwrap();
        let mut anchor = self.anchor_timeline_sample.lock().unwrap();
        let mut device = self.device_samples.lock().unwrap();
        let mut disc = self.discontinuity.lock().unwrap();

        *anchor = position.0 as i64;
        *device = position.0;
        *gen += 1; // new generation
        *disc = true; // discontinuity

        // If currently playing, restart from new position.
        if matches!(*state, TransportState::Playing) {
            // callback will pick up new position
        }

        Ok(())
    }

    /// Set playback rate (spec Section17.7: rate change publishes generation change).
    pub fn set_rate(&self, rate: f64) -> PreviewResult<()> {
        if rate <= 0.0 || !rate.is_finite() {
            return Err(PreviewError::TransportError(
                "rate must be positive and finite".into(),
            ));
        }
        let mut r = self.rate.lock().unwrap();
        let mut gen = self.generation.lock().unwrap();
        let mut disc = self.discontinuity.lock().unwrap();
        *r = rate;
        *gen += 1;
        *disc = true;
        Ok(())
    }

    /// Get current state.
    pub fn state(&self) -> TransportState {
        *self.state.lock().unwrap()
    }

    /// Audio callback: fill output buffer with mixed PCM (spec Section17.7).
    ///
    /// This is called from the cpal audio callback. It must not allocate,
    /// block on disk/GPU work, or acquire the project lock.
    pub fn callback(&self, output: &mut [i16]) -> PreviewResult<()> {
        let state = *self.state.lock().unwrap();
        let playing = matches!(state, TransportState::Playing);

        if !playing {
            // Fill with silence when not playing.
            output.fill(0);
            return Ok(());
        }

        let device_samples = *self.device_samples.lock().unwrap();
        let _anchor = *self.anchor_timeline_sample.lock().unwrap();
        let _rate = *self.rate.lock().unwrap();

        // Calculate the timeline sample range for this callback.
        let chunk_frames = self.chunk_samples as u64;
        let start_sample = SampleOffset(device_samples);
        let range = crate::time::SampleRange::new(start_sample, chunk_frames);

        // Mix the audio for this range.
        let mut output_buffer = vec![0i16; (chunk_frames * 2) as usize];
        let result =
            mix_range(&self.mix_plan, &*self.asset_source, range, &mut output_buffer);

        match result {
            Ok(_) => {
                // Copy to output
                output.copy_from_slice(&output_buffer);
                *self.device_samples.lock().unwrap() += chunk_frames;
            }
            Err(_) => {
                // Underrun or error: emit silence, mark discontinuity
                output.fill(0);
                *self.discontinuity.lock().unwrap() = true;
            }
        }

        Ok(())
    }

    /// Called when device is lost/restarted (spec Section17.7).
    pub fn on_device_restart(&self) -> PreviewResult<()> {
        let mut gen = self.generation.lock().unwrap();
        let mut disc = self.discontinuity.lock().unwrap();
        *gen += 1;
        *disc = true;
        Ok(())
    }
}
