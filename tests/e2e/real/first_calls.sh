#!/usr/bin/env bash
# Do agents make ACS's two first calls without being asked (issue 73)? Real sessions in a
# throwaway repository, with tasks that never mention ACS. Not part of CI: it spends real model
# calls.
#
#   tests/e2e/real/first_calls.sh <empty-scratch-dir>
#
# Needs a build of `acs` in this checkout's target/ (the newer of release and debug), a logged-in
# `claude` and, for the Antigravity part, a logged-in `agy` (ACS_REAL_SKIP_AGY=1 or
# ACS_REAL_SKIP_CLAUDE=1 leaves one part out). Everything it writes is under the scratch
# directory. Claude Code loads the plugin with --plugin-dir; Antigravity gets it from
# `acs plugin install antigravity` in the scratch repository.
#
# Claude Code: three separate sessions (an edit, a question that needs no edit, a new file). Each
# must call acs_status and acs_intent, before its first change to a file, and the intent must be
# in ACS's own state afterwards.
#
# Antigravity: one conversation of three messages. The first message must bring acs_status and
# acs_intent before the first change, and the later messages must not bring acs_status again
# (the human saw it called with every message).
#
# ACS_REAL_MODEL picks Claude Code's model (default sonnet), ACS_REAL_AGY_MODEL Antigravity's
# (default gemini-3.8-flash-medium).

set -u
. "$(dirname "$0")/common.sh" "${1:?usage: first_calls.sh <empty-scratch-dir>}"
agy_model=${ACS_REAL_AGY_MODEL:-gemini-3.8-flash-medium}

export GIT_AUTHOR_NAME=Pilot GIT_AUTHOR_EMAIL=pilot@example.com
export GIT_COMMITTER_NAME=Pilot GIT_COMMITTER_EMAIL=pilot@example.com

# new_repo <dir>: a small project with ACS adopted.
new_repo() {
    git init -q -b main "$1"
    (
        cd "$1" || exit 2
        git config user.email pilot@example.com
        git config user.name Pilot
        printf 'def add(a, b):\n    return a + b\n\n\ndef sub(a, b):\n    return a - b\n' > calc.py
        printf '# calc\n\nA tiny calculator module.\n' > README.md
        git add -A
        git commit -q -m first
        acs init --yes > "$logs/init-$(basename "$1").txt" 2>&1 || exit 2
        git add -A
        git commit -q -m "Adopt ACS"
    ) || { echo "could not set up $1" >&2; exit 2; }
}

# calls <harness> <file>: the tool calls of a session in order, one name per line, with ACS's
# tools and the file-changing tools reduced to: status, intent, release, change.
calls() {
    node -e '
        const fs = require("fs");
        const [harness, file] = process.argv.slice(1);
        const short = (name) => {
            if (/acs_status$/.test(name)) return "status";
            if (/acs_intent$/.test(name)) return "intent";
            if (/acs_release$/.test(name)) return "release";
            if (/^(Edit|Write|MultiEdit|NotebookEdit|Bash|PowerShell)$/.test(name)) return "change";
            if (/^(edit_file|write_file|create_file|replace_file_content|multi_replace_file_content|write_to_file|run_command|run_shell_command)$/.test(name)) return "change";
            return "other:" + name;
        };
        let text = "";
        try { text = fs.readFileSync(file, "utf8"); } catch {}
        for (const line of text.split("\n")) {
            let e;
            try { e = JSON.parse(line); } catch { continue; }
            if (harness === "claude") {
                for (const block of e.message?.content ?? []) {
                    if (e.type === "assistant" && block.type === "tool_use") console.log(short(block.name));
                }
            } else {
                if (e.type === "USER_INPUT") console.log("message");
                for (const call of e.tool_calls ?? []) console.log(short(call.name));
            }
        }
    ' "$1" "$2"
}
# before_change <calls-file> <name>: true when <name> was called, and before the first change.
before_change() {
    awk -v want="$2" '
        $0 == want { seen = 1 }
        $0 == "change" && !seen { exit 1 }
        END { exit seen ? 0 : 1 }
    ' "$1"
}
count() { grep -c -x -- "$2" "$1"; }
# intent_of <harness-session-id>: the intent ACS holds for that session, from its state.
intent_of() {
    acs status --json > "$logs/status-last.json" 2>&1
    acs sync > /dev/null 2>&1
    acs log --json --limit 200 > "$logs/log.json" 2>> "$logs/log.err"
    node -e '
        const fs = require("fs");
        const records = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
        const hit = records.find((r) => r.seat && r.seat.harness_session_id === process.argv[2]);
        console.log(hit && hit.intent ? hit.intent : "");
    ' "$logs/log.json" "$1" 2>> "$logs/log.err"
}

# ---------------------------------------------------------------------------------------------
# Claude Code
# ---------------------------------------------------------------------------------------------
if [ -z "${ACS_REAL_SKIP_CLAUDE:-}" ]; then
    new_repo "$repo"
    cd "$repo" || exit 2
    echo "Claude Code $(claude --version 2> /dev/null), model $model"

    claude_session() { # <name> <prompt>
        session "$1" "$2" "Bash(git:*)" "Bash(python:*)" "mcp__plugin_acs_acs__*"
        calls claude "$logs/$1.jsonl" > "$logs/$1.calls"
        echo "  $1: $(tr '\n' ' ' < "$logs/$1.calls")"
        check "$1: the hooks ran" "hooks_ran $1"
        check "$1: acs_status was called" "[ \"\$(count '$logs/$1.calls' status)\" -ge 1 ]"
        check "$1: acs_intent was called" "[ \"\$(count '$logs/$1.calls' intent)\" -ge 1 ]"
        check "$1: acs_status came before the first change" "before_change '$logs/$1.calls' status"
        check "$1: acs_intent came before the first change" "before_change '$logs/$1.calls' intent"
        check "$1: acs_status was called once" "[ \"\$(count '$logs/$1.calls' status)\" -eq 1 ]"
        intent=$(intent_of "$(cat "$logs/$1.id")")
        echo "  $1: intent on record: ${intent:-(none)}"
        check "$1: the session's record carries the intent" "[ -n \"$intent\" ]"
        # Reported, not required: claims end with the session anyway.
        echo "  $1: acs_release calls: $(count "$logs/$1.calls" release)"
    }

    claude_session edit "Add a function multiply(a, b) to calc.py that returns a * b."
    check "edit: the function is there" "grep -q 'def multiply' calc.py"
    claude_session question "How many functions does calc.py define, and what are their names? Answer in one line."
    claude_session newfile "Create test_calc.py with one plain assert-based test for each function in calc.py."
    check "newfile: the file is there" "[ -f test_calc.py ]"
fi

# ---------------------------------------------------------------------------------------------
# Antigravity
# ---------------------------------------------------------------------------------------------
if [ -z "${ACS_REAL_SKIP_AGY:-}" ]; then
    command -v agy > /dev/null || { echo "agy is not on PATH (ACS_REAL_SKIP_AGY=1 skips it)" >&2; exit 2; }
    agy_repo="$work/agy-repo"
    new_repo "$agy_repo"
    cd "$agy_repo" || exit 2
    acs plugin install antigravity > "$logs/agy-plugin.txt" 2>&1 || { cat "$logs/agy-plugin.txt"; exit 2; }
    git add -A
    git commit -q -m "ACS plugin for Antigravity"
    echo "agy $(agy --version 2> /dev/null), model $agy_model"
    printf '{"toolPermission":"always-proceed"}\n' > "$work/agy-settings.json"

    conversation=""
    agy_message() { # <n> <prompt>
        local extra=()
        [ -n "$conversation" ] && extra=(--conversation "$conversation")
        MSYS_NO_PATHCONV=1 agy -p "$2" \
            ${extra[@]+"${extra[@]}"} \
            --model "$agy_model" \
            --settings "$work/agy-settings.json" \
            --output-format stream-json \
            --print-timeout 600s \
            < /dev/null > "$logs/agy-$1.ndjson" 2> "$logs/agy-$1.err"
        if [ -z "$conversation" ]; then
            conversation=$(node -e '
                const lines = require("fs").readFileSync(process.argv[1], "utf8").split("\n");
                for (const line of lines) {
                  try { const e = JSON.parse(line); if (e.event === "init") { console.log(e.conversation_id); break; } } catch {}
                }' "$logs/agy-$1.ndjson")
        fi
    }
    agy_message 1 "Add a function multiply(a, b) to calc.py that returns a * b."
    agy_message 2 "Now add a function divide(a, b) to calc.py that returns a / b."
    agy_message 3 "Mention both new functions in one sentence at the end of README.md."

    transcript="$logs/agy-transcript.ndjson"
    cp "$HOME/.gemini/antigravity-cli/brain/$conversation/.system_generated/logs/transcript_full.jsonl" \
        "$transcript" 2> /dev/null || : > "$transcript"
    calls agy "$transcript" > "$logs/agy.calls"
    echo "  agy: $(tr '\n' ' ' < "$logs/agy.calls")"
    # The part of the conversation that answers the first message.
    awk '$0 == "message" { n++ } n == 1' "$logs/agy.calls" > "$logs/agy-first.calls"
    awk '$0 == "message" { n++ } n >= 2' "$logs/agy.calls" > "$logs/agy-later.calls"

    check "agy: the conversation has three messages" "[ \"\$(count '$logs/agy.calls' message)\" -eq 3 ]"
    check "agy: the work was done" \
        "grep -q 'def multiply' calc.py && grep -q 'def divide' calc.py && grep -q multiply README.md"
    check "agy: ACS's start message reached the agent" "grep -q 'make these two tool calls' '$transcript'"
    check "agy: acs_status came before the first change" "before_change '$logs/agy-first.calls' status"
    check "agy: acs_intent came before the first change" "before_change '$logs/agy-first.calls' intent"
    check "agy: acs_status was not called again with the later messages" \
        "[ \"\$(count '$logs/agy-later.calls' status)\" -eq 0 ]"
    check "agy: acs_status was called once in the conversation" "[ \"\$(count '$logs/agy.calls' status)\" -eq 1 ]"
    # An Antigravity conversation has no end event, so its record is not written yet: ACS's own
    # answer to the call is the evidence.
    check "agy: ACS recorded the intent" "grep -q 'Intent recorded:' '$transcript'"
    check "agy: ACS's start message came once"         "[ \"\$(grep -c 'make these two tool calls' '$transcript')\" -eq 1 ]"
    echo "  agy: acs_intent calls: $(count "$logs/agy.calls" intent), acs_release calls: $(count "$logs/agy.calls" release)"
fi
finish
