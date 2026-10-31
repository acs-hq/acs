//! Scripted multi-seat scenarios for release v0.1 (WP10.2): simulated agents drive the built `acs`
//! binary with canonical events, so no model is called. `tests/e2e/README.md` says which scenarios
//! are here and which arrive with later milestones.

#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "../../crates/acs-cli/tests/common/mod.rs"]
mod common;

mod acceptance;
mod husky;
mod junction;
mod offline_team;
mod sealed;
