//! GPU → CPU readback with 256-byte padded rows (wgpu copy trait) plus
//! unpadded extraction helpers for the G5 goldens (§15.10, WP06 tests).
//!
//! Readback is an explicit, never-silent operation: mapping happens
//! through `poll` (blocking) so a failed wait surfaces as an error
//! instead of a stale texture.

use crate::error::{RenderError, RenderResult};

/// Closest row-pitch greater or equal to `bpp · width` that is a
/// multiple of 256.
pub fn padded_bytes_per_row(bytes_per_pixel: u32, width: u32) -> u32 {
    (bytes_per_pixel * width).div_ceil(256) * 256
}

/// Copy a texture into a staging buffer and block until mapped.
/// Returns the PADDED bytes (row stride `padded_bytes_per_row`).
pub fn download_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    label: &'static str,
) -> RenderResult<Vec<u8>> {
    let extent = texture.size();
    let bytes_per_pixel = block_bytes(texture.format());
    let bytes_per_row = padded_bytes_per_row(bytes_per_pixel, extent.width);
    let buffer_size = u64::from(bytes_per_row) * u64::from(extent.height);

    let target = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &target,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        extent,
    );
    queue.submit(Some(encoder.finish()));

    let slice = target.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| ());
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| RenderError::InvalidSnapshot(format!("readback poll failed: {e}")))?;
    let data = slice.get_mapped_range();
    let copied = data.to_vec();
    drop(data);
    target.unmap();
    Ok(copied)
}

/// Strip 256-byte padding from a download: bytes of row `y` live at
/// `y · padded_stride`; each row contributes only its first
/// `bytes_per_pixel · width` values.
pub fn unpad_rows(
    padded: &[u8],
    bytes_per_pixel: u32,
    width: u32,
    height: u32,
) -> RenderResult<Vec<u8>> {
    let row = padded_bytes_per_row(bytes_per_pixel, width) as usize;
    let row_len = (bytes_per_pixel * width) as usize;
    if row == 0 || padded.len() < row * height as usize {
        return Err(RenderError::InvalidSnapshot(format!(
            "padded buffer too small: {} < {}",
            padded.len(),
            row * height as usize
        )));
    }
    let mut out = Vec::with_capacity(row_len * height as usize);
    for y in 0..height as usize {
        out.extend_from_slice(&padded[y * row..y * row + row_len]);
    }
    Ok(out)
}

/// Bytes per texel block of every format this crate produces or reads.
/// Unknown formats are a hard error (never a silent guess).
pub fn block_bytes(format: wgpu::TextureFormat) -> u32 {
    match format {
        wgpu::TextureFormat::Rgba16Float
        | wgpu::TextureFormat::Rgba16Unorm
        | wgpu::TextureFormat::Rgba16Uint => 8,
        wgpu::TextureFormat::Rgba8Unorm
        | wgpu::TextureFormat::Rgba8UnormSrgb
        | wgpu::TextureFormat::Rgba8Uint
        | wgpu::TextureFormat::Bgra8Unorm
        | wgpu::TextureFormat::Bgra8UnormSrgb => 4,
        wgpu::TextureFormat::Rgba32Float | wgpu::TextureFormat::Rgba32Uint => 16,
        other => {
            if let Some(size) = other.block_copy_size(None) {
                return size;
            }
            unreachable!("unhandled format {other:?} lacks a block size");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_matches_the_wgpu_256_rule() {
        // 8 bpp, 64 wide: 8*64 = 512 → already aligned.
        assert_eq!(padded_bytes_per_row(8, 64), 512);
        // 8*15 = 120 → rounds up to 256.
        assert_eq!(padded_bytes_per_row(8, 15), 256);
        // 4*300 = 1200 → next multiple of 256 is 1280.
        assert_eq!(padded_bytes_per_row(4, 300), 1280);
        // 8*48 = 384 → 512.
        assert_eq!(padded_bytes_per_row(8, 48), 512);
    }

    #[test]
    fn unpad_restores_tight_rows() {
        // 2x2 texture, 8 bpp: tight row = 16 bytes; padded row = 256.
        let mut padded = vec![0u8; 256 * 2];
        let row_a: Vec<u8> = (1u8..=16).collect();
        let row_b: Vec<u8> = (17u8..=32).collect();
        padded[..16].copy_from_slice(&row_a);
        padded[256..272].copy_from_slice(&row_b);
        let out = unpad_rows(&padded, 8, 2, 2).unwrap();
        assert_eq!(out, row_a.iter().chain(row_b.iter()).copied().collect::<Vec<_>>());
    }

    #[test]
    fn unpad_rejects_short_buffers() {
        assert!(unpad_rows(&[0, 0], 8, 3, 2).is_err());
    }
}
