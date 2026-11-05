//! Canonical byte encoding and domain-separated hashing (spec 10.5, 10.8).
//!
//! Every type that persists carries a stable, deterministic byte encoding;
//! hashes are blake3 with a domain prefix so different document sections
//! can never collide by accidental re-use. Field order in `append_canonical`
//! is the serialization order and must never change once released (a change
//! is a new document-schema version, not a silent hash break).

use uuid::Uuid;

/// Stable, versioned byte encoding for persisted types.
///
/// Implementations append little-endian fixed-width fields in declaration
/// order with no padding, no variable-length prefixes for fixed-width data,
/// and no floating point.
pub trait Canonical {
    fn append_canonical(&self, out: &mut Vec<u8>);

    /// Encoding with the type's own framing omitted; hashing types composed
    /// of others must call `append_canonical` of each child in order.
    fn canonical_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.append_canonical(&mut out);
        out
    }
}

/// Domain-separated blake3: `hash = blake3(len(domain) || domain || payload)`.
/// The length prefix keeps `"ab" + "c"` distinct from `"a" + "bc"`.
pub fn domain_hash(domain: &str, payload: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&(domain.len() as u32).to_le_bytes());
    hasher.update(domain.as_bytes());
    hasher.update(payload);
    *hasher.finalize().as_bytes()
}

/// Convenience: domain-separated hash rendered lowercase hex.
pub fn domain_hash_hex(domain: &str, payload: &[u8]) -> String {
    hex::encode(domain_hash(domain, payload))
}

pub fn append_u8(out: &mut Vec<u8>, v: u8) {
    out.push(v);
}

pub fn append_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn append_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn append_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn append_i64(out: &mut Vec<u8>, v: i64) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn append_i128(out: &mut Vec<u8>, v: i128) {
    out.extend_from_slice(&v.to_le_bytes());
}

pub fn append_bool(out: &mut Vec<u8>, v: bool) {
    out.push(u8::from(v));
}

/// UUIDs hash their 16 big-endian canonical bytes (`Uuid::as_bytes`) — the
/// same ordering used for stable byte ordering of IDs everywhere.
pub fn append_uuid(out: &mut Vec<u8>, v: &Uuid) {
    out.extend_from_slice(v.as_bytes());
}

impl Canonical for u32 {
    fn append_canonical(&self, out: &mut Vec<u8>) {
        append_u32(out, *self);
    }
}

impl Canonical for u64 {
    fn append_canonical(&self, out: &mut Vec<u8>) {
        append_u64(out, *self);
    }
}

impl Canonical for i64 {
    fn append_canonical(&self, out: &mut Vec<u8>) {
        append_i64(out, *self);
    }
}

impl Canonical for i128 {
    fn append_canonical(&self, out: &mut Vec<u8>) {
        append_i128(out, *self);
    }
}

impl Canonical for bool {
    fn append_canonical(&self, out: &mut Vec<u8>) {
        append_bool(out, *self);
    }
}

impl Canonical for String {
    /// Length-prefixed UTF-8: the only case where the encoder needs a prefix,
    /// because the length is variable.
    fn append_canonical(&self, out: &mut Vec<u8>) {
        append_u32(out, self.len() as u32);
        out.extend_from_slice(self.as_bytes());
    }
}

impl Canonical for Uuid {
    fn append_canonical(&self, out: &mut Vec<u8>) {
        append_uuid(out, self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_separation_is_length_framed() {
        // "ab"+"c" vs "a"+"bc" must not collide under naive concatenation.
        let a = domain_hash("ab", b"c");
        let b = domain_hash("a", b"bc");
        assert_ne!(a, b);
    }

    #[test]
    fn string_encoding_distinguishes_boundaries() {
        let mut a = Vec::new();
        String::from("ab").append_canonical(&mut a);
        a.extend_from_slice(b"c");
        let mut b = Vec::new();
        String::from("a").append_canonical(&mut b);
        b.extend_from_slice(b"bc");
        assert_ne!(a, b);
    }

    #[test]
    fn uuid_canonical_bytes_are_stable() {
        let id = Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let mut out = Vec::new();
        id.append_canonical(&mut out);
        assert_eq!(out.len(), 16);
        assert_eq!(hex::encode(&out), "0123456789abcdef0123456789abcdef");
    }

    #[test]
    fn determinism() {
        let mut a = Vec::new();
        7u32.append_canonical(&mut a);
        (-2i64).append_canonical(&mut a);
        let mut b = Vec::new();
        7u32.append_canonical(&mut b);
        (-2i64).append_canonical(&mut b);
        assert_eq!(a, b);
        assert_eq!(a.len(), 12);
    }
}
