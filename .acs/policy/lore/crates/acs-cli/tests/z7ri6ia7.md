+++
id = "lore_z7ri6ia7"
scope = ["crates/acs-cli/tests/cli_plugin.rs", "crates/acs-cli/src/commands/plugin.rs"]
owner = "omarahmed3001@gmail.com"
written_against = "427a03b7bf9c"
+++

Tests of `acs plugin install` that need "no harness command line on PATH" use `path_without_harnesses()` in `cli_plugin.rs` (target dir + git's folder only); it works on Windows too, unlike `git_only_path`, which needs symlinks. Never let a test reach a user-scope write of Claude Code settings: `cli_common::run_with_path` sets `HOME`, but on Windows `std::env::home_dir()` reads `USERPROFILE`, so such a write would land in the developer's real `~/.claude/settings.json`. That is one reason user scope without `claude` stops instead of writing (decision 0052).
