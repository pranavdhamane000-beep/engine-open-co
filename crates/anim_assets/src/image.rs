//! Image metadata, color and alpha decode (spec 11.3).
//!
//! MVP decoder coverage (WP02): PNG → RGBA-8 (straight, NOT premultiplied).
//! Unknown image formats error as UnsupportedDecoder at dispatch; corrupt
//! bytes as Decode. GPU textures derived later key on content + this config.

use crate::AssetError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    /// RGBA8, straight alpha (not premultiplied) — render compositor contract.
    pub rgba: Vec<u8>,
    /// True when the source format carries an alpha channel.
    pub has_alpha: bool,
    /// Stable decoder configuration fragment for derived-cache keys.
    pub configuration: &'static str,
}

/// Decode PNG bytes to RGBA-8 + metadata.
pub fn decode_png_rgba8(bytes: &[u8]) -> crate::AssetResult<DecodedImage> {
    let img = image::load_from_memory(bytes)
        .map_err(|e| AssetError::Decode(format!("png: {e}")))?;
    let rgba_img = img.to_rgba8();
    let has_alpha = png_has_alpha(bytes, rgba_img.width(), rgba_img.height())?;

    Ok(DecodedImage {
        width: rgba_img.width(),
        height: rgba_img.height(),
        rgba: rgba_img.into_raw(),
        has_alpha,
        configuration: "png-rgba8-straight",
    })
}

/// PNG alpha detection by parsing the IHDR color-type byte directly
/// (7 = palette, 3 = RGB, 0 = gray → no alpha; 6 = RGBA, 4 = gray+alpha →
/// alpha). Structure: 8-byte signature, 4-byte length, "IHDR", then the
/// 13-byte IHDR payload starts at offset 16; color type = byte at offset 25.
/// A palette indexed image can still carry a tRNS chunk, but that is an
/// informational alpha — not counted here (documented decoder limitation).
fn png_has_alpha(bytes: &[u8], w: u32, h: u32) -> crate::AssetResult<bool> {
    if bytes.len() < 26 {
        return Err(AssetError::Decode(format!("png too small for IHDR ({w}x{h})")));
    }
    if bytes[12..16] != *b"IHDR" {
        return Err(AssetError::Decode("png missing IHDR".into()));
    }
    Ok(matches!(bytes[25], 4 | 6))
}
