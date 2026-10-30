//! The `acs` command line and the code behind it.
//!
//! - [`runtime`]: opening a repository context (work tree, repo id, state, policy, snapshots),
//!   resolving seats, and the shared operations the hook, the CLI and the MCP server all call
//!   (`status`, `claim_paths`, `release_paths`, `set_intent`, `last_decision`).
//! - [`hook`]: the pre-tool, post-tool and session pipelines behind `acs hook`.
//! - [`cli`]: the clap command tree and dispatch; `main.rs` only calls into it.
//! - [`commands`]: one module per command, added by later work packages.

pub mod cli;
pub mod commands;
pub mod hook;
pub mod runtime;
