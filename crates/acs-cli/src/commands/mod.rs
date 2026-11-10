//! One module per command (decision 0011): `git_hook` (WP2.4), `init`, `status`, `claim`,
//! `release`, `off`, `doctor` (WP2.5), `mcp` (WP2.6), `sync` (WP3.1), `plugin` (WP5.2),
//! `override` (WP7.1), `watch` (WP7.4), `lore` (WP6.1, WP6.2), `log` (WP4.1), `blame` (WP4.2),
//! `undo` (WP4.3), `verify` (WP4.4) and `stats` (WP4.5).
//!
//! This is the registration point: a work package adds one `pub mod` line here, one variant to
//! [`crate::cli::Command`] and one arm to [`crate::cli::run`]. Commands call the shared
//! operations in [`crate::runtime`] instead of reimplementing them.

pub mod blame;
pub mod claim;
pub mod doctor;
pub mod git_hook;
pub mod init;
pub mod log;
pub mod lore;
pub mod mcp;
pub mod off;
pub mod r#override;
pub mod plugin;
pub mod release;
pub mod stats;
pub mod status;
mod support;
pub mod sync;
pub mod undo;
pub mod verify;
pub mod watch;
