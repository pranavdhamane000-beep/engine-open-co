//! Revision/position-keyed Arc snapshot cache (spec §13.8).
//!
//! Entries are `Arc<SceneSnapshot>` keyed by `(SceneId, Revision,
//! FramePos)`. Snapshots are IMMUTABLE and remain valid for their ORIGINAL
//! revision after undo or new edits — but a stale revision can never be
//! presented as current, because current evaluation keys on the document's
//! NOW revision (isolation by key identity, §13.12 cache-isolation proof).
//!
//! Bounded: oldest-inserted entry is dropped when the capacity is reached.
//! Eviction drops only cached copies (never snapshot contents).

use crate::error::SceneResult;
use crate::evaluator::SceneEvaluator;
use crate::snapshot::SceneSnapshot;
use anim_core::{FramePos, SceneId};
use anim_project::ProjectDocument;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    scene: SceneId,
    revision: u64,
    position: FramePos,
}

pub struct SceneCache {
    capacity: usize,
    entries: HashMap<CacheKey, (Arc<SceneSnapshot>, u64)>,
    insertion: u64,
}

impl Default for SceneCache {
    fn default() -> Self {
        Self::new(128)
    }
}

impl SceneCache {
    pub fn new(capacity: usize) -> Self {
        Self { capacity: capacity.max(1), entries: HashMap::new(), insertion: 0 }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Cached snapshot for the KEY (never for "current" — the caller passes
    /// the document revision explicitly; a stale revision can be fetched
    /// by key but cannot impersonate the current revision).
    pub fn get(
        &self,
        document: &ProjectDocument,
        scene_id: SceneId,
        position: FramePos,
    ) -> Option<Arc<SceneSnapshot>> {
        let key =
            CacheKey { scene: scene_id, revision: document.revision.as_u64(), position };
        self.entries.get(&key).map(|(arc, _)| Arc::clone(arc))
    }

    /// Compute-or-cached evaluation at a whole position of the CURRENT
    /// document revision.
    pub fn get_or_evaluate(
        &mut self,
        document: &ProjectDocument,
        scene_id: SceneId,
        position: FramePos,
    ) -> SceneResult<Arc<SceneSnapshot>> {
        let key = CacheKey {
            scene: scene_id,
            revision: document.revision.as_u64(),
            position: position.clone(),
        };
        if let Some((arc, _)) = self.entries.get(&key) {
            return Ok(Arc::clone(arc));
        }
        let snapshot = SceneEvaluator::evaluate(document, scene_id, position.clone())?;
        self.insertion += 1;
        self.entries.insert(key, (Arc::clone(&snapshot), self.insertion));
        if self.entries.len() > self.capacity {
            // evict the OLDEST insertion (bounded cache)
            if let Some(oldest) =
                self.entries.iter().min_by_key(|(_, (_, seq))| *seq).map(|(k, _)| k.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        Ok(snapshot)
    }

    /// Remove every entry whose revision is NOT the given one (undo/new
    /// revisions: proven isolation without touching other revisions'
    /// still-valid snapshots).
    pub fn retain_only_revision(&mut self, document: &ProjectDocument) {
        let current = document.revision.as_u64();
        self.entries.retain(|k, _| k.revision == current);
    }

    /// Drop everything (editor-initiated cache policy only — never called
    /// inside evaluation).
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
