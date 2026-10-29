//! JSON Schema generation for every top-level document of the spec.
//!
//! The schemas checked in under `spec/schemas/` are produced by [`all`]; a test regenerates them
//! and fails if the files differ. Output is deterministic: keys are sorted and the text is
//! pretty-printed with a trailing newline.

use schemars::{JsonSchema, schema_for};
use serde_json::{Map, Value};

use crate::decision::Decision;
use crate::event::ToolEvent;
use crate::policy_types::{Config, Guards, LoreFrontMatter, Territory};
use crate::records::{Claim, ClaimsDocument, Notice, Seat, SessionRecord};

/// A generated schema and the file it belongs in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaFile {
    /// File name relative to `spec/schemas/`, for example `tool_event.schema.json`.
    pub file_name: String,
    /// The schema as pretty-printed JSON ending in a newline.
    pub contents: String,
}

/// Generates the schema of every top-level document, in a fixed order.
#[must_use]
pub fn all() -> Vec<SchemaFile> {
    vec![
        render::<ToolEvent>("tool_event"),
        render::<Decision>("decision"),
        render::<Seat>("seat"),
        render::<Claim>("claim"),
        render::<SessionRecord>("session_record"),
        render::<Notice>("notice"),
        render::<ClaimsDocument>("claims_document"),
        render::<Config>("config"),
        render::<Guards>("guards"),
        render::<Territory>("territory"),
        render::<LoreFrontMatter>("lore_front_matter"),
    ]
}

fn render<T: JsonSchema>(name: &str) -> SchemaFile {
    let value = serde_json::to_value(schema_for!(T)).unwrap_or(Value::Null);
    let mut contents = serde_json::to_string_pretty(&sorted(value)).unwrap_or_default();
    contents.push('\n');
    SchemaFile {
        file_name: format!("{name}.schema.json"),
        contents,
    }
}

/// Rebuilds `value` with object keys in sorted order, whatever order the map type keeps.
fn sorted(value: Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, sorted(v)))
                    .collect::<Map<_, _>>(),
            )
        }
        Value::Array(items) => Value::Array(items.into_iter().map(sorted).collect()),
        other => other,
    }
}
