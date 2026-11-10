//! Canonical little-endian PCM WAV writing and counting (spec Section17.3, Section17.8, Section17.9).
//!
//! write_wav returns the number of sample frames, not interleaved scalar values.
//! Chunked WAV output proves chunk-size invariance (spec Section17.5 step 6).

use crate::error::AudioResult;
use std::io::{Seek, Write};

/// WAV header for 48 kHz stereo PCM (spec Section17.8 defaults).
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
const BITS_PER_SAMPLE: u16 = 16;
const BLOCK_ALIGN: u16 = CHANNELS * (BITS_PER_SAMPLE / 8);
const BYTE_RATE: u32 = SAMPLE_RATE * BLOCK_ALIGN as u32;

/// Write a complete WAV file (spec Section17.9).
///
/// Returns the number of sample frames written (not interleaved scalars).
pub fn write_wav<W: Write + Seek>(
    plan: &crate::plan::MixPlan,
    assets: &dyn crate::mix::PcmAssetSource,
    range: crate::time::SampleRange,
    mut writer: W,
) -> AudioResult<u64> {
    let frame_count = range.len;
    let data_size = frame_count * CHANNELS as u64 * (BITS_PER_SAMPLE as u64 / 8);

    // RIFF header
    write_u32_le(&mut writer, 0x46464952)?; // "RIFF"
    write_u32_le(&mut writer, 36 + data_size as u32)?; // file size - 8
    write_u32_le(&mut writer, 0x45564157)?; // "WAVE"

    // fmt chunk
    write_u32_le(&mut writer, 0x20746d66)?; // "fmt "
    write_u32_le(&mut writer, 16)?; // fmt chunk size
    write_u16_le(&mut writer, 1)?; // PCM format
    write_u16_le(&mut writer, CHANNELS)?;
    write_u32_le(&mut writer, SAMPLE_RATE)?;
    write_u32_le(&mut writer, BYTE_RATE)?;
    write_u16_le(&mut writer, BLOCK_ALIGN)?;
    write_u16_le(&mut writer, BITS_PER_SAMPLE)?;

    // data chunk
    write_u32_le(&mut writer, 0x61746164)?; // "data"
    write_u32_le(&mut writer, data_size as u32)?;

    // Write PCM data in chunks (proves chunk-size invariance, spec Section17.5 step 6).
    let chunk_frames = plan.spec.chunk_samples as u64;
    let mut written_frames = 0u64;

    while written_frames < frame_count {
        let remaining = frame_count - written_frames;
        let chunk_size = chunk_frames.min(remaining);

        let mut buffer = vec![0i16; (chunk_size * CHANNELS as u64) as usize];
        crate::mix::mix_range(
            plan,
            assets,
            crate::time::SampleRange::new(
                crate::time::SampleOffset(written_frames),
                chunk_size,
            ),
            &mut buffer,
        )?;

        // Write as little-endian i16
        for &sample in &buffer {
            write_i16_le(&mut writer, sample)?;
        }

        written_frames += chunk_size;
    }

    writer.flush()?;
    Ok(written_frames)
}

/// Write WAV in streaming fashion (for large exports).
pub fn write_wav_streaming<W: Write + Seek>(
    _assets: &dyn crate::mix::PcmAssetSource,
    frame_count: u64,
    mut writer: W,
) -> AudioResult<u64> {
    let data_size = frame_count * CHANNELS as u64 * (BITS_PER_SAMPLE as u64 / 8);

    // RIFF header
    write_u32_le(&mut writer, 0x46464952)?;
    write_u32_le(&mut writer, 36 + data_size as u32)?;
    write_u32_le(&mut writer, 0x45564157)?;

    // fmt chunk
    write_u32_le(&mut writer, 0x20746d66)?;
    write_u32_le(&mut writer, 16)?;
    write_u16_le(&mut writer, 1)?;
    write_u16_le(&mut writer, CHANNELS)?;
    write_u32_le(&mut writer, SAMPLE_RATE)?;
    write_u32_le(&mut writer, BYTE_RATE)?;
    write_u16_le(&mut writer, BLOCK_ALIGN)?;
    write_u16_le(&mut writer, BITS_PER_SAMPLE)?;

    // data chunk
    write_u32_le(&mut writer, 0x61746164)?;
    write_u32_le(&mut writer, data_size as u32)?;

    // We'll just write silence here - in practice this would be fed by the caller.
    // This is a placeholder for the streaming interface.
    let silence = vec![0i16; (frame_count * CHANNELS as u64) as usize];
    for &sample in &silence {
        write_i16_le(&mut writer, sample)?;
    }

    writer.flush()?;
    Ok(frame_count)
}

fn write_u32_le<W: Write>(writer: &mut W, val: u32) -> AudioResult<()> {
    writer.write_all(&val.to_le_bytes())?;
    Ok(())
}

fn write_u16_le<W: Write>(writer: &mut W, val: u16) -> AudioResult<()> {
    writer.write_all(&val.to_le_bytes())?;
    Ok(())
}

fn write_i16_le<W: Write>(writer: &mut W, val: i16) -> AudioResult<()> {
    writer.write_all(&val.to_le_bytes())?;
    Ok(())
}
