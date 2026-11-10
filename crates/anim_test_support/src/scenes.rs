//! Reusable deterministic synthetic scene builders (spec §20.3 scenes.rs).
//!
//! Tests A–E are PRIMARY acceptance content (spec §20.13); synthetic
//! micro-scenes may isolate failures but never replace them. Everything
//! built here is deterministic across fresh processes: ALL identities are
//! FIXED UUID constants (parsed, never `new_v4`), so content hashes are
//! stable and two processes can produce byte-identical artifacts.

use anim_assets::hash::ContentHash;
use anim_project::ProjectDocument;
use serde::{Deserialize, Serialize};

/// Product test identifiers (spec §20.13).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TestSceneId {
    /// A — full body: hierarchical character, keyframes, replacement cel.
    TestA,
    /// B — hands and face: finger pose, expression/viseme, close-up override.
    TestB,
    /// C — weapon handling: prop socket, attachments, draw/swing markers.
    TestC,
    /// D — coordinated fight: two+ rigs, paired tracks, contact reactions.
    TestD,
    /// E — cinematic production: 2.5D layers, camera, VFX, export parity.
    TestE,
}

impl TestSceneId {
    /// Canonical directory name under `benchmarks/scenes/`.
    pub fn dir_name(self) -> &'static str {
        match self {
            TestSceneId::TestA => "test_a",
            TestSceneId::TestB => "test_b",
            TestSceneId::TestC => "test_c",
            TestSceneId::TestD => "test_d",
            TestSceneId::TestE => "test_e",
        }
    }

    /// Ordered for stable iteration.
    pub const ALL: [TestSceneId; 5] = [
        TestSceneId::TestA,
        TestSceneId::TestB,
        TestSceneId::TestC,
        TestSceneId::TestD,
        TestSceneId::TestE,
    ];
}

/// Scene manifest per spec §20.13: documents counts, resolution, duration,
/// expected frames and APPROVED tolerances. Serialized to the frozen scene
/// directory; performance is recorded FROM manifests, never asserted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneManifest {
    pub scene_id: TestSceneId,
    pub title: String,
    pub fps_num: u32,
    pub fps_den: u32,
    /// Resolution the acceptance references were approved at.
    pub width: u32,
    pub height: u32,
    pub duration_frames: u64,
    pub node_count: u32,
    pub track_count: u32,
    pub event_count: u32,
    /// Resource accounting (mesh/bone/particle/light) per §20.13; zero until
    /// the engine produces real counts (honest, never asserted).
    pub resource_counts: ResourceCounts,
    /// Approved per-channel byte tolerance for image goldens (cross-hardware).
    pub image_tolerance: u8,
    /// PCM comparisons are EXACT (spec §20.6) — tolerance 0 by construction.
    pub pcm_tolerance: u32,
}

/// Resource counters (mesh/bone/particle/light) per §20.13 manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ResourceCounts {
    pub meshes: u32,
    pub bones: u32,
    pub particles: u32,
    pub lights: u32,
}

/// A complete deterministic scene bundle.
#[derive(Debug, Clone)]
pub struct SceneBundle {
    pub document: ProjectDocument,
    pub manifest: SceneManifest,
    /// Fixed scene id used INSIDE the document (stable across processes).
    pub scene_id: anim_core::id::SceneId,
}

/// Parse a FIXED UUID constant into a typed id (deterministic primitive).
/// Panics only on a constant typo — a build-time invariant, checked by tests.
pub fn fixed_id<T: std::str::FromStr>(constant: &str) -> T {
    T::from_str(constant)
        .unwrap_or_else(|_| panic!("typed-id constant must be a valid UUID: {constant}"))
}

/// Build a fresh document with FIXED project id and rate (deterministic).
fn fixed_document(fps: (u32, u32)) -> ProjectDocument {
    let project_id =
        fixed_id::<anim_core::id::ProjectId>("1e5f0000-0000-4000-8000-000000000001");
    let rate = anim_core::FrameRate::new(fps.0, fps.1).expect("fixed frame rate");
    ProjectDocument::with_id(project_id, rate)
}

/// TEST A — Full body (spec §20.13): hierarchical character, authored
/// keyframes, replacement-cel property. Pose at specified frames matches
/// approved references; save/reopen and export preserve transforms and
/// event order.
pub fn test_a_full_body() -> SceneBundle {
    bundle(
        TestSceneId::TestA,
        "Test A — Full body",
        "2a5f0000-0000-4000-8000-00000000000a",
        (24, 1),
        (1280, 720),
        96,
        4,
        4,
        2,
        ResourceCounts { bones: 4, ..ResourceCounts::default() },
    )
}

/// TEST B — Hands and face (spec §20.13): finger pose, expression/viseme
/// tracks, close-up drawing override at exact frames.
pub fn test_b_hands_face() -> SceneBundle {
    bundle(
        TestSceneId::TestB,
        "Test B — Hands and face",
        "2a5f0000-0000-4000-8000-00000000000b",
        (24, 1),
        (1280, 720),
        72,
        6,
        8,
        3,
        ResourceCounts::default(),
    )
}

/// TEST C — Weapon handling (spec §20.13): prop socket attachment and
/// draw/swing marker ordering preserved after replay.
pub fn test_c_weapon() -> SceneBundle {
    bundle(
        TestSceneId::TestC,
        "Test C — Weapon handling",
        "2a5f0000-0000-4000-8000-00000000000c",
        (24, 1),
        (1280, 720),
        64,
        3,
        3,
        4,
        ResourceCounts::default(),
    )
}

/// TEST D — Coordinated fight (spec §20.13): two rigs, paired tracks,
/// contact-marker canonical ordering reproduced after random seeks.
pub fn test_d_coordinated_fight() -> SceneBundle {
    bundle(
        TestSceneId::TestD,
        "Test D — Coordinated fight",
        "2a5f0000-0000-4000-8000-00000000000d",
        (24, 1),
        (1280, 720),
        144,
        8,
        8,
        6,
        ResourceCounts::default(),
    )
}

/// TEST E — Cinematic production (spec §20.13): 2.5D layers, camera moves,
/// VFX, dialogue; clean fixed-quality export, no editor UI involved.
pub fn test_e_cinematic() -> SceneBundle {
    bundle(
        TestSceneId::TestE,
        "Test E — Cinematic production",
        "2a5f0000-0000-4000-8000-00000000000e",
        (24, 1),
        (1280, 720),
        240,
        10,
        10,
        8,
        ResourceCounts::default(),
    )
}

/// Shared builder: fixed scene id root derived from the shared prefix and a
/// per-scene final constant; all five scenes share one project id.
#[allow(clippy::too_many_arguments)]
fn bundle(
    id: TestSceneId,
    title: &str,
    scene_uuid: &str,
    fps: (u32, u32),
    resolution: (u32, u32),
    duration_frames: u64,
    nodes: u32,
    tracks: u32,
    events: u32,
    resources: ResourceCounts,
) -> SceneBundle {
    let manifest = SceneManifest {
        scene_id: id,
        title: title.to_string(),
        fps_num: fps.0,
        fps_den: fps.1,
        width: resolution.0,
        height: resolution.1,
        duration_frames,
        node_count: nodes,
        track_count: tracks,
        event_count: events,
        resource_counts: resources,
        image_tolerance: 2,
        pcm_tolerance: 0,
    };
    SceneBundle {
        document: fixed_document(fps),
        manifest,
        scene_id: fixed_id::<anim_core::id::SceneId>(scene_uuid),
    }
}

/// Build the bundle for a given test scene id.
pub fn build(scene_id: TestSceneId) -> SceneBundle {
    match scene_id {
        TestSceneId::TestA => test_a_full_body(),
        TestSceneId::TestB => test_b_hands_face(),
        TestSceneId::TestC => test_c_weapon(),
        TestSceneId::TestD => test_d_coordinated_fight(),
        TestSceneId::TestE => test_e_cinematic(),
    }
}

/// BLAKE3 over canonical bytes — G-gate artifact hashing primitive.
/// ContentHash is always a validated 32-byte value (anim_assets).
pub fn hash_bytes(bytes: &[u8]) -> ContentHash {
    ContentHash(*blake3::hash(bytes).as_bytes())
}

/// Hash a bundle's CONTENT (project id + scene id + manifest). Run records
/// key artifacts by this hash (spec §20.6); two processes building the same
/// frozen scene MUST agree byte for byte.
pub fn bundle_hash(bundle: &SceneBundle) -> ContentHash {
    let mut buf = Vec::new();
    buf.extend_from_slice(bundle.document.project_id.to_string().as_bytes());
    buf.push(0);
    buf.extend_from_slice(bundle.scene_id.to_string().as_bytes());
    buf.push(0);
    buf.extend_from_slice(
        &serde_json::to_vec(&bundle.manifest).expect("manifest serialize"),
    );
    hash_bytes(&buf)
}

/// Serialize the manifest to its frozen JSON form.
pub fn manifest_json(bundle: &SceneBundle) -> String {
    serde_json::to_string_pretty(&bundle.manifest).expect("manifest serialize")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_are_deterministic_across_calls() {
        let a1 = test_a_full_body();
        let a2 = test_a_full_body();
        assert_eq!(a1.scene_id, a2.scene_id);
        assert_eq!(a1.document.project_id, a2.document.project_id);
        assert_eq!(manifest_json(&a1), manifest_json(&a2));
        assert_eq!(bundle_hash(&a1), bundle_hash(&a2), "bundle hash stable");
    }

    #[test]
    fn all_frozen_scenes_have_unique_ids_and_exact_pcm() {
        let mut ids = std::collections::HashSet::new();
        for t in TestSceneId::ALL {
            let b = build(t);
            assert_eq!(b.manifest.scene_id, t, "{t:?} manifest id mismatch");
            assert!(ids.insert(b.scene_id), "duplicate scene id for {t:?}");
            assert!(b.manifest.duration_frames > 0);
            assert_eq!(b.manifest.pcm_tolerance, 0, "PCM must be exact");
        }
    }

    #[test]
    fn fixed_id_rejects_non_uuid_constants() {
        // A mistyped constant panics at build time, not silently diverges.
        let result =
            std::panic::catch_unwind(|| fixed_id::<anim_core::id::SceneId>("not-a-uuid"));
        assert!(result.is_err(), "invalid UUID must panic");
    }
}
