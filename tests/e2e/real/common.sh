# Shared setup for the real-agent scripts first_run.sh, history.sh and guards.sh. Source it with
# the scratch directory as its argument:
#
#   . "$(dirname "$0")/common.sh" "${1:?usage: <script> <empty-scratch-dir>}"
#
# It picks the newer of this checkout's release and debug builds of `acs`, puts it first on PATH,
# points ACS_HOME into the scratch directory and defines the helpers below. Everything a script
# writes stays under the scratch directory. The plugin is loaded with --plugin-dir, so a session
# changes no settings file anywhere.

checkout=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
work=$(cygpath -m "$1" 2>/dev/null || echo "$1")
model=${ACS_REAL_MODEL:-sonnet}
budget=${ACS_REAL_BUDGET:-2}
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

fail=0
# check <name> <shell text>: the text is evaluated; true is a pass.
check() {
    if eval "$2" >> "$logs/checks.log" 2>&1; then echo "ok    $1"; else echo "FAIL  $1"; fail=1; fi
}
finish() {
    echo
    if [ "$fail" -eq 0 ]; then
        echo "all checks passed (logs in $logs)"
    else
        echo "some checks failed (logs in $logs)"
    fi
    exit "$fail"
}

# Options for `claude` that must come before the prompt; a script may fill this before `session`.
session_extra=()

# session <name> <prompt> <allowed tool>...: one headless Claude Code session in the current
# directory. Writes logs/<name>.id (the Claude Code session id), logs/<name>.jsonl (the stream)
# and logs/<name>.transcript.jsonl (Claude Code's own transcript, which holds the hook output).
session() {
    local name=$1 prompt=$2 id transcript
    shift 2
    id=$(node -e 'console.log(require("crypto").randomUUID())')
    echo "$id" > "$logs/$name.id"
    # --allowedTools takes every word after it, so it comes last.
    claude -p "$prompt" \
        ${session_extra[@]+"${session_extra[@]}"} \
        --plugin-dir "$checkout/integrations/claude-code" \
        --model "$model" \
        --session-id "$id" \
        --permission-mode acceptEdits \
        --output-format stream-json --verbose \
        --max-budget-usd "$budget" \
        --allowedTools "$@" \
        < /dev/null > "$logs/$name.jsonl" 2> "$logs/$name.err"
    transcript=$(ls "$HOME"/.claude/projects/*/"$id".jsonl 2>/dev/null | head -n 1)
    [ -n "$transcript" ] && cp "$transcript" "$logs/$name.transcript.jsonl"
}

# said <name> <text>: how many lines of the session's stream and transcript contain the text.
said() { cat "$logs/$1.transcript.jsonl" "$logs/$1.jsonl" 2>/dev/null | grep -c -- "$2"; }
# hooks_ran <name>: true when the transcript shows hook output, so ACS was in the loop.
hooks_ran() { grep -q '"type":"hook_success"' "$logs/$1.transcript.jsonl" 2>/dev/null; }

# ses_of <name>: the ACS session id (`ses_...`) of that session, from `acs log --json`. Empty when
# the session has no record. Records are written by `acs sync`, so it runs first.
ses_of() {
    acs sync > /dev/null 2>&1
    acs log --json --limit 200 > "$logs/log.json" 2>> "$logs/log.err"
    node -e '
        const fs = require("fs");
        const records = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
        const id = fs.readFileSync(process.argv[2], "utf8").trim();
        const hit = records.find((r) => r.seat && r.seat.harness_session_id === id);
        console.log(hit ? hit.session_id : "");
    ' "$logs/log.json" "$logs/$1.id" 2>> "$logs/log.err"
}
