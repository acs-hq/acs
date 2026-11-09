#!/usr/bin/env bash
# Phase 6 with a real agent: one Claude Code session in a throwaway repository with lore for
# payments/ and for docs/. Not part of CI: it spends real model calls.
#
#   tests/e2e/real/lore.sh <empty-scratch-dir>
#
# Needs a build of `acs` in this checkout's target/ (the newer of release and debug) and a
# logged-in `claude`. Everything it writes is under the scratch directory: the repository, ACS_HOME
# and the logs. The plugin is loaded with --plugin-dir, so no settings file changes anywhere.
#
# The agent is asked to add a function in payments/ and then to propose lore. It passes when the
# payments lore reached the agent, the docs lore (never touched) did not, the agent followed the
# lore (money variables end in _cents), a proposal waits in ACS_HOME, and nothing under .acs/
# changed in the repository.

set -u

checkout=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(cygpath -m "${1:?usage: lore.sh <empty-scratch-dir>}" 2>/dev/null || echo "$1")
model=${ACS_REAL_MODEL:-sonnet}
exe() { if [ -e "$1/acs.exe" ]; then echo "$1/acs.exe"; else echo "$1/acs"; fi; }
release=$(exe "$checkout/target/release")
debug=$(exe "$checkout/target/debug")
# The newer build wins, so a stale release binary is not tested by mistake.
if [ -x "$debug" ] && { [ ! -x "$release" ] || [ "$debug" -nt "$release" ]; }; then
    bin="$checkout/target/debug"
else
    bin="$checkout/target/release"
fi
[ -x "$(exe "$bin")" ] || {
    echo "build first: cargo build -p acs-cli" >&2
    exit 2
}
mkdir -p "$work/logs"
export PATH="$bin:$PATH"
export ACS_HOME="$work/acs-home"
repo="$work/repo"
logs="$work/logs"

mkdir -p "$repo/payments" "$repo/docs" "$repo/.acs/policy/lore/payments"
cd "$repo" || exit 2
git init -q -b main .
git config user.email pilot@example.com
git config user.name Pilot
printf 'def refund(amount):\n    return -amount\n' > payments/refund.py
printf '# Guide\n\nHow to use the payments module.\n' > docs/guide.md
git add -A
git commit -q -m init
base=$(git rev-parse --short=12 HEAD)
cat > .acs/policy/lore/payments/money.md <<EOF
+++
id = "lore_cents"
scope = ["payments/**"]
owner = "lina@example.com"
written_against = "$base"
+++
Money in payments/ is always an int number of cents, never a float or Decimal. Every parameter
or variable holding money ends in \`_cents\` (for example \`amount_cents\`), including in new code.
EOF
cat > .acs/policy/lore/docs.md <<EOF
+++
id = "lore_docs"
scope = ["docs/**"]
owner = "lina@example.com"
written_against = "$base"
+++
Docs are written in British English.
EOF
git add -A
git commit -q -m lore

id=$(node -e 'console.log(require("crypto").randomUUID())')
prompt="Add a function partial_refund to payments/refund.py that refunds a given percentage of a \
payment. Keep it small. When you are done, propose one piece of lore about payments/ for the \
next agent with your ACS tools (acs_lore_propose), then stop."
claude -p "$prompt" \
    --plugin-dir "$checkout/integrations/claude-code" \
    --model "$model" \
    --session-id "$id" \
    --permission-mode acceptEdits \
    --output-format stream-json --verbose \
    --max-budget-usd 1 \
    --allowedTools "Bash(git:*)" "mcp__plugin_acs_acs__*" \
    < /dev/null > "$logs/session.jsonl" 2> "$logs/session.err"

acs lore list --json > "$logs/lore-list.json" 2>&1
pending=$(ls "$ACS_HOME"/repos/*/pending/lore/*.json 2>/dev/null | head -n 1)
[ -n "$pending" ] && cp "$pending" "$logs/proposal.json"
# Hook context is not in the stream-json output; the session transcript has it as attachments.
transcript=$(ls "$HOME"/.claude/projects/*/"$id".jsonl 2>/dev/null | head -n 1)
[ -n "$transcript" ] && cp "$transcript" "$logs/transcript.jsonl"
hook_lore() { grep '"type":"hook_success"' "$logs/transcript.jsonl" 2>/dev/null | grep -c "$1"; }
fail=0
check() {
    if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fail=1; fi
}
check "the payments lore reached the agent exactly once" "[ \"\$(hook_lore lore_cents)\" = 1 ]"
check "the docs lore did not" "[ \"\$(hook_lore lore_docs)\" = 0 ]"
check "the agent followed the lore (partial_refund uses _cents)" \
    "grep -A6 'def partial_refund' payments/refund.py | grep -q '_cents'"
check "a proposal waits for review" "[ -n '$pending' ]"
check "nothing under .acs/ changed in the repository" \
    "[ -z \"\$(git status --porcelain -- .acs)\" ]"
exit $fail
