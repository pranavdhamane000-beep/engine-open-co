//! Philox4x32-10 random access (spec §14.7 "Philox random access").
//!
//! * Cursor-free random: NO mutable RNG cursor exists anywhere — every
//!   draw is derived from (StreamKey, absolute tick, semantic draw_index,
//!   lane), so any process can reconstruct any draw at any point.
//! * stream keys come from BLAKE3 over domain `anim_engine/philox/v1` of
//!   (project_seed, system_id, entity_id, purpose, stream_version) —
//!   VERSIONED derivation; changing one version changes ONLY that stream.
//! * output conversion: upper 24 bits / 2^24 → f32 in [0,1).
//! * goldens: the PUBLISHED Random123 KAT (philox4x32, 10 rounds), incl.
//!   the pi counter/key vector; see tests in crate tests/determinism.rs
//!   and the `#[cfg(test)]` module here.
//!
//! Counter words: (tick_low, tick_high, draw_index, 0). The stream key
//! provides two u64s; each splits LITTLE-ENDIAN into the pair of 32-bit
//! Philox key words (Random123 convention).

use crate::error::{SimError, SimResult};
use anim_core::{domain_hash, NodeId};

pub const RANDOM_ALGORITHM_VERSION: u32 = 1;
/// Domain separator pinned here and in ADR-0009; changing it re-keys ALL
/// streams (a version bump, not an edit).
pub const PHILOX_KEY_DOMAIN: &[u8] = b"anim_engine/philox/v1";

const PHILOX_M0: u32 = 0xD2511F53;
const PHILOX_M1: u32 = 0xCD9E8D57;
const PHILOX_W0: u32 = 0x9E3779B9;
const PHILOX_W1: u32 = 0xBB67AE85;
pub const ROUNDS: u32 = 10;

/// Address of a random stream: WHO draws, for WHAT semantic purpose, at
/// WHICH derivation version. Draw indices come from fixed semantic channel
/// tables + deterministic spawn ordinals, never a "next random" cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamKey {
    pub project_seed: [u8; 32],
    pub system_id: u16,
    pub entity_id: NodeId,
    pub purpose: u16,
    pub stream_version: u32,
}

/// Semantic purpose tags (pinned; renumbering is a version bump).
pub mod purpose {
    pub const EMITTER_RATE: u16 = 0;
    pub const PARTICLE_SPAWN: u16 = 1;
}

/// Semantic spawn draw channels (pinned table; 8 channels per ordinal).
pub mod channel {
    pub const RATE_DECISION: u32 = 100; // separate from particle channels
                                        // Particle spawn channels (0..7 = 8 channels = SPAN)
    pub const ANGLE: u32 = 0;
    pub const SPEED: u32 = 1;
    pub const LIFETIME_BAND: u32 = 2;
    pub const SIZE_BAND: u32 = 3;
    pub const ROTATION: u32 = 4;
    pub const JITTER_X: u32 = 5;
    pub const JITTER_Y: u32 = 6;
    pub const JITTER_Z: u32 = 7;
    pub const SPAN: u32 = 8;
}

impl StreamKey {
    /// Versioned stream-key derivation → two key pairs of u64s. Pinned
    /// encoding: domain ∥ seed ∥ u16 system ∥ uuid ∥ u16 purpose ∥ u32
    /// version, all little-endian.
    pub fn derive_keys(&self) -> ([u64; 2], [u64; 2]) {
        let mut buf: Vec<u8> = Vec::with_capacity(64);
        buf.extend_from_slice(PHILOX_KEY_DOMAIN);
        buf.extend_from_slice(&self.project_seed);
        buf.extend_from_slice(&self.system_id.to_le_bytes());
        buf.extend_from_slice(self.entity_id.0.as_bytes());
        buf.extend_from_slice(&self.purpose.to_le_bytes());
        buf.extend_from_slice(&self.stream_version.to_le_bytes());

        let digest = domain_hash("anim_engine/stream_key_v1", &buf);
        let k_64 = [
            u64::from_le_bytes(digest[0..8].try_into().expect("slice")),
            u64::from_le_bytes(digest[8..16].try_into().expect("slice")),
        ];
        let k2_64 = [
            u64::from_le_bytes(digest[16..24].try_into().expect("slice")),
            u64::from_le_bytes(digest[24..32].try_into().expect("slice")),
        ];
        (k_64, k2_64)
    }
}

/// The PUBLISHED Random123 KAT translation: philox4x32 with `rounds`
/// applications. Counter is 4 u32 words, key 2 u32 words. Key bump happens
/// AFTER each round application (known-answer-verified below); the u64
/// stream keys split LITTLE-ENDIAN into the u32 key pair at call time.
pub fn philox4x32(counter: [u32; 4], key: [u32; 2], rounds: u32) -> [u32; 4] {
    let mut c = counter;
    let mut k = [key[0], key[1]];
    for _ in 0..rounds {
        // one 64-bit product gives both halves
        let p0 = (c[0] as u64) * (PHILOX_M0 as u64);
        let p1 = (c[2] as u64) * (PHILOX_M1 as u64);
        let hi0 = (p0 >> 32) as u32;
        let lo0 = p0 as u32;
        let hi1 = (p1 >> 32) as u32;
        let lo1 = p1 as u32;
        let out = [hi1 ^ c[1] ^ k[0], lo1, hi0 ^ c[3] ^ k[1], lo0];
        // key bump AFTER each round application
        k = [k[0].wrapping_add(PHILOX_W0), k[1].wrapping_add(PHILOX_W1)];
        c = out;
    }
    c
}

/// Address-based u32 draw (spec §14.9). Counter words:
/// (tick_low, tick_high, draw_index, 0); lane selects the output word.
pub fn random_u32(
    key: &StreamKey,
    tick: crate::clock::SimTick,
    draw: u32,
    lane: u8,
) -> SimResult<u32> {
    if lane > 3 {
        return Err(SimError::InvalidLane(lane));
    }
    if tick.0 < 0 {
        return Err(SimError::NegativeTick(format!("{tick:?}")));
    }
    let (first, _second) = key.derive_keys();
    let counter = [
        tick.0 as u32,         // tick_low
        (tick.0 >> 32) as u32, // tick_high
        draw,                  // semantic draw index
        0u32,                  // reserved
    ];
    // Philox4x32's key is 64-bit: the FIRST derived u64 splits
    // little-endian into the u32 pair (lo, hi); the second u64 is the
    // reserved twin from the derivation (forward compatibility for
    // 128-bit-key generators).
    let key32 = [first[0] as u32, (first[0] >> 32) as u32];
    let out = philox4x32(counter, key32, ROUNDS);
    Ok(out[lane as usize])
}

/// Address-based f32 draw: upper 24 bits of the lane / 2^24 → [0,1).
pub fn random_f32(
    key: &StreamKey,
    tick: crate::clock::SimTick,
    draw: u32,
    lane: u8,
) -> SimResult<f32> {
    let u = random_u32(key, tick, draw, lane)?;
    Ok((u >> 8) as f32 / (1u32 << 24) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_hex4(s: &str) -> [u32; 4] {
        let mut out = [0u32; 4];
        for (i, w) in s.split_whitespace().enumerate() {
            out[i] = u32::from_str_radix(w, 16).unwrap();
        }
        out
    }

    #[test]
    fn matches_published_random123_kat_vectors() {
        // tests/kat_vectors (Random123): philox4x32, 10 rounds; the vector
        // layout is (counter[4], key[2]) → expected[4].
        let vectors: &[([u32; 4], [u32; 2], [u32; 4])] = &[
            ([0, 0, 0, 0], [0, 0], parse_hex4("6627e8d5 e169c58d bc57ac4c 9b00dbd8")),
            (
                [0xffff_ffff; 4],
                [0xffff_ffff, 0xffff_ffff],
                parse_hex4("408f276d 41c83b0e a20bc7c6 6d5451fd"),
            ),
            (
                parse_hex4("243f6a88 85a308d3 13198a2e 03707344"),
                [0xa409_3822, 0x299f_31d0],
                parse_hex4("d16cfe09 94fdcceb 5001e420 24126ea1"),
            ),
        ];
        for (i, (c, k, expected)) in vectors.iter().enumerate() {
            let got = philox4x32(*c, *k, ROUNDS);
            assert_eq!(got, *expected, "KAT vector {} mismatch", i);
        }
        // also prove rounds=7 diverges from the 10-round engine (the tested
        // round count is part of the pinned config)
        assert_ne!(
            philox4x32([0, 0, 0, 0], [0, 0], 7),
            parse_hex4("6627e8d5 e169c58d bc57ac4c 9b00dbd8"),
            "a 7-round engine run must NOT match the 10-round KAT"
        );
        // ...and the 7-round PUBLISHED vector itself stays valid at R=7
        assert_eq!(
            philox4x32([0, 0, 0, 0], [0, 0], 7),
            parse_hex4("5f6fb709 0d893f64 4f121f81 4f730a48"),
            "the engine reproduces even the 7-round KAT at R=7"
        );
    }

    #[test]
    fn address_draws_are_cursor_free_and_lane_checked() {
        let key = StreamKey {
            project_seed: [7u8; 32],
            system_id: 1,
            entity_id: NodeId::from_u128(9),
            purpose: purpose::PARTICLE_SPAWN,
            stream_version: 1,
        };
        // same address → same draw, in ANY order, ANY number of times
        let a = random_f32(&key, crate::clock::SimTick(100), 3, 0).unwrap();
        let b = random_f32(&key, crate::clock::SimTick(100), 3, 0).unwrap();
        assert_eq!(a, b);
        // range hold: [0,1)
        let windows: Vec<f32> = (0..64)
            .map(|t| random_f32(&key, crate::clock::SimTick(t), 1, 0).unwrap())
            .collect();
        assert!(windows.iter().all(|f| (0.0..1.0).contains(f)));
        // lane rule
        assert_eq!(
            random_u32(&key, crate::clock::SimTick(0), 0, 4),
            Err(SimError::InvalidLane(4))
        );
        // negative ticks rejected
        assert_eq!(
            random_u32(&key, crate::clock::SimTick(-1), 0, 0),
            Err(SimError::NegativeTick("SimTick(-1)".to_string()))
        );
    }

    #[test]
    fn stream_version_change_leaves_other_streams_untouched() {
        let base = StreamKey {
            project_seed: [3u8; 32],
            system_id: 2,
            entity_id: NodeId::from_u128(4),
            purpose: purpose::PARTICLE_SPAWN,
            stream_version: 1,
        };
        let other_purpose = StreamKey { purpose: purpose::EMITTER_RATE, ..base };
        let bumped = StreamKey { stream_version: 2, ..base };
        // bumping stream A's version changes A, NEVER stream B
        assert_ne!(
            random_f32(&base, crate::clock::SimTick(5), 2, 0).unwrap(),
            random_f32(&bumped, crate::clock::SimTick(5), 2, 0).unwrap()
        );
        let b_before = random_f32(&other_purpose, crate::clock::SimTick(5), 2, 0).unwrap();
        let b_after = random_f32(&other_purpose, crate::clock::SimTick(5), 2, 0).unwrap();
        assert_eq!(b_before, b_after);
    }
}
