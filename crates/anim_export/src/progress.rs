//! Progress and cancellation contracts (spec §18.9, §18.11).
//!
//! ProgressSink receives stage and completed/total frames but cannot alter
//! timing or quality. Cancellation closes encoder input, terminates and
//! reaps children when needed, unmaps buffers and removes temporary
//! artifacts unless diagnostic preservation is requested.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Cancellation token (spec §18.9).
#[derive(Debug, Clone, Default)]
pub struct CancellationToken {
    cancelled: Arc<AtomicBool>,
}

impl CancellationToken {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> crate::error::ExportResult<()> {
        if self.is_cancelled() {
            Err(crate::error::ExportError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Progress sink trait (spec §18.9).
pub trait ProgressSink: Send + Sync {
    /// Called when pipeline stage changes
    fn on_stage(&self, stage: crate::pipeline::PipelineStage, frame: u64, total: u64);

    /// Called periodically with frame progress
    fn on_progress(&self, frame: u64, total: u64);

    /// Called on error
    fn on_error(&self, error: &crate::error::ExportError);
}

/// Simple console progress sink
pub struct ConsoleProgressSink {
    last_stage: std::sync::Mutex<Option<crate::pipeline::PipelineStage>>,
    start_time: std::time::Instant,
}

impl Default for ConsoleProgressSink {
    fn default() -> Self {
        Self {
            last_stage: std::sync::Mutex::new(None),
            start_time: std::time::Instant::now(),
        }
    }
}

impl ConsoleProgressSink {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ProgressSink for ConsoleProgressSink {
    fn on_stage(&self, stage: crate::pipeline::PipelineStage, frame: u64, total: u64) {
        let mut last = self.last_stage.lock().unwrap();
        if *last != Some(stage) {
            *last = Some(stage);
            println!(
                "[{:?}] Stage: {:?} ({}/{})",
                self.start_time.elapsed(),
                stage,
                frame,
                total
            );
        }
    }

    fn on_progress(&self, frame: u64, total: u64) {
        if frame.is_multiple_of(100) || frame == total {
            let elapsed = self.start_time.elapsed();
            let fps = if elapsed.as_secs_f64() > 0.0 {
                frame as f64 / elapsed.as_secs_f64()
            } else {
                0.0
            };
            println!("Progress: {}/{} ({:.1} fps)", frame, total, fps);
        }
    }

    fn on_error(&self, error: &crate::error::ExportError) {
        eprintln!("Export error: {}", error);
    }
}

/// Progress callback for FFmpeg
pub struct FfmpegProgressCallback {
    total_frames: u64,
    current_frame: std::sync::atomic::AtomicU64,
    progress_sink: Box<dyn crate::progress::ProgressSink>,
}

impl FfmpegProgressCallback {
    pub fn new(
        total_frames: u64,
        progress_sink: Box<dyn crate::progress::ProgressSink>,
    ) -> Self {
        Self {
            total_frames,
            current_frame: std::sync::atomic::AtomicU64::new(0),
            progress_sink,
        }
    }

    pub fn frame_done(&self) {
        let frame =
            self.current_frame.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        self.progress_sink.on_progress(frame, self.total_frames);
    }
}
