//! Read-only AssetResolver and diagnostics (spec 11.3, 11.7 resolve+verify).
//!
//! Scene evaluation resolves AssetId READ-ONLY; export MUST verify all
//! referenced source objects before frame zero (spec 11.7/11.10) so missing
//! or changed content STOPS the export. Preview shows diagnostics — never a
//! silent substitute (spec 11.11).

use crate::manifest::AssetManifest;
use crate::store::AssetStore;
use crate::{AssetError, AssetResult};
use anim_core::id::AssetId;
use std::path::PathBuf;

/// Spec 11.9 interface: free `resolve` on (store, manifest, id).
pub fn resolve(
    store: &AssetStore,
    manifest: &AssetManifest,
    id: AssetId,
) -> AssetResult<ResolvedAsset> {
    AssetResolver::new(store, manifest).resolve(id)
}

/// Spec 11.9 interface: free `verify` on (store, entry).
pub fn verify(
    store: &AssetStore,
    entry: &crate::manifest::AssetManifestEntry,
) -> AssetResult<()> {
    store.verify(entry)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAsset {
    pub entry: crate::manifest::AssetManifestEntry,
    pub canonical_path: PathBuf,
}

/// Human-readable diagnostic for preview surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetDiagnostic {
    Ok { id: String, name: String },
    MissingRecord { id: String },
    MissingObject { id: String, detail: String },
    Corrupt { id: String, detail: String },
}

pub struct AssetResolver<'a> {
    store: &'a AssetStore,
    manifest: &'a AssetManifest,
}

impl<'a> AssetResolver<'a> {
    pub fn new(store: &'a AssetStore, manifest: &'a AssetManifest) -> Self {
        Self { store, manifest }
    }

    /// Resolve id → record → object path. Missing record and missing object
    /// are DISTINCT typed errors (spec 11.11).
    pub fn resolve(&self, id: AssetId) -> AssetResult<ResolvedAsset> {
        let entry = self
            .manifest
            .get(&id)
            .ok_or_else(|| AssetError::MissingAssetId(id.to_string()))?;
        let path = self.store.object_path(&entry.content_hash);
        if !path.exists() {
            return Err(AssetError::MissingObject(entry.content_hash.to_hex()));
        }
        Ok(ResolvedAsset { entry: entry.clone(), canonical_path: path })
    }

    /// Resolve AND verify (length + content) — the export-grade path.
    pub fn resolve_verified(&self, id: AssetId) -> AssetResult<ResolvedAsset> {
        let resolved = self.resolve(id)?;
        self.store.verify(&resolved.entry)?;
        Ok(resolved)
    }

    /// Verify every referenced asset; STOPS at the first failure (spec 11.7
    /// "missing or changed content stops export").
    pub fn verify_all(&self, ids: &[AssetId]) -> AssetResult<()> {
        for id in ids {
            let resolved = self.resolve(*id)?;
            self.store.verify(&resolved.entry)?;
        }
        Ok(())
    }

    /// Full-diagnostics sweep (preview surface): every id in the manifest
    ///gets a state; per-id failures never abort the sweep.
    pub fn diagnostics(&self) -> Vec<AssetDiagnostic> {
        let mut out = Vec::new();
        for (id, entry) in self.manifest.records.iter() {
            // diagnostics are VERIFIED state previews (spec 11.7: preview may
            // use verified results but reports missing/mismatched content) —
            // never a silent substitute.
            let diag = match self.resolve_verified(*id) {
                Ok(_) => AssetDiagnostic::Ok {
                    id: id.to_string(),
                    name: entry.display_name.clone(),
                },
                Err(AssetError::MissingObject(detail)) => {
                    AssetDiagnostic::MissingObject { id: id.to_string(), detail }
                }
                Err(AssetError::SizeMismatch { expected, found }) => {
                    AssetDiagnostic::Corrupt {
                        id: id.to_string(),
                        detail: format!(
                            "size mismatch: expected {expected} B, found {found} B"
                        ),
                    }
                }
                Err(other) => AssetDiagnostic::Corrupt {
                    id: id.to_string(),
                    detail: other.to_string(),
                },
            };
            out.push(diag);
        }
        out
    }
}
