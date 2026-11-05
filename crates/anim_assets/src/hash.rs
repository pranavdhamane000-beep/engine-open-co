//! Streaming BLAKE3 and fixed hash parsing (spec 11.3, 11.5 step 1).
//!
//! A ContentHash is ALWAYS a validated fixed-size 32-byte value. Parsing is
//! the only entry point from text, so no caller can smuggle traversal
//! characters into a path component derived from a hash (spec 11.6).

use crate::AssetError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    /// Hex length of a canonical hash representation: 64 chars.
    pub const HEX_CHARS: usize = 64;

    /// Parse a fixed-size hash from text. Accepts upper or lower case hex,
    /// but the canonical (path-deriving, digest-comparing) form is LOWERCASE.
    pub fn parse(s: &str) -> Result<Self, AssetError> {
        if s.len() != Self::HEX_CHARS {
            return Err(AssetError::InvalidHash(format!(
                "expected {0} hex chars, got {1}",
                Self::HEX_CHARS,
                s.len()
            )));
        }
        let bytes =
            hex::decode(s).map_err(|_| AssetError::InvalidHash("non-hex digest".into()))?;
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Self(arr))
    }

    /// Lowercase canonical hex (spec 11.5: lowercase path derivation).
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// `<first-2-hex>` path component (of the lowercase hex form).
    pub fn first_2(&self) -> String {
        self.to_hex()[0..2].to_string()
    }

    /// `<next-2-hex>` path component.
    pub fn next_2(&self) -> String {
        self.to_hex()[2..4].to_string()
    }

    /// Full 64-char object file name.
    pub fn full_hex(&self) -> String {
        self.to_hex()
    }
}

impl anim_core::hash::Canonical for ContentHash {
    fn append_canonical(&self, out: &mut Vec<u8>) {
        anim_core::hash::append_u32(out, 32); // length-prefixed (spec 11.7 digest)
        out.extend_from_slice(&self.0);
    }
}

impl serde::Serialize for ContentHash {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> serde::Deserialize<'de> for ContentHash {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::parse(&s).map_err(|e| serde::de::Error::custom(e.to_string()))
    }
}

/// Incremental hasher so imports never hold a whole file in memory
/// (spec 11.5: read bounded chunks; blob bypass requires a pinned BLAKE3
/// variant, so plain streaming is used for identical results).
pub struct StreamingHasher(blake3::Hasher);

impl StreamingHasher {
    pub fn new() -> Self {
        Self(blake3::Hasher::new())
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.0.update(chunk);
    }

    pub fn finalize(&self) -> ContentHash {
        ContentHash(*self.0.finalize().as_bytes())
    }
}

impl Default for StreamingHasher {
    fn default() -> Self {
        Self::new()
    }
}
