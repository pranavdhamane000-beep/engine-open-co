//! Payload handlers and schema dispatch (spec 12.3, 12.2, 12.5).
//!
//! The registry is the validation/dispatch FRONT DOOR for external
//! automation: JSON payloads with versioned schemas arrive here, are
//! shape-checked against the dispatched type, and are converted into typed
//! Commands. UI/headless callers may hand-construct envelopes; automation
//! goes through the registry. No privileged mutation route exists
//! (spec 12.10) — everything ends at the same CommandEnvelope → processor.

use crate::command::Command;
use crate::envelope::{CommandEnvelope, COMMAND_SCHEMA_VERSION};
use crate::{CommandError, CommandResult};
use serde_json::{Map as JsonMap, Value};

/// Build one typed Command from a versionless payload VALUE (the object's
/// `"type"` tag selects the handler). Schema version is a whole-envelope
/// concern; this validates the single command's dispatch shape.
pub fn command_from_value(value: &Value) -> CommandResult<Command> {
    let obj = value
        .as_object()
        .ok_or_else(|| CommandError::UnknownCommand("payload is not an object".into()))?;
    let tag = obj
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CommandError::UnknownCommand("missing type tag".into()))?;

    // Schema dispatch table: tag → payload handler.
    let payload = Value::Object(strip_tag(obj));
    let command = match tag {
        "upsert_asset" => {
            serde_json::from_value::<crate::models::UpsertAssetPayload>(payload)
                .map(Command::from)
        }
        "replace_asset" => {
            serde_json::from_value::<crate::models::ReplaceAssetPayload>(payload)
                .map(Command::from)
        }
        "remove_asset" => {
            serde_json::from_value::<crate::models::RemoveAssetPayload>(payload)
                .map(Command::from)
        }
        "upsert_scene" => {
            serde_json::from_value::<crate::models::UpsertScenePayload>(payload)
                .map(Command::from)
        }
        "rename_scene" => {
            serde_json::from_value::<crate::models::RenameScenePayload>(payload)
                .map(Command::from)
        }
        "remove_scene" => {
            serde_json::from_value::<crate::models::RemoveScenePayload>(payload)
                .map(Command::from)
        }
        "upsert_node" => {
            serde_json::from_value::<crate::models::UpsertNodePayload>(payload)
                .map(Command::from)
        }
        "remove_node" => {
            serde_json::from_value::<crate::models::RemoveNodePayload>(payload)
                .map(Command::from)
        }
        "set_node_property" => {
            serde_json::from_value::<crate::models::SetNodePropertyPayload>(payload)
                .map(Command::from)
        }
        "remove_node_property_key" => {
            serde_json::from_value::<crate::models::RemoveNodePropertyKeyPayload>(payload)
                .map(Command::from)
        }
        "set_frame_rate" => {
            serde_json::from_value::<crate::models::SetFrameRatePayload>(payload)
                .map(Command::from)
        }
        other => return Err(CommandError::UnknownCommand(other.to_string())),
    };
    command.map_err(|e| {
        CommandError::Validation(format!("command '{tag}' payload rejected: {e}"))
    })
}

fn strip_tag(obj: &JsonMap<String, Value>) -> JsonMap<String, Value> {
    obj.iter()
        .filter(|(k, _)| k.as_str() != "type")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Whole-envelope dispatch from external JSON (the automation route):
/// schema version checked FIRST, commands dispatched per-payload.
pub fn envelope_from_json(json: &Value) -> CommandResult<CommandEnvelope> {
    let schema_version =
        json.get("schema_version").and_then(|v| v.as_u64()).ok_or_else(|| {
            CommandError::Validation("envelope missing schema_version".into())
        })? as u32;
    if schema_version != COMMAND_SCHEMA_VERSION {
        return Err(CommandError::UnsupportedSchema {
            found: schema_version,
            supported: COMMAND_SCHEMA_VERSION,
        });
    }
    let envelope: CommandEnvelope = serde_json::from_value(json.clone())
        .map_err(|e| CommandError::Validation(format!("envelope payload rejected: {e}")))?;
    // The typed enum reserialization already validated tags; command
    // contents are validated at apply time (same path as UI callers).
    Ok(envelope)
}
