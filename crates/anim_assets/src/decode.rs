//! Decoder registry and importer-version dispatch (spec 11.3, 11.5 step 7).
//!
//! A decoder is REGISTERED against (kind, importer_version). Derived caches
//! (GPU textures, decoded blobs) key on
//!   (content_hash, importer_version, decoder_configuration)
//! and are NEVER authoritative — the manifest is.

use crate::hash::ContentHash;
use crate::{AssetError, AssetKind, AssetResult};

/// Bumped when import/decode behavior changes; derived-cache keys must
/// invalidate on change (spec 11.6).
pub const IMPORTER_VERSION: u32 = 1;

/// Stable derived-cache key spec 11.6: content + importer version + decoder
/// configuration. configuration is opaque text (e.g. "png-rgba8",
/// "wav-pcm16le"); change it ⇒ new cache row, same object.
pub fn derived_cache_key(
    content_hash: &ContentHash,
    importer_version: u32,
    configuration: &str,
) -> String {
    format!("{}_v{}_{}", content_hash.full_hex(), importer_version, configuration)
}

/// Dispatch into the per-kind decoder. Registration table:
///   Image/1 → png → image::decode_png_rgba8
///   Audio/1 → wav → audio::decode_wav_pcm16
/// Anything else → UnsupportedDecoder (spec 11.11: never guess a decoder).
pub fn dispatch(
    kind: AssetKind,
    importer_version: u32,
    bytes: &[u8],
) -> AssetResult<DecodedAsset> {
    match (kind, importer_version) {
        (AssetKind::Image, 1) => {
            Ok(DecodedAsset::Image(crate::image::decode_png_rgba8(bytes)?))
        }
        (AssetKind::Audio, 1) => {
            Ok(DecodedAsset::Audio(crate::audio::decode_wav_pcm16(bytes)?))
        }
        (k, v) => Err(AssetError::UnsupportedDecoder(format!("{k:?}"), v)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedAsset {
    Image(crate::image::DecodedImage),
    Audio(crate::audio::DecodedPcm),
}
