//! Ordered asset manifest records and domain-separated manifest digest
//! (spec 11.3, 11.5 step 5, 11.7 manifest digest).
//!
//! Records sort by AssetId byte order (BTreeMap over UUID canonical bytes);
//! the digest hashes domain + schema version + length-prefixed canonical
//! fields per record in that order — deterministic under shuffled insertion.
//! Entries are validated at construction and immutable while referenced.

use crate::hash::ContentHash;
use crate::{AssetError, AssetResult, MANIFEST_SCHEMA_VERSION};
use anim_core::hash::domain_hash;
use anim_core::hash::Canonical;
use anim_core::id::AssetId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AssetKind {
    Image,
    Audio,
    Font,
    Vector,
    Rig,
    Other,
}

impl AssetKind {
    /// Stable byte encoding for digests/tests.
    pub fn canonical_u8(self) -> u8 {
        match self {
            AssetKind::Image => 0,
            AssetKind::Audio => 1,
            AssetKind::Font => 2,
            AssetKind::Vector => 3,
            AssetKind::Rig => 4,
            AssetKind::Other => 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetManifestEntry {
    pub asset_id: AssetId,
    pub content_hash: ContentHash,
    pub byte_length: u64,
    pub kind: AssetKind,
    pub importer_version: u32,
    /// Sorted by AssetId; duplicates REJECTED (spec 11.7). Stored internally
    /// sorted so the canonical encoding is order-stable.
    pub dependencies: Vec<AssetId>,
    /// Display metadata only — NEVER used for storage paths (spec 11.6).
    pub display_name: String,
}

impl AssetManifestEntry {
    pub fn new(
        asset_id: AssetId,
        content_hash: ContentHash,
        byte_length: u64,
        kind: AssetKind,
        importer_version: u32,
        dependencies: Vec<AssetId>,
        display_name: String,
    ) -> AssetResult<Self> {
        let mut deps = dependencies;
        deps.sort_by_key(|d| d.canonical_bytes());
        // duplicates REJECTED (spec 11.7 "duplicate dependencies are rejected")
        if deps.windows(2).any(|w| w[0] == w[1]) {
            return Err(AssetError::DuplicateAssetId(format!(
                "{asset_id}: duplicate dependency in list"
            )));
        }
        if deps.contains(&asset_id) {
            return Err(AssetError::DependencyCycle(asset_id.to_string()));
        }
        Ok(Self {
            asset_id,
            content_hash,
            byte_length,
            kind,
            importer_version,
            dependencies: deps,
            display_name,
        })
    }

    /// Length-prefixed canonical field encoding, in declaration order
    /// (spec 11.7 manifest digest). NEVER changes without a schema bump.
    pub fn append_canonical(&self, out: &mut Vec<u8>) {
        let mut field = Vec::new();
        // asset_id
        field.clear();
        self.asset_id.append_canonical(&mut field);
        anim_core::hash::append_u32(out, field.len() as u32);
        out.extend_from_slice(&field);
        // content_hash
        field.clear();
        self.content_hash.append_canonical(&mut field);
        anim_core::hash::append_u32(out, field.len() as u32);
        out.extend_from_slice(&field);
        // byte_length
        field.clear();
        anim_core::hash::append_u64(&mut field, self.byte_length);
        anim_core::hash::append_u32(out, field.len() as u32);
        out.extend_from_slice(&field);
        // kind
        field.clear();
        anim_core::hash::append_u8(&mut field, self.kind.canonical_u8());
        anim_core::hash::append_u32(out, field.len() as u32);
        out.extend_from_slice(&field);
        // importer_version
        field.clear();
        anim_core::hash::append_u32(&mut field, self.importer_version);
        anim_core::hash::append_u32(out, field.len() as u32);
        out.extend_from_slice(&field);
        // dependencies: count + each id length-prefixed (sorted)
        field.clear();
        anim_core::hash::append_u32(&mut field, self.dependencies.len() as u32);
        for d in &self.dependencies {
            d.append_canonical(&mut field);
        }
        anim_core::hash::append_u32(out, field.len() as u32);
        out.extend_from_slice(&field);
        // display_name (UTF-8, length-prefixed)
        field.clear();
        anim_core::hash::append_u32(&mut field, self.display_name.len() as u32);
        field.extend_from_slice(self.display_name.as_bytes());
        anim_core::hash::append_u32(out, field.len() as u32);
        out.extend_from_slice(&field);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AssetManifest {
    pub schema_version: u32,
    /// Ordered by AssetId byte order — BTreeMap over the canonical UUID
    /// ordering (spec 11.7: "sort records by AssetId byte order").
    pub records: BTreeMap<AssetId, AssetManifestEntry>,
}

impl AssetManifest {
    /// Empty manifests are valid (spec 11.7).
    pub fn new() -> Self {
        Self { schema_version: MANIFEST_SCHEMA_VERSION, records: BTreeMap::new() }
    }

    /// Idempotent insert: same id + same content is a no-op success; same id
    /// with different content requires the explicit replace path (WP03's
    /// Command::UpsertAsset handles the revision bump).
    pub fn insert(&mut self, entry: AssetManifestEntry) -> AssetResult<()> {
        if let Some(existing) = self.records.get(&entry.asset_id) {
            if existing.content_hash == entry.content_hash
                && existing.byte_length == entry.byte_length
            {
                return Ok(());
            }
            return Err(AssetError::DuplicateAssetId(entry.asset_id.to_string()));
        }
        self.validate_no_cycle(&entry)?;
        self.records.insert(entry.asset_id, entry);
        Ok(())
    }

    /// Explicit replacement: keeps logical identity (same AssetId), sets a
    /// new content revision (spec 11.6 "reimport with same AssetId creates a
    /// new manifest revision").
    pub fn replace(&mut self, entry: AssetManifestEntry) -> AssetResult<()> {
        self.validate_no_cycle(&entry)?;
        self.records.insert(entry.asset_id, entry);
        Ok(())
    }

    /// DFS cycle detection over the dependency graph rooted at `entry`
    /// (spec 11.11 DependencyCycle). Runs against the manifest as it WOULD
    /// be with the entry applied; the tentative insert is rolled back on
    /// error, keeping the manifest unchanged.
    fn validate_no_cycle(&self, entry: &AssetManifestEntry) -> AssetResult<()> {
        // adjacency: manifest + the new entry replacing its own id
        let mut windows: BTreeMap<AssetId, Vec<AssetId>> =
            self.records.iter().map(|(k, v)| (*k, v.dependencies.clone())).collect();
        windows.insert(entry.asset_id, entry.dependencies.clone());

        // DFS from entry itself, looking for a path back to entry.asset_id
        let mut stack = vec![entry.asset_id];
        let mut seen: std::collections::HashSet<AssetId> = std::collections::HashSet::new();
        while let Some(cur) = stack.pop() {
            if !seen.insert(cur) {
                continue;
            }
            let Some(deps) = windows.get(&cur) else { continue };
            for d in deps {
                if *d == entry.asset_id {
                    return Err(AssetError::DependencyCycle(entry.asset_id.to_string()));
                }
                stack.push(*d);
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &AssetId) -> Option<&AssetManifestEntry> {
        self.records.get(id)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

/// Domain-separated manifest digest (spec 11.7): domain string, schema
/// version, then each record's length-prefixed canonical fields IN ORDER.
/// Records already ordered via BTreeMap — deterministic under shuffled
/// insertion. Fixed only by a schema version bump.
pub fn manifest_digest(manifest: &AssetManifest) -> AssetResult<ContentHash> {
    let mut buf = Vec::new();
    const DOMAIN: &str = "anim_assets_manifest_v1";
    anim_core::hash::append_u32(&mut buf, DOMAIN.len() as u32);
    buf.extend_from_slice(DOMAIN.as_bytes());
    anim_core::hash::append_u32(&mut buf, manifest.schema_version);
    anim_core::hash::append_u32(&mut buf, manifest.records.len() as u32);
    for record in manifest.records.values() {
        record.append_canonical(&mut buf);
    }
    Ok(ContentHash(domain_hash(DOMAIN, &buf)))
}
