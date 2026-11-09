# Phase 6 (lore): real-agent runs

`tests/e2e/real/lore.sh`, 2026-11-09, Windows 11, Claude Code 2.1.316 (`claude -p`, Sonnet), the
debug build of the `phase6-lore` branch, a throwaway repository and `ACS_HOME`, the plugin loaded
with `--plugin-dir`. The repository had two entries: `lore_cents` for `payments/**` ("money is
int cents; money variables end in `_cents`") and `lore_docs` for `docs/**`. The agent was asked to
add `partial_refund` to `payments/refund.py` and then to propose lore.

| Run | Result |
| --- | --- |
| 1 | The agent read the file, got `lore_cents` with that `Read` (transcript: `PreToolUse:Read` hook context, rendered as a system reminder), claimed, edited, proposed, released. `partial_refund(amount_cents, percent)`. A proposal waited in `ACS_HOME`; `.acs/` unchanged; `lore_docs` never shown. The script reported a miss on "lore reached the agent" only because it grepped the stream-json output, which does not contain hook context; fixed to read the transcript. |
| 2 | All five checks passed: lore exactly once, no docs lore, `_cents` naming followed, proposal waiting, `.acs/` unchanged. The proposal itself said the old `refund(amount)` "predates the `_cents` naming convention". |

Tool calls in run 1: `Read`, `acs_claim`, `Edit`, `acs_lore_propose`, `acs_release` (plus
`ToolSearch` and the usage skill). The claim's reply carried no lore: it had been shown already.

Not covered by a real run: `acs lore review` (agents may not run it; covered by
`crates/acs-cli/tests/lore.rs`), staleness markers and the cap (covered by the same tests).
