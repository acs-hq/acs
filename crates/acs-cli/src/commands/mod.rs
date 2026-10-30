//! One module per command (decision 0011): `git_hook` (WP2.4), `init`, `status`, `claim`,
//! `release`, `off`, `doctor` (WP2.5) and `mcp` (WP2.6).
//!
//! This is the registration point: a work package adds one `pub mod` line here, one variant to
//! [`crate::cli::Command`] and one arm to [`crate::cli::run`]. Commands call the shared
//! operations in [`crate::runtime`] instead of reimplementing them.

pub mod claim;
pub mod doctor;
pub mod git_hook;
pub mod init;
pub mod mcp;
pub mod off;
pub mod release;
pub mod status;
mod support;
