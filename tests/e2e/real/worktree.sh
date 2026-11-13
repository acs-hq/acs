#!/usr/bin/env bash
# A real agent working in a linked worktree from a session started in the main checkout, which is
# what Claude Code's subagents do. Not part of CI: it spends real model calls.
#
#   tests/e2e/real/worktree.sh <empty-scratch-dir>
#
# Needs a build of `acs` in this checkout's target/ (the newer of release and debug) and a
# logged-in `claude`. Everything it writes is under the scratch directory: the repository, ACS_HOME
# and the logs. The plugin is loaded with --plugin-dir, so no settings file changes anywhere.
#
# The session starts in the main checkout. It is asked to edit one file in the worktree
# `.claude/worktrees/wp` and to commit there with `git -C <worktree> commit` (Claude Code itself
# asks for approval of `cd <dir> && git ...`, which a headless run cannot give; that form is
# covered by `crates/acs-cli/tests/git_hooks.rs`). It passes when the
# commit exists on the worktree's branch with the session's `ACS-Session` trailer, the pre-commit
# hook rejected nothing, the file was claimed as `src/app.py` (never under `.claude/worktrees/`),
# and the main checkout's copy is untouched.

set -u

checkout=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(cygpath -m "${1:?usage: worktree.sh <empty-scratch-dir>}" 2>/dev/null || echo "$1")
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

mkdir -p "$repo/src"
cd "$repo" || exit 2
git init -q -b main .
git config user.email pilot@example.com
git config user.name Pilot
printf 'def main():\n    return 0\n' > src/app.py
printf '/.claude/worktrees/\n' > .gitignore
git add -A
git commit -q -m init
acs init --yes > "$logs/init.txt" 2>&1 || { cat "$logs/init.txt"; exit 2; }
# The policy must be committed for the worktree's checkout to get the git hook checks.
git add -A
git commit -q -m "Adopt ACS" >> "$logs/init.txt" 2>&1
git worktree add -q -b wp .claude/worktrees/wp

id=$(node -e 'console.log(require("crypto").randomUUID())')
prompt="There is a git worktree at .claude/worktrees/wp. Use the Edit tool to add the line \
'# reviewed' at the end of .claude/worktrees/wp/src/app.py (do not touch src/app.py in this \
directory). Then run exactly these two shell commands, one after the other: \
git -C .claude/worktrees/wp add -A \
git -C .claude/worktrees/wp commit -m reviewed \
If ACS or a git hook blocks something, quote its message word for word and stop; do not try \
anything else to get around a block."
claude -p "$prompt" \
    --plugin-dir "$checkout/integrations/claude-code" \
    --model "$model" \
    --session-id "$id" \
    --permission-mode acceptEdits \
    --output-format stream-json --verbose \
    --max-budget-usd 1 \
    --allowedTools "Bash(git:*)" "mcp__plugin_acs_acs__*" \
    < /dev/null > "$logs/session.jsonl" 2> "$logs/session.err"

acs status --json > "$logs/status.json" 2>&1
acs log --json > "$logs/log.json" 2>&1
git -C .claude/worktrees/wp log -1 --format=%B > "$logs/commit.txt" 2>&1
# What the hooks decided, from Claude Code's own transcript of this session.
transcript=$(ls "$HOME"/.claude/projects/*/"$id".jsonl 2>/dev/null | head -1)
[ -n "$transcript" ] && cp "$transcript" "$logs/transcript.jsonl"

fail=0
check() {
    if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1"; fail=1; fi
}
check "the worktree's file was changed" "grep -q reviewed .claude/worktrees/wp/src/app.py"
check "the main checkout's file is untouched" "! grep -q reviewed src/app.py"
check "the commit is on the worktree's branch" "grep -q '^reviewed' '$logs/commit.txt'"
check "the commit carries the session's trailer" "grep -q '^ACS-Session: ses_' '$logs/commit.txt'"
check "the pre-commit hook rejected nothing" "! grep -q 'commit rejected' '$logs/session.jsonl'"
check "the file was claimed as src/app.py" \
    "grep -q 'src/app.py' '$logs/log.json' '$logs/status.json'"
check "nothing was claimed under .claude/worktrees/" \
    "! grep -q 'claude/worktrees' '$logs/log.json' '$logs/status.json'"
exit $fail
