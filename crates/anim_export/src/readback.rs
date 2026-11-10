//! GPU readback ring with fixed capacity, row unpadding and backpressure (spec §18.6, §18.7).
//!
//! The readback ring supplies backpressure so rendering cannot run more than
//! its slot capacity ahead of FFmpeg. wgpu padded bytes per row are removed
//! before raw-frame writing.
//!
//! State machine: Free -> Submitted -> Mapped -> Ready -> Free

use crate::error::{ExportError, ExportResult};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Readback slot state (spec §18.8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    Free,
    Submitted,
    Mapped,
    Ready,
    Error,
}

/// Readback slot (spec §18.8).
#[derive(Debug)]
pub struct ReadbackSlot {
    pub buffer: wgpu::Buffer,
    pub frame_index: i64,
    pub state: SlotState,
    pub bytes_per_row: u32,
    pub padded_bytes_per_row: u32,
    pub height: u32,
}

/// Fixed-capacity readback ring with backpressure (spec §18.6, §18.7).
pub struct ReadbackRing {
    slots: Vec<Arc<Mutex<ReadbackSlot>>>,
    capacity: usize,
    padded_bytes_per_row: u32,
    packed_stride: usize,
    submitted_count: Mutex<usize>,
    completed_count: Mutex<usize>,
    not_full: Condvar,
    not_empty: Condvar,
}

impl ReadbackRing {
    /// Create a new readback ring (spec §18.8: starts with 3 slots unless profile says otherwise).
    pub fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        capacity: usize,
    ) -> ExportResult<Self> {
        let bytes_per_pixel = format
            .block_copy_size(None)
            .ok_or(ExportError::InvalidSettings("unsupported texture format".into()))?;

        let padded_bytes_per_row = (bytes_per_pixel * width).div_ceil(256) * 256;
        let packed_stride = (bytes_per_pixel * width) as usize;

        let mut slots = Vec::with_capacity(capacity);
        for i in 0..capacity {
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(&format!("readback_slot_{}", i)),
                size: (padded_bytes_per_row * height) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            slots.push(Arc::new(Mutex::new(ReadbackSlot {
                buffer,
                frame_index: -1,
                state: SlotState::Free,
                bytes_per_row: bytes_per_pixel * width,
                padded_bytes_per_row,
                height,
            })));
        }

        Ok(Self {
            slots,
            capacity,
            padded_bytes_per_row,
            packed_stride: packed_stride * height as usize,
            submitted_count: Mutex::new(0),
            completed_count: Mutex::new(0),
            not_full: Condvar::new(),
            not_empty: Condvar::new(),
        })
    }

    /// Get the padded bytes per row for this ring.
    pub fn padded_bytes_per_row(&self) -> u32 {
        self.padded_bytes_per_row
    }

    /// Get a reference to a slot by index.
    pub fn slot(&self, idx: usize) -> Option<Arc<Mutex<ReadbackSlot>>> {
        self.slots.get(idx).cloned()
    }

    /// Acquire a free slot for the next frame (blocks if ring is full).
    /// Returns the slot index and the slot itself.
    pub fn acquire_slot(
        &self,
        _frame_index: i64,
    ) -> ExportResult<(usize, Arc<Mutex<ReadbackSlot>>)> {
        let mut submitted = self.submitted_count.lock().unwrap();

        // Wait for a free slot
        while *submitted >= self.capacity {
            submitted = self.not_full.wait(submitted).unwrap();
        }

        // Find a free slot
        let (_, slot) = self
            .slots
            .iter()
            .enumerate()
            .find(|(_, s)| s.lock().unwrap().state == SlotState::Free)
            .ok_or_else(|| ExportError::ReadbackFailed("no free slot found".into()))?;

        // Mark as submitted
        {
            let mut slot_guard = slot.lock().unwrap();
            slot_guard.frame_index = *submitted as i64;
            slot_guard.state = SlotState::Submitted;
        }

        *submitted += 1;
        self.not_empty.notify_one();

        Ok((*submitted - 1, slot.clone()))
    }

    /// Submit the slot for GPU copy (called after encoder.submit()).
    pub fn submit_slot(&self, idx: usize) -> ExportResult<()> {
        let slot = &self.slots[idx];
        let mut guard = slot.lock().unwrap();
        if guard.state != SlotState::Submitted {
            return Err(ExportError::ReadbackFailed("slot not in Submitted state".into()));
        }
        guard.state = SlotState::Mapped;
        Ok(())
    }

    /// Wait for a slot to become Ready (mapped and data available).
    /// Returns the unpadded data.
    pub async fn wait_ready(
        &self,
        slot_idx: usize,
        timeout: Duration,
    ) -> ExportResult<Vec<u8>> {
        let start = std::time::Instant::now();

        loop {
            // Check if this slot is ready
            {
                let slot = &self.slots[slot_idx];
                let mut guard = slot.lock().unwrap();
                if guard.state == SlotState::Ready {
                    let data = self.unpad_slot(&mut guard)?;
                    guard.state = SlotState::Free;
                    guard.frame_index = -1;

                    let mut completed = self.completed_count.lock().unwrap();
                    *completed += 1;
                    self.not_full.notify_one();

                    return Ok(data);
                }
            }

            // Timeout check
            if start.elapsed() > timeout {
                return Err(ExportError::Timeout("readback timeout".into()));
            }

            // Wait a bit
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// Try to get a ready slot without blocking (for polling).
    pub fn try_ready(&self) -> Option<(usize, Vec<u8>)> {
        for (idx, slot) in self.slots.iter().enumerate() {
            let mut guard = slot.lock().unwrap();
            if guard.state == SlotState::Ready {
                let data = self.unpad_slot(&mut guard).ok()?;
                guard.state = SlotState::Free;
                guard.frame_index = -1;

                let mut completed = self.completed_count.lock().unwrap();
                *completed += 1;
                self.not_full.notify_one();

                return Some((idx, data));
            }
        }
        None
    }

    /// Mark slot as error state
    pub fn mark_error(&self, idx: usize) {
        if let Some(slot) = self.slots.get(idx) {
            slot.lock().unwrap().state = SlotState::Error;
        }
    }

    /// Unpad row-padded data to packed format
    fn unpad_slot(&self, slot: &mut ReadbackSlot) -> ExportResult<Vec<u8>> {
        let padded = slot.buffer.slice(..).get_mapped_range();
        let mut output = Vec::with_capacity(self.packed_stride * slot.height as usize);

        let padded_row = slot.padded_bytes_per_row as usize;
        let packed_row = slot.bytes_per_row as usize;

        for y in 0..slot.height as usize {
            let start = y * padded_row;
            let end = start + packed_row;
            if end <= padded.len() {
                output.extend_from_slice(&padded[start..end]);
            } else {
                return Err(ExportError::ShortReadback {
                    expected: self.packed_stride * slot.height as usize,
                    actual: output.len(),
                });
            }
        }

        Ok(output)
    }

    /// Get current ring statistics
    pub fn stats(&self) -> RingStats {
        let submitted = *self.submitted_count.lock().unwrap();
        let completed = *self.completed_count.lock().unwrap();
        let free = self
            .slots
            .iter()
            .filter(|s| s.lock().unwrap().state == SlotState::Free)
            .count();

        RingStats {
            capacity: self.capacity,
            submitted,
            completed,
            free,
            in_flight: submitted - completed,
        }
    }
}

/// Ring buffer statistics
#[derive(Debug, Clone, Copy)]
pub struct RingStats {
    pub capacity: usize,
    pub submitted: usize,
    pub completed: usize,
    pub free: usize,
    pub in_flight: usize,
}
