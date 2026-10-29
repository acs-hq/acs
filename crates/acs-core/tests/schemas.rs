//! Drift test: the schemas checked in under `spec/schemas/` must equal the ones generated from the
//! Rust types. Run with `ACS_UPDATE_SCHEMAS=1` to rewrite them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn schema_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec/schemas")
}

#[test]
fn checked_in_schemas_match_generated_schemas() {
    let dir = schema_dir();
    let generated = acs_core::schema::all();

    if std::env::var_os("ACS_UPDATE_SCHEMAS").is_some_and(|v| v == "1") {
        fs::create_dir_all(&dir).unwrap();
        let wanted: BTreeSet<_> = generated.iter().map(|s| s.file_name.clone()).collect();
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            if !wanted.contains(entry.file_name().to_str().unwrap()) {
                fs::remove_file(entry.path()).unwrap();
            }
        }
        for schema in &generated {
            fs::write(dir.join(&schema.file_name), &schema.contents).unwrap();
        }
        return;
    }

    let mut problems = Vec::new();
    for schema in &generated {
        match fs::read_to_string(dir.join(&schema.file_name)) {
            Ok(on_disk) if on_disk.replace("\r\n", "\n") == schema.contents => {}
            Ok(_) => problems.push(format!(
                "{} differs from the generated schema",
                schema.file_name
            )),
            Err(err) => problems.push(format!("{} cannot be read: {err}", schema.file_name)),
        }
    }
    let expected: BTreeSet<_> = generated.iter().map(|s| s.file_name.as_str()).collect();
    for entry in fs::read_dir(&dir).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        if !expected.contains(name.as_str()) {
            problems.push(format!(
                "{name} is in spec/schemas/ but is not generated from any type"
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "spec/schemas/ is out of date:\n  {}\nIf the type change is intended, regenerate with:\n  \
         ACS_UPDATE_SCHEMAS=1 cargo test -p acs-core --test schemas\nand commit the result.",
        problems.join("\n  ")
    );
}
