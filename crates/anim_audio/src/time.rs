//! Checked sample offsets, ranges and frame-boundary conversion (spec Section17.3, Section17.7, Section17.8).
//!
//! Timeline scheduling uses INTEGER samples at 48,000 Hz. Floating-point
//! seconds never determine clip placement (Section17.6). Frame-range conversion
//! uses WP01 `frame_samples` at both boundaries and checked-convert to
//! nonnegative export offsets.

use crate::error::{AudioError, AudioResult};
use anim_core::id::TrackId;
use anim_core::time::{FrameRange, FrameRate};

/// Nonnegative sample offset at 48 kHz (spec Section17.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SampleOffset(pub u64);

impl SampleOffset {
    pub const ZERO: SampleOffset = SampleOffset(0);

    pub fn checked_add(self, other: u64) -> AudioResult<Self> {
        self.0.checked_add(other).map(SampleOffset).ok_or_else(|| {
            AudioError::SampleOffsetOverflow(format!("{} + {}", self.0, other))
        })
    }

    pub fn checked_sub(self, other: u64) -> AudioResult<Self> {
        self.0.checked_sub(other).map(SampleOffset).ok_or_else(|| {
            AudioError::NegativeExportOffset(format!("{} - {}", self.0, other))
        })
    }
}

/// Half-open sample range [start, start+len) at 48 kHz (spec Section17.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleRange {
    pub start: SampleOffset,
    pub len: u64,
}

impl SampleRange {
    pub fn new(start: SampleOffset, len: u64) -> Self {
        Self { start, len }
    }

    pub fn end(&self) -> SampleOffset {
        self.start.checked_add(self.len).expect("range overflow")
    }

    pub fn contains(&self, offset: SampleOffset) -> bool {
        offset >= self.start && offset < self.end()
    }

    pub fn intersection(&self, other: &SampleRange) -> Option<SampleRange> {
        let start = self.start.max(other.start);
        let end = self.end().min(other.end());
        if start < end {
            Some(SampleRange { start, len: end.0 - start.0 })
        } else {
            None
        }
    }
}

/// Immutable offline audio specification (spec Section17.8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineAudioSpec {
    /// Always 48,000 Hz for offline (spec Section17.8 default).
    pub sample_rate: u32,
    /// 1 = mono, 2 = stereo (spec Section17.8 default = 2).
    pub channels: u16,
    /// Export frame range [first, end) (spec Section17.7).
    pub frames: FrameRange,
    /// Frame rate for sample conversion (from scene document).
    pub frame_rate: FrameRate,
    /// Chunk size in samples (spec Section17.8 default = 1024).
    pub chunk_samples: u32,
    /// Algorithm version for cache keying (spec Section17.6).
    pub algorithm_version: u32,
}

impl Default for OfflineAudioSpec {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            frames: FrameRange::new(0, 100).unwrap(),
            frame_rate: FrameRate::new(24000, 1001).unwrap(),
            chunk_samples: 1024,
            algorithm_version: 1,
        }
    }
}

impl OfflineAudioSpec {
    pub fn new(frames: FrameRange, frame_rate: FrameRate) -> Self {
        Self { frames, frame_rate, ..Default::default() }
    }

    /// Convert frame range to sample range at 48 kHz (spec Section17.7).
    /// Uses WP01 `frame_samples` at both boundaries, checked conversion.
    pub fn sample_range(&self) -> AudioResult<SampleRange> {
        // frame_samples uses exact Rational arithmetic (WP01).
        let first_sample =
            self.frame_rate.frame_samples(self.frames.start, 48_000)?.start as u64;
        let end_sample =
            self.frame_rate.frame_samples(self.frames.end, 48_000)?.start as u64;

        let start = SampleOffset(first_sample);
        let len =
            end_sample.checked_sub(first_sample).ok_or(AudioError::EmptyFrameRange)?;

        Ok(SampleRange::new(start, len))
    }

    /// Total samples in the export range.
    pub fn total_samples(&self) -> AudioResult<u64> {
        self.sample_range().map(|r| r.len)
    }
}

/// One planned clip in the mix plan (spec Section17.8).
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedClip {
    pub clip_id: TrackId,
    pub asset_id: anim_core::id::AssetId,
    pub timeline: SampleRange,
    pub source_start: SampleOffset,
    pub source_len: u64,
    pub looped: bool,
    pub gain: f64,
    pub fade_in_samples: u64,
    pub fade_out_samples: u64,
}

impl PlannedClip {
    pub fn source_end(&self) -> SampleOffset {
        self.source_start.checked_add(self.source_len).expect("source overflow")
    }

    /// Whether the clip intersects the given output range.
    pub fn intersects(&self, range: &SampleRange) -> bool {
        self.timeline.intersection(range).is_some()
    }
}
