+++
id = "lore_geg7engp"
scope = ["crates/acs-git/**"]
owner = "omarahmed3001@gmail.com"
written_against = "89f24b9f1b5f"
+++

Tests in `acs-git` that call `commit_only` (or any production function that runs a plain `git commit`) need a repo-local identity: run `git config user.email` and `git config user.name` in the temp repo after `git init`. The test helper `git()` passes `-c user.name/email` only to its own commands, and CI runners have no global git identity, so such tests pass on a developer machine and fail in CI with "Author identity unknown". To reproduce locally, run the test with `GIT_CONFIG_GLOBAL=NUL` (or `/dev/null`) and `GIT_CONFIG_NOSYSTEM=1`.
