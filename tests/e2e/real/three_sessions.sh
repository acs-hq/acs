#!/usr/bin/env bash
# The M1 gate with real agents (plan sections 8.3 and 9.1): three Claude Code sessions with
# overlapping tasks in one throwaway repository. Not part of CI: it spends real model calls.
#
#   tests/e2e/real/three_sessions.sh <empty-scratch-dir>
#
# Needs a release build of `acs` in this checkout's target/ and a logged-in `claude`. Everything it
# writes is under the scratch directory: the repository, ACS_HOME and the logs. The plugin is loaded
# with --plugin-dir, so no settings file changes anywhere.
#
# With ACS_REAL_NO_MCP=1 the ACS tools are not allowed. Agents that can read `acs_status` plan around
# each other, so few calls collide; without it they go straight to Edit and the hooks must stop them.
#
# Each round starts the sessions that still have work left, all at once, in one work tree. A session
# blocked on a shared file does the rest of its task and stops; the next round gives it another go.
# check.mjs then compares what Claude Code says happened with the files on disk.

set -u

checkout=$(cd "$(dirname "$0")/../../.." && pwd)
work=$(cygpath -m "${1:?usage: three_sessions.sh <empty-scratch-dir>}" 2>/dev/null || echo "$1")
model=${ACS_REAL_MODEL:-sonnet}
rounds=${ACS_REAL_ROUNDS:-3}
allowed=("Bash(git:*)" "Bash(python:*)")
[ "${ACS_REAL_NO_MCP:-}" = 1 ] || allowed+=("mcp__plugin_acs_acs__*")

[ -x "$checkout/target/release/acs" ] || [ -x "$checkout/target/release/acs.exe" ] || {
    echo "build first: cargo build --release -p acs-cli" >&2
    exit 2
}
mkdir -p "$work/logs"
export PATH="$checkout/target/release:$PATH"
export ACS_HOME="$work/acs-home"
repo="$work/repo"
logs="$work/logs"

mkdir -p "$repo/src" "$repo/tests"
cd "$repo" || exit 2
git init -q -b main .
git config user.email pilot@example.com
git config user.name Pilot
cat > src/pricing.py <<'EOF'
"""Prices are integer cents."""


def line_total(unit_cents, quantity):
    return unit_cents * quantity
EOF
cat > src/orders.py <<'EOF'
from pricing import line_total


def order_total(lines):
    return sum(line_total(cents, quantity) for cents, quantity in lines)
EOF
cat > src/reports.py <<'EOF'
def format_cents(cents):
    return f"{cents // 100}.{cents % 100:02d}"
EOF
cat > src/shipping.py <<'EOF'
def shipping_cents(weight_grams):
    return 499 if weight_grams < 2000 else 899
EOF
cat > CHANGELOG.md <<'EOF'
# Changelog

## Unreleased

EOF
printf '__pycache__/\n' > .gitignore
git add -A
git commit -q -m "Start the pricing project"
acs init > "$logs/init.txt" 2>&1 || { cat "$logs/init.txt"; exit 2; }
git add -A
git commit -q -m "Adopt ACS"

task() {
    case $1 in
    discount) cat <<'EOF'
Add a function `apply_discount(cents, percent)` to src/pricing.py that returns the discounted price in integer cents, rounding down. Use it from a new function `discounted_order_total(lines, percent)` in src/orders.py. Add one line about it under Unreleased in CHANGELOG.md, then commit the files you changed.
EOF
        ;;
    tax) cat <<'EOF'
Add a function `apply_tax(cents, rate_percent)` to src/pricing.py that returns the taxed price in integer cents, rounding up. Use it from a new function `format_with_tax(cents, rate_percent)` in src/reports.py. Add one line about it under Unreleased in CHANGELOG.md, then commit the files you changed.
EOF
        ;;
    rounding) cat <<'EOF'
Add a function `round_to_nearest(cents, step)` to src/pricing.py that rounds to the nearest multiple of step. Use it from a new function `rounded_shipping_cents(weight_grams)` in src/shipping.py. Add one line about it under Unreleased in CHANGELOG.md, then commit the files you changed.
EOF
        ;;
    esac
}

marker() {
    case $1 in
    discount) echo apply_discount ;;
    tax) echo apply_tax ;;
    rounding) echo round_to_nearest ;;
    esac
}

# Whether a task's work reached both shared files in a commit.
finished() {
    git show HEAD:src/pricing.py 2>/dev/null | grep -q "def $(marker "$1")" &&
        git show HEAD:CHANGELOG.md 2>/dev/null | grep -qi "${1%ing}"
}

run_session() {
    local name=$1 round=$2 id
    id=$(node -e 'console.log(require("crypto").randomUUID())')
    echo "$id" > "$logs/round$round-$name.session"
    claude -p "$(task "$name") If a file is blocked, do the rest of the task and say what is left." \
        --plugin-dir "$checkout/integrations/claude-code" \
        --model "$model" \
        --session-id "$id" \
        --permission-mode acceptEdits \
        --output-format stream-json --verbose \
        --max-budget-usd 2 \
        --allowedTools "${allowed[@]}" \
        < /dev/null > "$logs/round$round-$name.jsonl" 2> "$logs/round$round-$name.err"
}

pending=(discount tax rounding)
for round in $(seq 1 "$rounds"); do
    [ ${#pending[@]} -gt 0 ] || break
    echo "round $round: ${pending[*]}"
    pids=()
    for name in "${pending[@]}"; do
        run_session "$name" "$round" &
        pids+=($!)
    done
    # `acs status --json` while the sessions run, so collisions and live claims are on record.
    running=1
    while [ "$running" = 1 ]; do
        running=0
        for pid in "${pids[@]}"; do
            kill -0 "$pid" 2>/dev/null && running=1
        done
        acs status --json >> "$logs/status-during.jsonl" 2>/dev/null
        sleep 3
    done
    for pid in "${pids[@]}"; do wait "$pid"; done
    left=()
    for name in "${pending[@]}"; do
        finished "$name" || left+=("$name")
    done
    pending=(${left[@]+"${left[@]}"})
done

acs status > "$logs/status-after.txt" 2>&1
acs status --json > "$logs/status-after.json" 2>&1
git log --format='%h %s%n%(trailers:key=ACS-Session,valueonly)' > "$logs/git-log.txt"
git status --short > "$logs/git-status.txt"
[ ${#pending[@]} -eq 0 ] || echo "still pending after $rounds rounds: ${pending[*]}"
node "$checkout/tests/e2e/real/check.mjs" "$repo" "$logs"
