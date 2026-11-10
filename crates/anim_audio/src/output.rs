//! cpal stream and preallocated lock-free output ring (spec Section17.3, Section17.9).
//!
//! Offline code MUST NOT call cpal, wall-clock APIs, renderer or editor code.
//! cpal plus ringbuf only under preview-audio feature.

#[cfg(feature = "preview-audio")]
use {
    crate::mix::MemoryAssetSource,
    crate::plan::MixPlan,
    crate::preview::{
        PreviewClockSnapshot, PreviewError, PreviewResult, PreviewTransport, TransportState,
    },
    cpal::traits::{DeviceTrait, HostTrait, StreamTrait},
    cpal::{SampleFormat, SampleRate, StreamConfig, SupportedStreamConfig},
    ringbuf::{Consumer, HeapRb, Producer},
    std::sync::{Arc, Mutex},
};

/// Lock-free ring buffer for audio output (spec Section17.9: preallocated lock-free ring).
#[cfg(feature = "preview-audio")]
type AudioRing = ringbuf::HeapRb<i16>;

/// Preview audio output manager (only available with `preview-audio` feature).
#[cfg(feature = "preview-audio")]
pub struct PreviewOutput {
    transport: Arc<PreviewTransport>,
    stream: Option<cpal::Stream>,
    ring_producer: Arc<Mutex<Producer<i16>>>,
    ring_consumer: Arc<Mutex<Consumer<i16>>>,
    device_name: String,
}

#[cfg(feature = "preview-audio")]
impl PreviewOutput {
    /// Create a new preview output with the given mix plan and asset source.
    pub fn new(
        mix_plan: MixPlan,
        asset_source: MemoryAssetSource,
        chunk_samples: u32,
    ) -> PreviewResult<Self> {
        let transport =
            Arc::new(PreviewTransport::new(mix_plan, asset_source, chunk_samples));

        // Create ring buffer (preallocated, lock-free).
        let ring_size = (chunk_samples as usize * 2 * 4).next_power_of_two(); // 4 chunks of stereo
        let ring = AudioRing::new(ring_size);
        let (producer, consumer) = ring.split();
        let ring_producer = Arc::new(Mutex::new(producer));
        let ring_consumer = Arc::new(Mutex::new(consumer));

        Ok(Self {
            transport,
            stream: None,
            ring_producer,
            ring_consumer,
            device_name: String::new(),
        })
    }

    /// Start the audio stream on the default output device.
    pub fn start(&mut self) -> PreviewResult<()> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| {
            PreviewError::DeviceNotFound("no default output device".into())
        })?;

        self.device_name = device.name().unwrap_or_else(|_| "unknown".into());

        let config = device
            .default_output_config()
            .map_err(|e| PreviewError::FormatNotSupported(e.to_string()))?;

        let sample_format = config.sample_format();
        let config: StreamConfig = config.into();

        // Verify format matches our expectations (48 kHz stereo).
        if config.channels != 2 {
            return Err(PreviewError::FormatNotSupported(format!(
                "expected 2 channels, got {}",
                config.channels
            )));
        }
        if config.sample_rate.0 != 48_000 {
            return Err(PreviewError::FormatNotSupported(format!(
                "expected 48000 Hz, got {}",
                config.sample_rate.0
            )));
        }

        let transport = Arc::clone(&self.transport);
        let ring_producer = Arc::clone(&self.ring_producer);

        // Build the stream with our callback.
        let stream = match sample_format {
            SampleFormat::I16 => device.build_output_stream(
                &config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    let mut producer = ring_producer.lock().unwrap();
                    let transport = Arc::clone(&transport);

                    // Fill from ring buffer first
                    let mut written = 0;
                    while written < data.len() {
                        if let Some(sample) = producer.pop() {
                            data[written] = sample;
                            written += 1;
                        } else {
                            break;
                        }
                    }

                    // If ring buffer underrun, call transport callback for fresh mix
                    if written < data.len() {
                        let mut mix_buffer = vec![0i16; data.len() - written];
                        if transport.callback(&mut mix_buffer).is_ok() {
                            // Push fresh mix to ring buffer
                            for &sample in &mix_buffer {
                                let _ = producer.push(sample);
                            }
                            // Fill remaining from ring
                            while written < data.len() {
                                if let Some(sample) = producer.pop() {
                                    data[written] = sample;
                                    written += 1;
                                } else {
                                    break;
                                }
                            }
                        }

                        // Fill any remaining with silence
                        for sample in &mut data[written..] {
                            *sample = 0;
                        }
                    }
                },
                |err| eprintln!("Audio stream error: {}", err),
                None,
            )?,
            SampleFormat::F32 => {
                return Err(PreviewError::FormatNotSupported("F32 not supported".into()))
            }
            _ => {
                return Err(PreviewError::FormatNotSupported(
                    "unsupported sample format".into(),
                ))
            }
        };

        stream.play().map_err(|e| PreviewError::StreamError(e.to_string()))?;
        self.stream = Some(stream);

        Ok(())
    }

    /// Stop the audio stream.
    pub fn stop(&mut self) {
        if let Some(stream) = self.stream.take() {
            drop(stream);
        }
    }

    /// Get the transport for control (play/pause/seek).
    pub fn transport(&self) -> Arc<PreviewTransport> {
        Arc::clone(&self.transport)
    }

    /// Get current clock snapshot.
    pub fn clock_snapshot(&self) -> PreviewClockSnapshot {
        self.transport.clock_snapshot()
    }

    /// Get device name.
    pub fn device_name(&self) -> &str {
        &self.device_name
    }
}

#[cfg(feature = "preview-audio")]
impl Drop for PreviewOutput {
    fn drop(&mut self) {
        self.stop();
    }
}
