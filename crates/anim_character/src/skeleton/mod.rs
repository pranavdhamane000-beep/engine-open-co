//! Skeleton, bone hierarchy and topological evaluation (spec §27 Data structures).
//!
//! Skeleton is immutable after construction — built from validated RigDefinition.
//! Topological order is cached and used for all parent-before-child passes.

use crate::error::{CharError, CharResult};
use crate::limits::JointLimit;
use crate::pose::LocalTransform;
use crate::BoneId;
use crate::RigDefinitionId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Bone definition in the skeleton.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bone {
    pub id: BoneId,
    pub parent: Option<BoneId>,
    pub rest: LocalTransform,
    pub length: f64,
    pub limit: JointLimit,
    pub deform: bool,
}

impl Bone {
    pub fn new(
        id: BoneId,
        parent: Option<BoneId>,
        rest: LocalTransform,
        length: f64,
        limit: JointLimit,
        deform: bool,
    ) -> Self {
        Self { id, parent, rest, length, limit, deform }
    }
}

/// Complete skeleton with cached topological order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Skeleton {
    bones: Vec<Bone>,
    index: BTreeMap<BoneId, usize>,
    topo: Vec<BoneId>,
    rig_id: RigDefinitionId,
}

impl Skeleton {
    /// Build a skeleton from a validated RigDefinition.
    pub fn from_rig(rig: &crate::schema::RigDefinition) -> CharResult<Self> {
        // Check for duplicate bone IDs
        let mut seen = BTreeSet::new();
        for bone in &rig.bones {
            if !seen.insert(bone.id) {
                return Err(CharError::DuplicateBoneId(bone.id));
            }
        }

        // Check all parents exist
        let bone_ids: BTreeMap<BoneId, &Bone> =
            rig.bones.iter().map(|b| (b.id, b)).collect();
        for bone in &rig.bones {
            if let Some(parent) = bone.parent {
                if !bone_ids.contains_key(&parent) {
                    return Err(CharError::MissingParent { bone: bone.id, parent });
                }
            }
        }

        // Check for cycles using DFS
        let mut visited = BTreeSet::new();
        let mut rec_stack = BTreeSet::new();
        let children = build_child_map(&rig.bones);

        fn dfs(
            bone_id: BoneId,
            children: &BTreeMap<BoneId, Vec<BoneId>>,
            visited: &mut BTreeSet<BoneId>,
            rec_stack: &mut BTreeSet<BoneId>,
        ) -> CharResult<()> {
            visited.insert(bone_id);
            rec_stack.insert(bone_id);
            if let Some(kids) = children.get(&bone_id) {
                for child in kids {
                    if rec_stack.contains(child) {
                        return Err(CharError::CycleDetected(*child));
                    }
                    if !visited.contains(child) {
                        dfs(*child, children, visited, rec_stack)?;
                    }
                }
            }
            rec_stack.remove(&bone_id);
            Ok(())
        }

        // Check for cycles starting from roots
        for bone in &rig.bones {
            if bone.parent.is_none() {
                dfs(bone.id, &children, &mut visited, &mut rec_stack)?;
            }
        }

        // Build topological order using Kahn's algorithm
        let topo = topological_order(&rig.bones, &children)?;

        // Build index map
        let mut index = BTreeMap::new();
        for (idx, bone) in rig.bones.iter().enumerate() {
            index.insert(bone.id, idx);
        }

        Ok(Skeleton { bones: rig.bones.clone(), index, topo, rig_id: rig.id })
    }

    pub fn rig_id(&self) -> RigDefinitionId {
        self.rig_id
    }

    pub fn bones(&self) -> &[Bone] {
        &self.bones
    }

    pub fn bone(&self, id: BoneId) -> CharResult<&Bone> {
        self.index.get(&id).map(|&idx| &self.bones[idx]).ok_or(CharError::BoneNotFound(id))
    }

    pub fn parent(&self, id: BoneId) -> CharResult<Option<BoneId>> {
        self.bone(id).map(|b| b.parent)
    }

    pub fn children(&self, id: BoneId) -> Vec<BoneId> {
        self.bones.iter().filter(|b| b.parent == Some(id)).map(|b| b.id).collect()
    }

    pub fn topological_order(&self) -> &[BoneId] {
        &self.topo
    }

    pub fn len(&self) -> usize {
        self.bones.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bones.is_empty()
    }
}

fn build_child_map(bones: &[Bone]) -> BTreeMap<BoneId, Vec<BoneId>> {
    let mut map: BTreeMap<BoneId, Vec<BoneId>> = BTreeMap::new();
    for bone in bones {
        if let Some(parent) = bone.parent {
            map.entry(parent).or_default().push(bone.id);
        }
    }
    map
}

fn topological_order(
    bones: &[Bone],
    children: &BTreeMap<BoneId, Vec<BoneId>>,
) -> CharResult<Vec<BoneId>> {
    let mut indegree: BTreeMap<BoneId, usize> = BTreeMap::new();
    for bone in bones {
        let entry = indegree.entry(bone.id).or_insert(0);
        if bone.parent.is_some() {
            *entry += 1;
        }
        // Ensure parent exists in map
        if let Some(parent) = bone.parent {
            indegree.entry(parent).or_insert(0);
        }
    }

    let mut queue: Vec<BoneId> =
        indegree.iter().filter(|(_, &deg)| deg == 0).map(|(&id, _)| id).collect();

    let mut topo = Vec::with_capacity(bones.len());
    while let Some(id) = queue.pop() {
        topo.push(id);
        if let Some(kids) = children.get(&id) {
            for &kid in kids {
                if let Some(deg) = indegree.get_mut(&kid) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(kid);
                    }
                }
            }
        }
    }

    if topo.len() != bones.len() {
        return Err(CharError::CycleDetected(topo.last().copied().unwrap_or_default()));
    }
    Ok(topo)
}
