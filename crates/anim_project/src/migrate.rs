//! Sequential schema migrations and tests (spec 12.3, 12.5 step 8).
//!
//! v0 (the pre-WP03 draft shape: flat scene array, numeric rate fields) is
//! the only historical version. The chain is applied ONE STEP AT A TIME and
//! each step's output is re-parsed — a failing step is a typed
//! MigrationFailure, never a field-by-field silent repair.

use crate::ProjectError;
use serde_json::{json, Value};

/// Parse bytes, detect the schema version, apply every needed migration in
/// sequence, then parse + validate the result as the current version.
pub fn migrate_document(bytes: &[u8]) -> Result<crate::ProjectDocument, ProjectError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|e| ProjectError::Migration(format!("not JSON: {e}")))?;

    let version = value
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ProjectError::Migration("missing schema_version".into()))?
        as u32;

    let mut current = value;
    let mut current_version = version;
    while current_version < crate::PROJECT_SCHEMA_VERSION {
        current = migration_step(current_version, current)?;
        // chain proof: the step itself must carry the NEXT version
        let next =
            current.get("schema_version").and_then(|v| v.as_u64()).ok_or_else(|| {
                ProjectError::Migration("step dropped schema_version".into())
            })? as u32;
        if next != current_version + 1 {
            return Err(ProjectError::Migration(format!(
                "migration step {current_version} did not set schema_version to {}",
                current_version + 1
            )));
        }
        current_version = next;
    }

    if current_version != crate::PROJECT_SCHEMA_VERSION {
        return Err(ProjectError::UnsupportedSchema {
            found: current_version,
            supported: crate::PROJECT_SCHEMA_VERSION,
        });
    }

    let bytes = serde_json::to_vec(&current)
        .map_err(|e| ProjectError::Migration(format!("re-serialize: {e}")))?;
    crate::schema::parse_document(&bytes).map_err(|e| match e {
        ProjectError::UnsupportedSchema { .. } => e,
        other => ProjectError::Migration(format!("migrated document invalid: {other}")),
    })
}

fn migration_step(from: u32, value: Value) -> Result<Value, ProjectError> {
    match from {
        0 => migrate_v0_to_v1(value),
        other => Err(ProjectError::Migration(format!(
            "no migration path from schema version {other}"
        ))),
    }
}

/// v0 document shape (hex-documented historical draft):
/// { schema_version: 0, project_id, revision, rate_num, rate_den,
///   scenes: [ { id, title, nodes: [ { id, caption, parent: id|null } ] } ] }
fn migrate_v0_to_v1(value: Value) -> Result<Value, ProjectError> {
    let obj = value
        .as_object()
        .ok_or_else(|| ProjectError::Migration("v0 document is not an object".into()))?;

    let project_id = obj
        .get("project_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| ProjectError::Migration("v0: missing project_id".into()))?
        .to_string();
    let revision = obj.get("revision").and_then(|v| v.as_u64()).unwrap_or(0);
    let rate_num = obj
        .get("rate_num")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ProjectError::Migration("v0: missing rate_num".into()))?;
    let rate_den = obj
        .get("rate_den")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| ProjectError::Migration("v0: missing rate_den".into()))?;

    // rate validation happens through FrameRate parsing in schema::parse —
    // but a zero denominator must fail HERE, in the migration step itself.
    if rate_num == 0 || rate_den == 0 {
        return Err(ProjectError::Migration(format!(
            "v0: invalid rate {rate_num}/{rate_den}"
        )));
    }

    let mut scenes = serde_json::Map::new();
    if let Some(list) = obj.get("scenes").and_then(|v| v.as_array()) {
        for scene in list {
            let sid = scene
                .get("id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ProjectError::Migration("v0 scene missing id".into()))?
                .to_string();
            let name = scene
                .get("title")
                .and_then(|v| v.as_str())
                .unwrap_or("migrated scene")
                .to_string();

            let mut nodes = serde_json::Map::new();
            if let Some(node_list) = scene.get("nodes").and_then(|v| v.as_array()) {
                for node in node_list {
                    let nid = node
                        .get("id")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            ProjectError::Migration("v0 node missing id".into())
                        })?
                        .to_string();
                    let node_json = json!({
                        "name": node.get("caption").and_then(|v| v.as_str()).unwrap_or("migrated node"),
                        "parent": node.get("parent").filter(|v| !v.is_null()).cloned().unwrap_or(Value::Null),
                        "asset": Value::Null,
                        "properties": {},
                    });
                    nodes.insert(nid, node_json);
                }
            }

            scenes.insert(
                sid,
                json!({
                    "name": name,
                    "nodes": nodes,
                    "tracks": {},
                    "markers": [],
                }),
            );
        }
    }

    Ok(json!({
        "schema_version": 1,
        "project_id": project_id,
        "revision": revision,
        "frame_rate": format!("{rate_num}/{rate_den}"),
        "assets": { "schema_version": 1, "records": {} },
        "scenes": scenes,
    }))
}
