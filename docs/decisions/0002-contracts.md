# 0002: Contracts

Date: 2026-10-29 (WP0.2)

## Context

WP0.2 freezes the wire contracts of plan section 4 as Rust types in `acs-core`, generates JSON Schemas from them into `spec/schemas/`, and writes `spec/SPEC.md` (draft 0.1). The orchestrator's brief fixed most shapes; this record lists the choices made beyond it and where the implementation extends it.

## Decision

**Crates** (versions from crates.io on 2026-10-29, declared in root `[workspace.dependencies]`):

- `schemars 1.2.3` with the `jiff02` feature; emits JSON Schema draft 2020-12. Derive honors `#[serde(try_from)]`/`into` by using the wire type's schema, and `#[schemars(extend(...))]` adds `pattern`, `const` and `minLength`, which is how the newtypes carry their validation into the schemas.
- `jiff 0.2.38` (`default-features = false`, features `std` and `serde`) for timestamps: the brief prefers it; it has a first-class schemars integration, serializes `Timestamp` as RFC 3339 with `Z`, and avoids pulling in tzdb. `jiff::Timestamp` is re-exported as `acs_core::Timestamp`.
- `serde 1.0.229`, `serde_json 1.0.154`, `sha2 0.11.1` (pilot id, content hashes).
- Dev only: `jsonschema 0.60.2` (`default-features = false`; supports draft 2020-12 and `format: date-time`) and `toml 1.1.8`. `toml` is deliberately not a runtime dependency: policy loading is WP1.1's job, and `acs-core` only splits lore front matter as a string.
- `cargo deny check` passes with no change to `deny.toml`.

**Newtypes.** All validated at deserialize time with messages that name the value and the rule: the five prefixed ids (body is 1 to 64 characters of `[0-9a-z_-]`), `SpecVersion` (a unit struct that serializes as `"acs/0.1"` and rejects every other value), `RepoPath` (non-empty; invalid if it starts with `/`, `\` or a drive letter; `..` and normalization are WP1.3's), `SymbolPath` (non-empty dot-separated segments; `contains` is segment-prefix), `ContentHash` (64 lowercase hex; `ContentHash::of(bytes)` hashes), `Owner` (`Pilot(email)` or `Team(name)`, one string on the wire). Each carries a `pattern` in its schema; a test mutates example documents and requires that the Rust types and the schemas accept and reject exactly the same cases, which also guards the hand-written regexes.

**Claim scope.** `ClaimScope` is an untagged enum so the wire shape is exactly `{"file"}` or `{"symbol","symbol_path"}`. Serialization and the schema are derived (variants have `additionalProperties: false`); `Deserialize` is hand-written over a flat struct so a malformed scope yields one clear error instead of serde's "did not match any variant".

**Defaults and strictness.** Wire types (`ToolEvent`, `Decision`) ignore unknown fields. Policy types deny them. `Guards` and `Territory` have optional `spec`; `Config` requires it. `Config.claim_ttl_secs` and `sync_interval_secs` are `NonZeroU64` (zero is meaningless and the schema says `minimum: 1`); `Claim.ttl_secs` stays `u64` as the brief says. Serialization omits `None` and empty collections on wire types so output is compact; the two content-hash fields of `FileChange` are always written (`null` has a meaning there).

**Schemas.** `acs_core::schema::all()` generates them; `tests/schemas.rs` compares with `spec/schemas/` (after `\r\n` to `\n`), fails on stray files, and rewrites with `ACS_UPDATE_SCHEMAS=1`. Object keys are sorted by our own pass so the output does not depend on whether another crate enables `serde_json/preserve_order`. No `$id` is set: there is no agreed schema URL yet. `guard_decisions.counts` adds `propertyNames` referencing `ReasonCode`, which schemars does not emit for enum-keyed maps.

**SPEC.md and tests.** A test (`tests/spec_doc.rs`) checks that the reason-code table, the default sealed list and the pilot-id test vectors in `SPEC.md` agree with the code.

**Extensions to the brief.**

- `SessionRecord` has a required `spec` field. Ledger records are pushed to a shared remote and read by other versions, so they need a version marker as much as `ClaimsDocument` does.
- `ClaimHistoryEntry.release_reason` is optional alongside `released_at` (a claim may be open when the record is assembled).
- Added helpers: `ReasonCode::{ALL, as_str, typical_outcomes}`, `Decision::new`, `ClaimScope::path`, `ContentHash::of`, `Config::default`, `split_front_matter`, and the public `schema` module.
- `SPEC.md` states that a `sealed` territory zone also returns `sealed_path`, and that implementations without ownership enforcement treat `owned` as `shared`.

## Consequences

- Any change to a public type changes a schema and fails the drift test until the schemas are regenerated and committed; this is the intended freeze mechanism.
- `Config` rejects unknown keys, so later WPs that add settings (for example a lore injection cap) must add the field to `acs-core` and regenerate the schema.
- The `ask` versus `deny` precedence between pipeline steps, the glob dialect and message templates are left open in `SPEC.md` (section 12) for WP1.1 and WP1.5 to settle.
- `path_or_command` in notable guard decisions can contain command text that gets pushed to the remote; recorders must redact (WP4.1). The type cannot enforce it.
- Reason codes are a closed enum: a record containing a code from a newer draft fails to deserialize here, which is consistent with the exact `spec` version check.
