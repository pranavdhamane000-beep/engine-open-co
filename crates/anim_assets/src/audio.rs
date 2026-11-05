//! PCM metadata and source decoding (spec 11.3).
//!
//! MVP decoder coverage (WP02): WAV → 16-bit interleaved PCM via symphonia
//! (mirror of the WP00 media-probe decode path, which was proven byte-exact
//! on this machine — ADR-0004). Audio planning NEVER treats container length
//! as authoritative; actual decoded sample counts are (ADR-0004 lesson).

use crate::AssetError;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedPcm {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
    /// 16-bit interleaved samples (frames × channels).
    pub samples: Vec<i16>,
    /// Stable decoder configuration fragment for derived-cache keys.
    pub configuration: &'static str,
}

/// Decode WAV (PCM s16 precision) to interleaved i16 + metadata.
pub fn decode_wav_pcm16(bytes: &[u8]) -> crate::AssetResult<DecodedPcm> {
    let mss = MediaSourceStream::new(
        Box::new(std::io::Cursor::new(bytes.to_vec())),
        Default::default(),
    );
    let mut hint = Hint::new();
    hint.with_extension("wav");

    let mut probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| AssetError::Decode(format!("wav probe: {e}")))?;

    let track = probed
        .format
        .tracks()
        .first()
        .cloned()
        .ok_or_else(|| AssetError::Decode("wav: no audio track".into()))?;

    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .ok_or_else(|| AssetError::Decode("wav: unknown channel count".into()))?;
    let sample_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| AssetError::Decode("wav: unknown sample rate".into()))?;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| AssetError::Decode(format!("wav decoder init: {e}")))?;

    let mut samples: Vec<i16> = Vec::new();
    let mut sample_buf: Option<SampleBuffer<i16>> = None;
    loop {
        let packet = match probed.format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(symphonia::core::errors::Error::DecodeError(e)) => {
                return Err(AssetError::Decode(format!("wav packet: {e}")));
            }
            Err(other) => return Err(AssetError::Decode(format!("wav: {other}"))),
        };
        match decoder.decode(&packet) {
            Ok(decoded) => {
                let buf = sample_buf.get_or_insert_with(|| {
                    SampleBuffer::<i16>::new(decoded.capacity() as u64, *decoded.spec())
                });
                buf.copy_interleaved_ref(decoded);
                samples.extend_from_slice(buf.samples());
            }
            Err(symphonia::core::errors::Error::DecodeError(e)) => {
                return Err(AssetError::Decode(format!("wav decode: {e}")))
            }
            Err(other) => return Err(AssetError::Decode(format!("wav: {other}"))),
        }
    }

    let per_frame = (channels.max(1)) as u64;
    let frames = (samples.len() as u64) / per_frame;
    Ok(DecodedPcm {
        sample_rate,
        channels,
        frames,
        samples,
        configuration: "wav-pcm16le-interleaved",
    })
}
