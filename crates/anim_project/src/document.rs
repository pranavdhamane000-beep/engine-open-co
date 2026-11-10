//! The authoritative document and its revision (spec 12.3, 12.8).
//!
//! `ProjectDocument` is the ONLY persistent project state. Fields are public
//! by construction for the command layer, but every mutation goes through
//! `anim_commands` in production; all invariants are re-checked by
//! `schema::validate_document` regardless of how fields were touched.

use crate::scene::SceneData;
use crate::{ProjectError, ProjectResult};
use anim_assets::AssetManifest;
use anim_core::{FrameRate, ProjectId, Revision, SceneId, SimulationSettings};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Bumped by migrations only (spec 12.5: sequential schema migrations).
pub const PROJECT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDocument {
    pub schema_version: u32,
    pub project_id: ProjectId,
    pub revision: Revision,
    pub frame_rate: FrameRate,
    /// WP02 content manifest — assets referenced by scenes live here.
    pub assets: AssetManifest,
    /// Ordered by SceneId byte order (BTreeMap) for canonical serialization.
    pub scenes: BTreeMap<SceneId, SceneData>,
    /// WP05 simulation settings (spec §14.2). `serde(default)` keeps
    /// WP03-era documents loadable with validation-valid defaults
    /// (ticks_per_frame = 1, zero seed — a DETERMINISTIC choice, used
    /// exactly as authored; validation checks invariants, never replaces).
    #[serde(default)]
    pub simulation: SimulationSettings,
}

impl ProjectDocument {
    /// A fresh document at revision zero with an empty scene roster.
    /// `frame_rate` is validated by construction (WP01).
    pub fn new(frame_rate: FrameRate) -> Self {
        Self {
            schema_version: PROJECT_SCHEMA_VERSION,
            project_id: ProjectId::new_v4(),
            revision: Revision::FIRST,
            frame_rate,
            assets: AssetManifest::new(),
            scenes: BTreeMap::new(),
            simulation: SimulationSettings::default(),
        }
    }

    /// New document with an EXISTING identity (load path, tests, replays).
    pub fn with_id(project_id: ProjectId, frame_rate: FrameRate) -> Self {
        let mut doc = Self::new(frame_rate);
        doc.project_id = project_id;
        doc
    }

    /// Revision read for envelope expected-revision matching.
    pub fn revision(&self) -> Revision {
        self.revision
    }
}

/// Rebuild helper used when documents are DESERIALIZED (constructors are
/// bypassed by serde): re-checks schema version before anything else.
pub(crate) fn check_schema_version(doc: &ProjectDocument) -> ProjectResult<()> {
    if doc.schema_version != PROJECT_SCHEMA_VERSION {
        return Err(ProjectError::UnsupportedSchema {
            found: doc.schema_version,
            supported: PROJECT_SCHEMA_VERSION,
        });
    }
    Ok(())
}
