+++
id = "lore_rvddevlc"
scope = ["crates/acs-cli/tests/**", "tests/e2e/**"]
owner = "omarahmed3001@gmail.com"
written_against = "c974ee4b17af"
+++

The CLI's human-readable output is word-wrapped at a fixed width (`support::wrap`), so where a line breaks depends on how long the temp path in front of it is. That length differs per OS (`/tmp/...` on Linux, `/var/folders/...` on macOS, `AppData\Local\Temp\...` on Windows). An assertion like `text.contains("some phrase")` can pass on Windows and fail only on Linux CI. Assert on the command's `--json` output (for example the file's `detail` or `action` in `acs undo --json`) instead of phrases in the wrapped text.
