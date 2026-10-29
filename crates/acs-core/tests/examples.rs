//! Every example under `spec/examples/` round-trips through its Rust type and validates against
//! its generated JSON Schema; and the Rust types and the schemas agree on what is invalid.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::fmt::Debug;
use std::fs;
use std::path::PathBuf;

use acs_core::{
    Claim, ClaimsDocument, Config, Decision, Guards, LoreFrontMatter, Notice, Seat, SessionRecord,
    Territory, ToolEvent, schema, split_front_matter,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

fn examples_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/examples")
}

fn validator(name: &str) -> jsonschema::Validator {
    let file = schema::all()
        .into_iter()
        .find(|s| s.file_name == format!("{name}.schema.json"))
        .unwrap_or_else(|| panic!("no schema named {name}"));
    let schema: Value = serde_json::from_str(&file.contents).unwrap();
    jsonschema::draft202012::options()
        .should_validate_formats(true)
        .build(&schema)
        .unwrap()
}

fn assert_valid(name: &str, instance: &Value, what: &str) {
    let errors: Vec<String> = validator(name)
        .iter_errors(instance)
        .map(|e| format!("{e} at {}", e.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "{what} does not validate against {name}.schema.json:\n  {}",
        errors.join("\n  ")
    );
}

/// The schema name an example file belongs to.
fn schema_name(file_name: &str) -> &str {
    match file_name.split('.').next().unwrap() {
        "lore" => "lore_front_matter",
        other => other,
    }
}

/// Loads an example as a JSON value (TOML and Markdown front matter are converted).
fn load_value(file_name: &str) -> Value {
    let text = fs::read_to_string(examples_dir().join(file_name)).unwrap();
    match file_name.rsplit('.').next().unwrap() {
        "json" => serde_json::from_str(&text).unwrap(),
        "toml" => toml::from_str(&text).unwrap(),
        "md" => toml::from_str(split_front_matter(&text).unwrap().0).unwrap(),
        ext => panic!("unexpected example extension {ext}"),
    }
}

fn round_trip<T: Serialize + DeserializeOwned + PartialEq + Debug>(
    name: &str,
    file_name: &str,
    parse: impl Fn(&str) -> T,
    print: impl Fn(&T) -> String,
) {
    let text = fs::read_to_string(examples_dir().join(file_name)).unwrap();
    let first = parse(&text);
    let second = parse(&print(&first));
    assert_eq!(first, second, "{file_name} changed after a round trip");
    assert_valid(name, &load_value(file_name), file_name);
    assert_valid(
        name,
        &serde_json::to_value(&first).unwrap(),
        &format!("re-serialized {file_name}"),
    );
}

fn json_example<T: Serialize + DeserializeOwned + PartialEq + Debug>(name: &str, file_name: &str) {
    round_trip::<T>(
        name,
        file_name,
        |text| serde_json::from_str(text).unwrap_or_else(|e| panic!("{file_name}: {e}")),
        |value| serde_json::to_string(value).unwrap(),
    );
}

fn toml_example<T: Serialize + DeserializeOwned + PartialEq + Debug>(name: &str, file_name: &str) {
    round_trip::<T>(
        name,
        file_name,
        |text| toml::from_str(text).unwrap_or_else(|e| panic!("{file_name}: {e}")),
        |value| toml::to_string(value).unwrap(),
    );
}

#[test]
fn every_example_round_trips_and_validates() {
    let mut covered = BTreeSet::new();
    for entry in fs::read_dir(examples_dir()).unwrap() {
        let file_name = entry.unwrap().file_name().into_string().unwrap();
        let name = schema_name(&file_name);
        covered.insert(name.to_owned());
        match (name, file_name.rsplit('.').next().unwrap()) {
            ("tool_event", "json") => json_example::<ToolEvent>(name, &file_name),
            ("decision", "json") => json_example::<Decision>(name, &file_name),
            ("seat", "json") => json_example::<Seat>(name, &file_name),
            ("claim", "json") => json_example::<Claim>(name, &file_name),
            ("session_record", "json") => json_example::<SessionRecord>(name, &file_name),
            ("notice", "json") => json_example::<Notice>(name, &file_name),
            ("claims_document", "json") => json_example::<ClaimsDocument>(name, &file_name),
            ("config", "toml") => toml_example::<Config>(name, &file_name),
            ("guards", "toml") => toml_example::<Guards>(name, &file_name),
            ("territory", "toml") => toml_example::<Territory>(name, &file_name),
            ("lore_front_matter", "md") => round_trip::<LoreFrontMatter>(
                name,
                &file_name,
                |text| toml::from_str(split_front_matter(text).unwrap().0).unwrap(),
                |value| format!("+++\n{}+++\n", toml::to_string(value).unwrap()),
            ),
            _ => panic!("example {file_name} has no test"),
        }
    }
    let schemas: BTreeSet<_> = schema::all()
        .into_iter()
        .map(|s| s.file_name.trim_end_matches(".schema.json").to_owned())
        .collect();
    assert_eq!(covered, schemas, "every schema needs at least one example");
}

fn rust_accepts(name: &str, value: &Value) -> bool {
    macro_rules! parses {
        ($ty:ty) => {
            serde_json::from_value::<$ty>(value.clone()).is_ok()
        };
    }
    match name {
        "tool_event" => parses!(ToolEvent),
        "decision" => parses!(Decision),
        "seat" => parses!(Seat),
        "claim" => parses!(Claim),
        "session_record" => parses!(SessionRecord),
        "notice" => parses!(Notice),
        "claims_document" => parses!(ClaimsDocument),
        "config" => parses!(Config),
        "guards" => parses!(Guards),
        "territory" => parses!(Territory),
        "lore_front_matter" => parses!(LoreFrontMatter),
        other => panic!("unknown schema {other}"),
    }
}

/// Applies a mutation to a copy of an example: `Some(v)` sets (or adds) the value at the JSON
/// pointer, `None` removes it.
fn mutate(example: &Value, pointer: &str, new: Option<Value>) -> Value {
    let mut doc = example.clone();
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    let parent = doc
        .pointer_mut(parent)
        .unwrap_or_else(|| panic!("no parent for {pointer}"));
    match (parent, new) {
        (Value::Object(map), Some(v)) => {
            map.insert(key.to_owned(), v);
        }
        (Value::Object(map), None) => {
            map.remove(key)
                .unwrap_or_else(|| panic!("nothing to remove at {pointer}"));
        }
        (Value::Array(items), Some(v)) => items[key.parse::<usize>().unwrap()] = v,
        _ => panic!("cannot mutate {pointer}"),
    }
    doc
}

#[test]
fn rust_types_and_schemas_agree_on_what_is_valid() {
    let long_hash = "a".repeat(64);
    #[rustfmt::skip]
    let cases: Vec<(&str, &str, Option<Value>, bool)> = vec![
        // Wire types: unknown fields are tolerated, unknown enum values are not.
        ("tool_event.pre_tool.json", "/spec", Some(json!("acs/0.2")), false),
        ("tool_event.pre_tool.json", "/phase", Some(json!("midnight")), false),
        ("tool_event.pre_tool.json", "/tool/kind", Some(json!("teleport")), false),
        ("tool_event.pre_tool.json", "/future_field", Some(json!(1)), true),
        ("tool_event.pre_tool.json", "/harness", None, false),
        ("tool_event.pre_tool.json", "/paths", None, true),
        ("tool_event.pre_tool.json", "/paths/0", Some(json!("/abs/path/stays/raw.ts")), true),
        ("decision.deny.json", "/outcome", Some(json!("redirect")), false),
        ("decision.deny.json", "/outcome", Some(json!("queue")), true),
        ("decision.deny.json", "/reason_code", Some(json!("bogus")), false),
        ("decision.deny.json", "/message", None, true),
        ("decision.deny.json", "/claims_acquired", Some(json!(["seat_1"])), false),
        ("decision.deny.json", "/future_field", Some(json!(1)), true),
        // Identifiers, timestamps, scopes and paths.
        ("claim.file.json", "/claim_id", Some(json!("seat_x")), false),
        ("claim.file.json", "/claim_id", Some(json!("clm_")), false),
        ("claim.file.json", "/claim_id", Some(json!("clm_UPPER")), false),
        ("claim.file.json", "/claim_id", Some(json!("clm_ok-1_x")), true),
        ("claim.file.json", "/acquired_at", Some(json!("yesterday")), false),
        ("claim.file.json", "/ttl_secs", Some(json!(-1)), false),
        ("claim.file.json", "/ttl_secs", Some(json!(0)), true),
        ("claim.file.json", "/origin", Some(json!("robot")), false),
        ("claim.file.json", "/scope", Some(json!({"file": ""})), false),
        ("claim.file.json", "/scope", Some(json!({"file": "/etc/passwd"})), false),
        ("claim.file.json", "/scope", Some(json!({"file": "\\\\host\\share"})), false),
        ("claim.file.json", "/scope", Some(json!({"file": "C:/x"})), false),
        ("claim.file.json", "/scope", Some(json!({"file": "c:x"})), false),
        ("claim.file.json", "/scope", Some(json!({"file": "ab:c"})), true),
        ("claim.file.json", "/scope", Some(json!({"file": "a", "symbol": "a", "symbol_path": "f"})), false),
        ("claim.file.json", "/scope", Some(json!({"file": "a", "extra": 1})), false),
        ("claim.file.json", "/scope", Some(json!({"symbol": "a"})), false),
        ("claim.file.json", "/scope", Some(json!({"symbol": "a", "symbol_path": "A..b"})), false),
        ("claim.file.json", "/scope", Some(json!({"symbol": "a", "symbol_path": ""})), false),
        ("claim.file.json", "/scope", Some(json!({"symbol": "a", "symbol_path": "A.b"})), true),
        ("claim.symbol.json", "/scope/symbol_path", Some(json!("RefundService")), true),
        ("seat.json", "/branch", None, true),
        ("seat.json", "/branch", Some(Value::Null), true),
        ("notice.json", "/kind", Some(json!("gossip")), false),
        ("notice.json", "/target_seat", Some(json!("ses_1")), false),
        ("notice.json", "/delivered_at", Some(json!("2026-10-29T14:25:00Z")), true),
        // Session records.
        ("session_record.json", "/spec", None, false),
        ("session_record.json", "/ended_reason", Some(json!("vanished")), false),
        ("session_record.json", "/ended_reason", Some(json!("crashed")), true),
        ("session_record.json", "/files_changed/0/pre_hash", Some(json!("ABC")), false),
        ("session_record.json", "/files_changed/0/pre_hash", Some(json!(long_hash.to_uppercase())), false),
        ("session_record.json", "/files_changed/0/pre_hash", Some(json!(long_hash)), true),
        ("session_record.json", "/files_changed/0/pre_hash", Some(Value::Null), true),
        ("session_record.json", "/files_changed/0/path", Some(json!("/abs")), false),
        ("session_record.json", "/guard_decisions/counts", Some(json!({"nope": 1})), false),
        ("session_record.json", "/guard_decisions/counts", Some(json!({"ok": -1})), false),
        ("session_record.json", "/claims/0/release_reason", Some(json!("forgotten")), false),
        ("session_record.json", "/lore_loaded/0", Some(json!("7f3a")), false),
        // Policy files reject unknown fields and bad values.
        ("config.toml", "/spec", Some(json!("acs/0.2")), false),
        ("config.toml", "/claim_ttl_secs", Some(json!(0)), false),
        ("config.toml", "/claim_ttl", Some(json!(5)), false),
        ("config.toml", "/ledger", None, true),
        ("config.toml", "/spec", None, false),
        ("config.toml", "/ledger/store_promts", Some(json!(true)), false),
        ("guards.toml", "/sealed_paths", Some(json!([])), false),
        ("guards.toml", "/destructive/ban", Some(json!("rm")), false),
        ("guards.toml", "/destructive/max_files_deleted_per_command", Some(json!(-1)), false),
        ("guards.toml", "/spec", Some(json!("acs/0.1")), true),
        ("guards.toml", "/spec", Some(json!("acs/9")), false),
        ("territory.toml", "/zone/0/kind", Some(json!("private")), false),
        ("territory.toml", "/zone/0/owners", Some(json!(["nobody"])), false),
        ("territory.toml", "/zone/0/owners", Some(json!(["team:"])), false),
        ("territory.toml", "/zone/0/owners", Some(json!(["a b@c.d"])), false),
        ("territory.toml", "/zone/0/owners", Some(json!(["team:core-infra", "a@b.c"])), true),
        ("territory.toml", "/zone/0/owner", Some(json!("a@b.c")), false),
        ("territory.toml", "/zone/0/paths", None, false),
        ("lore.md", "/owner", Some(json!("nobody")), false),
        ("lore.md", "/id", Some(json!("lore_")), false),
        ("lore.md", "/extra", Some(json!(1)), false),
        ("lore.md", "/scope", None, false),
    ];
    let mut failures = Vec::new();
    for (file, pointer, new, expected) in cases {
        let name = schema_name(file);
        let doc = mutate(&load_value(file), pointer, new.clone());
        let by_rust = rust_accepts(name, &doc);
        let by_schema = validator(name).is_valid(&doc);
        if by_rust != expected || by_schema != expected {
            failures.push(format!(
                "{file} {pointer} = {new:?}: expected valid={expected}, rust={by_rust}, schema={by_schema}"
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "disagreements:\n  {}",
        failures.join("\n  ")
    );
}
