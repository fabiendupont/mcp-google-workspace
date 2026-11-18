#!/usr/bin/env bash
# E2E harness: drives a real Google Workspace account through navra + an LLM and
# judges the result with Claude Opus.
#
# Exit status is 0 only when every step passed (or was deliberately skipped with
# an explicit --skip-* flag). Every failed assertion is reported by `fail`, which
# prints the step, the exit code and the captured output, and the EXIT trap turns
# any failure into a non-zero exit with a summary. This script is meant to be
# load-bearing in CI: see tests/e2e/CI.md.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESULTS_DIR="$SCRIPT_DIR/results"
BINARY="$PROJECT_DIR/target/release/mcp-google-workspace"
MCP_PORT=3100
NAVRA_PORT=9315

# Logs live in TMPDIR (CI must set a writable TMPDIR; the default /tmp quota has
# caused false failures in this repo).
WORK_TMP="${TMPDIR:-/tmp}"
MCP_LOG="$WORK_TMP/mcp-e2e-server.log"
NAVRA_LOG="$WORK_TMP/navra-e2e-serve.log"
# Generated at startup from navra.toml + $MCPD_TOKEN so no token hash is ever
# committed. Kept outside the repo so it cannot be committed by accident.
NAVRA_CONFIG="$WORK_TMP/navra-e2e-generated.toml"

# Documented dummy token for local runs. It is not a secret: the committed
# navra.toml holds only the sha256 of this string.
DEFAULT_MCPD_TOKEN="e2e-test-token-not-a-secret"

ALL_MODELS="gemma4:e4b gemma4:26b qwen3:8b qwen3.6:35b-a3b claude-sonnet-4-5@20250929"
ALL_SCENARIOS="drive-workflow docs-workflow docs-full docs-tabs full-e2e sheets-advanced slides-workflow slides-template"

# --- Bookkeeping ----------------------------------------------------------
# PASS/FAIL/SKIP are only ever incremented from the branch that observed the
# corresponding outcome; TOTAL counts attempted (model, scenario) pairs.
PASS=0
FAIL=0
SKIP=0
TOTAL=0
FAILED=0
SUMMARY=()
MCP_PID=""
NAVRA_PID=""
HAVE_GWS=true
HAVE_OLLAMA=true

pass() {
    PASS=$((PASS + 1))
    SUMMARY+=("PASS  $1${2:+  ($2)}")
}

skip() {
    SKIP=$((SKIP + 1))
    SUMMARY+=("SKIP  $1${2:+  ($2)}")
    echo "SKIP: $1${2:+ ($2)}"
}

# note <step> [detail] — a successful or skipped non-scored step (build,
# server startup, deliberately skipped prerequisite). Recorded in the summary
# but kept out of the Total/Pass/Fail/Skipped counters so that
# Total == Pass + Fail + Skipped for the (model, scenario) matrix.
note() {
    SUMMARY+=("INFO  $1${2:+  ($2)}")
}

# skip_unscored <step> <flag> — a prerequisite the caller explicitly skipped.
skip_unscored() {
    SUMMARY+=("SKIP  $1  (--$2)")
    echo "SKIP: $1 (--$2)"
}

# fail <step> <message> [exit-code] [captured-output]
# Records a failed assertion. Never exits by itself: the EXIT trap converts any
# recorded failure into a non-zero exit status, so the whole matrix is reported
# before the harness gives up.
fail() {
    local step="$1" msg="$2" rc="${3:-1}" out="${4:-}"
    FAIL=$((FAIL + 1))
    FAILED=1
    SUMMARY+=("FAIL  $step  ($msg)")
    {
        echo "FAIL: $step"
        echo "  reason:    $msg"
        echo "  exit code: $rc"
        if [[ -n "$out" ]]; then
            echo "  output:"
            while IFS= read -r line; do
                echo "    | $line"
            done <<<"$out"
        fi
    } >&2
}

# die <step> <message> [exit-code] [captured-output] — fatal, stop immediately.
die() {
    local rc="${3:-1}"
    fail "$@"
    exit "$rc"
}

# --- Summary and cleanup --------------------------------------------------

cleanup() {
    if [[ -n "$NAVRA_PID" ]]; then
        kill "$NAVRA_PID" 2>/dev/null || true
        wait "$NAVRA_PID" 2>/dev/null || true
    fi
    if [[ -n "$MCP_PID" ]]; then
        kill "$MCP_PID" 2>/dev/null || true
        wait "$MCP_PID" 2>/dev/null || true
    fi
    rm -f "$NAVRA_CONFIG"
}

print_summary() {
    local line
    echo ""
    echo "=== Summary ==="
    if [[ ${#SUMMARY[@]} -gt 0 ]]; then
        for line in "${SUMMARY[@]}"; do
            echo "  $line"
        done
    fi
    echo "Total: $TOTAL  Pass: $PASS  Fail: $FAIL"
    echo "Skipped: $SKIP"
    echo "Results in: $RESULTS_DIR/"
}

on_exit() {
    local rc=$?
    echo "Stopping servers..."
    cleanup
    if [[ $rc -ne 0 && $FAIL -eq 0 ]]; then
        # An uncaught command (set -e) aborted the run before it could record a
        # failure; surface it in the summary instead of exiting silently.
        FAIL=$((FAIL + 1))
        FAILED=1
        SUMMARY+=("FAIL  uncaught error  (exit $rc)")
    fi
    print_summary
    if [[ $rc -eq 0 && $FAILED -ne 0 ]]; then
        rc=1
    fi
    exit "$rc"
}
trap on_exit EXIT
trap 'echo "ERROR: command failed (status $?): $BASH_COMMAND" >&2' ERR

# --- HTTP / TCP waiters ---------------------------------------------------

# wait_for_http <url> [tries] — succeeds only on a 2xx/3xx response, so a 404
# (e.g. a probe against a route the server does not serve) is a failure.
wait_for_http() {
    local url="$1" tries="${2:-15}" i
    for ((i = 1; i <= tries; i++)); do
        if curl -fsS --max-time 2 "$url" >/dev/null 2>&1; then
            return 0
        fi
        sleep 1
    done
    return 1
}

# wait_for_tcp <host> <port> [tries] — used for navra, whose HTTP surface is not
# part of our contract; all we need is that it is accepting connections.
wait_for_tcp() {
    local host="$1" port="$2" tries="${3:-15}" i
    for ((i = 1; i <= tries; i++)); do
        if (exec 3<>"/dev/tcp/$host/$port") 2>/dev/null; then
            return 0
        fi
        sleep 1
    done
    return 1
}

# --- Parse args -----------------------------------------------------------
MODELS=""
SCENARIOS=""
SKIP_BUILD=false
SKIP_OLLAMA=false
SKIP_GWS=false
SKIP_JUDGE=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --model|--scenario)
            if [[ $# -lt 2 ]]; then
                die "arguments" "$1 requires a value" 2 ""
            fi
            if [[ "$1" == "--model" ]]; then MODELS="$2"; else SCENARIOS="$2"; fi
            shift 2
            ;;
        --skip-build)  SKIP_BUILD=true; shift ;;
        --skip-ollama) SKIP_OLLAMA=true; shift ;;
        --skip-gws)    SKIP_GWS=true; shift ;;
        --skip-judge)  SKIP_JUDGE=true; shift ;;
        --help|-h)
            echo "Usage: $0 [--model MODEL] [--scenario SCENARIO] [--skip-build]"
            echo "          [--skip-ollama] [--skip-gws] [--skip-judge]"
            echo ""
            echo "Required env vars:"
            echo "  GWS_TEST_FOLDER_ID   Google Drive folder ID for test output"
            echo "  GWS_PROJECT_ID       Google Cloud project ID"
            echo ""
            echo "Optional env vars:"
            echo "  MCPD_TOKEN           navra authentication token (default: the"
            echo "                       documented dummy token, which the generated"
            echo "                       navra config hashes at startup)"
            echo "  TMPDIR               where harness logs are written"
            echo ""
            echo "Skip flags replace the old silent skips; a skipped step is"
            echo "reported as SKIP in the summary and does not fail the run."
            echo ""
            echo "Models: $ALL_MODELS"
            echo "Scenarios: $ALL_SCENARIOS"
            exit 0
            ;;
        *) echo "Unknown arg: $1"; exit 2 ;;
    esac
done

MODELS="${MODELS:-$ALL_MODELS}"
SCENARIOS="${SCENARIOS:-$ALL_SCENARIOS}"

# --- Check env vars -------------------------------------------------------
missing=()
for var in GWS_TEST_FOLDER_ID GWS_PROJECT_ID; do
    if [[ -z "${!var:-}" ]]; then
        missing+=("$var")
    fi
done
if [[ ${#missing[@]} -gt 0 ]]; then
    die "preflight: env" "required env var(s) not set: ${missing[*]}" 1 \
        "export GWS_TEST_FOLDER_ID=... GWS_PROJECT_ID=... (see --help)"
fi
MCPD_TOKEN="${MCPD_TOKEN:-$DEFAULT_MCPD_TOKEN}"

# --- Check prerequisites --------------------------------------------------
command -v curl >/dev/null 2>&1 \
    || die "prerequisite: curl" "curl not found on PATH" 1 ""
command -v python3 >/dev/null 2>&1 \
    || die "prerequisite: python3" "python3 not found on PATH" 1 ""
command -v sha256sum >/dev/null 2>&1 \
    || die "prerequisite: sha256sum" "sha256sum not found on PATH" 1 ""
# navra is the only way to drive a model and the judge, so there is no
# meaningful --skip flag for it: a missing navra is always fatal.
command -v navra >/dev/null 2>&1 \
    || die "prerequisite: navra" "navra not found on PATH (install navra; see tests/e2e/CI.md)" 1 ""

if ! command -v ollama >/dev/null 2>&1; then
    if [[ "$SKIP_OLLAMA" == "true" ]]; then
        HAVE_OLLAMA=false
        skip_unscored "prerequisite: ollama" "skip-ollama"
    else
        die "prerequisite: ollama" "ollama not found on PATH (pass --skip-ollama to skip local models)" 1 ""
    fi
fi

if ! command -v gws >/dev/null 2>&1; then
    if [[ "$SKIP_GWS" == "true" ]]; then
        HAVE_GWS=false
        skip_unscored "prerequisite: gws" "skip-gws"
    else
        die "prerequisite: gws" "gws CLI not found on PATH (pass --skip-gws to skip folder cleanup)" 1 ""
    fi
fi

if [[ "$SKIP_JUDGE" == "false" ]]; then
    if ! python3 -c 'import yaml' >/dev/null 2>&1; then
        die "prerequisite: PyYAML" "python3 module 'yaml' is not installed (pip install pyyaml, or pass --skip-judge)" 1 ""
    fi
fi

mkdir -p "$RESULTS_DIR"

# --- Build ----------------------------------------------------------------
if [[ "$SKIP_BUILD" == "false" ]]; then
    echo "Building release binary..."
    build_log="$WORK_TMP/mgw-e2e-build.log"
    if (cd "$PROJECT_DIR" && cargo build --release --locked) >"$build_log" 2>&1; then
        tail -1 "$build_log"
    else
        build_rc=$?
        die "build" "cargo build --release --locked failed" "$build_rc" "$(tail -20 "$build_log")"
    fi
fi
[[ -x "$BINARY" ]] || die "build" "binary not found at $BINARY" 1 ""
note "build" "$BINARY"

# --- Generate policy from template ---------------------------------------
POLICY="$SCRIPT_DIR/policy.generated.json"
[[ -f "$SCRIPT_DIR/policy.template.json" ]] \
    || die "policy" "missing $SCRIPT_DIR/policy.template.json" 1 ""
sed -e "s|\\\$GWS_TEST_FOLDER_ID|$GWS_TEST_FOLDER_ID|g" \
    -e "s|\\\$GWS_PROJECT_ID|$GWS_PROJECT_ID|g" \
    "$SCRIPT_DIR/policy.template.json" > "$POLICY" \
    || die "policy" "could not render policy template into $POLICY" "$?" ""
echo "Policy generated: $POLICY"

# --- Generate navra config (no committed token hash) ----------------------
render_navra_config() {
    local src="$1" dst="$2" token="$3" hash
    [[ -f "$src" ]] || die "navra config" "missing $src" 1 ""
    hash=$(printf '%s' "$token" | sha256sum | cut -d' ' -f1)
    sed "s|^token_hash = .*|token_hash = \"$hash\"|" "$src" >"$dst" \
        || die "navra config" "could not render $src into $dst" "$?" ""
}
render_navra_config "$SCRIPT_DIR/navra.toml" "$NAVRA_CONFIG" "$MCPD_TOKEN"

# --- Start MCP server -----------------------------------------------------
echo "Starting MCP server on port $MCP_PORT..."
"$BINARY" --policy "$POLICY" --prompts-dir "$PROJECT_DIR/prompts" --http "127.0.0.1:$MCP_PORT" --eager-tools >/dev/null 2>"$MCP_LOG" &
MCP_PID=$!

# The server serves /healthz (always 200 once listening), /livez and /readyz;
# there is no /health route, so probing /health would always 404.
if ! wait_for_http "http://127.0.0.1:$MCP_PORT/healthz" 20; then
    die "MCP server startup" "no response from /healthz on 127.0.0.1:$MCP_PORT" 1 \
        "$(tail -20 "$MCP_LOG" 2>/dev/null || true)"
fi
# /healthz only proves the socket is open; /readyz is 200 only once Discovery
# Docs are loaded, so gate on readiness before running any scenario.
if ! wait_for_http "http://127.0.0.1:$MCP_PORT/readyz" 60; then
    die "MCP server readiness" "/readyz did not become ready on 127.0.0.1:$MCP_PORT" 1 \
        "$(tail -20 "$MCP_LOG" 2>/dev/null || true)"
fi
echo "MCP server ready (PID $MCP_PID)"
note "MCP server startup" "/healthz + /readyz"

# --- Start navra ----------------------------------------------------------
echo "Starting navra on port $NAVRA_PORT..."
navra serve -c "$NAVRA_CONFIG" &>"$NAVRA_LOG" &
NAVRA_PID=$!
if ! wait_for_tcp "127.0.0.1" "$NAVRA_PORT" 15; then
    die "navra startup" "navra is not listening on 127.0.0.1:$NAVRA_PORT" 1 \
        "$(tail -20 "$NAVRA_LOG" 2>/dev/null || true)"
fi
echo "navra ready (PID $NAVRA_PID)"
note "navra startup" "127.0.0.1:$NAVRA_PORT"

# --- Run timestamp --------------------------------------------------------
RUN_ID=$(date -u +%Y-%m-%dT%H-%M-%S)
echo ""
echo "=== E2E Test Run: $RUN_ID ==="
echo "Models:    $MODELS"
echo "Scenarios: $SCENARIOS"
echo ""

# --- Map scenario names to prompt files -----------------------------------
prompt_file() {
    case "$1" in
        drive-workflow) echo "$PROJECT_DIR/prompts/test-drive-workflow.md" ;;
        docs-workflow)  echo "$PROJECT_DIR/prompts/test-docs-workflow.md" ;;
        docs-full)      echo "$PROJECT_DIR/prompts/test-docs-full.md" ;;
        docs-tabs)      echo "$PROJECT_DIR/prompts/test-docs-tabs.md" ;;
        full-e2e)       echo "$PROJECT_DIR/prompts/test-e2e.md" ;;
        sheets-advanced) echo "$PROJECT_DIR/prompts/test-sheets-advanced.md" ;;
        slides-workflow) echo "$PROJECT_DIR/prompts/test-slides-workflow.md" ;;
        slides-template) echo "$PROJECT_DIR/prompts/test-slides-template.md" ;;
        *) echo ""; return 1 ;;
    esac
}

model_slug() {
    echo "$1" | tr ':/@.' '-'
}

# Map scenarios to MCP prompts for context injection
mcp_prompt_for_scenario() {
    case "$1" in
        drive-workflow)  echo "" ;;
        docs-workflow)   echo "google-workspace:create-document" ;;
        docs-full)       echo "google-workspace:create-document" ;;
        docs-tabs)       echo "google-workspace:create-document" ;;
        full-e2e)        echo "google-workspace:create-document" ;;
        sheets-advanced) echo "google-workspace:work-with-spreadsheet" ;;
        slides-workflow) echo "" ;;
        slides-template) echo "google-workspace:create-presentation" ;;
        *) echo "" ;;
    esac
}

# --- Clean test folder ----------------------------------------------------
# Returns non-zero on an auth/API failure instead of swallowing it: a cleanup
# that silently does nothing makes the scenarios non-deterministic.
clean_test_folder() {
    local pattern did fid
    for pattern in "Project Alpha Deliverables" "Drive Test Output" "AI Infrastructure Report" \
        "Quarterly Review" "Sales Tracker" "Template Test" "Tabs Test Report"; do
        did=$(gws drive files list --params "{\"q\": \"name = '$pattern' and '$GWS_TEST_FOLDER_ID' in parents and trashed = false\", \"fields\": \"files(id)\"}" 2>/dev/null) || return 1
        fid=$(printf '%s' "$did" | python3 -c 'import sys,json; f=json.load(sys.stdin).get("files",[]); print(f[0]["id"] if f else "")') || return 1
        if [[ -n "$fid" ]]; then
            gws drive files update --params "{\"fileId\":\"$fid\"}" --json '{"trashed":true}' >/dev/null 2>&1 || return 1
        fi
    done
}

# --- Judge helpers --------------------------------------------------------
# Sums rubric weights. Takes the path as argv so no shell interpolation ever
# reaches the Python source.
rubric_max() {
    python3 - "$1" <<'PY'
import sys

import yaml

with open(sys.argv[1]) as f:
    doc = yaml.safe_load(f) or {}
print(sum(c.get("weight", 1.0) for c in doc.get("rubric", [])))
PY
}

# Minimal envsubst replacement ($VAR only) so the harness does not depend on
# gettext's envsubst being installed.
render_template() {
    python3 - "$1" <<'PY'
import os
import re
import sys

with open(sys.argv[1]) as f:
    text = f.read()
print(re.sub(r"\$([A-Za-z_][A-Za-z0-9_]*)", lambda m: os.environ.get(m.group(1), ""), text), end="")
PY
}

total_score() {
    printf '%s' "$1" | python3 -c 'import sys,json; d=json.loads(sys.stdin.read()); print("{}/{}".format(d.get("points", d.get("score", "?")), d.get("max", "?")))'
}

# --- Run tests ------------------------------------------------------------
for scenario in $SCENARIOS; do
    if ! pfile=$(prompt_file "$scenario"); then
        # A scenario named on the command line must exist; skipping it would
        # silently shrink the suite.
        die "scenario" "unknown scenario '$scenario' (see --help)" 2 ""
    fi
    if [[ -z "$pfile" || ! -f "$pfile" ]]; then
        die "scenario" "prompt file for '$scenario' not found: ${pfile:-<none>}" 1 ""
    fi

    for model in $MODELS; do
        TOTAL=$((TOTAL + 1))
        slug=$(model_slug "$model")
        step="model [$slug] $scenario"
        logfile="$RESULTS_DIR/${RUN_ID}-${slug}-${scenario}.log"

        echo "--- [$slug] $scenario ---"

        if [[ "$HAVE_GWS" == "true" ]]; then
            if clean_out=$(clean_test_folder 2>&1); then
                :
            else
                clean_rc=$?
                fail "$step" "could not clean the test folder (gws CLI error)" "$clean_rc" "$clean_out"
                continue
            fi
        fi

        # Mark log position before this run
        log_offset=0
        if [[ -f "$MCP_LOG" ]]; then
            log_offset=$(wc -c <"$MCP_LOG")
        fi

        # Preload local model
        if [[ "$model" != *"claude"* && "$model" != *"vertex"* && "$HAVE_OLLAMA" == "true" ]]; then
            if ! curl -fsS --max-time 120 http://localhost:11434/api/generate \
                -d "{\"model\":\"$model\",\"prompt\":\"hi\",\"stream\":false}" >/dev/null 2>&1; then
                echo "  WARN: could not preload $model; letting navra pull it"
            fi
            sleep 2
        fi

        # Run the test model (with MCP prompt injection for large-context models only)
        mcp_prompt=$(mcp_prompt_for_scenario "$scenario")
        prompt_flag=""
        if [[ -n "$mcp_prompt" ]]; then
            case "$model" in
                *claude*|*opus*|*sonnet*|qwen3.6*) prompt_flag="--upstream-prompt $mcp_prompt" ;;
                *) ;; # skip for small-context models (gemma4:e4b, qwen3:8b, gemma4:26b)
            esac
        fi
        if MCPD_TOKEN="$MCPD_TOKEN" navra run -e "http://127.0.0.1:$NAVRA_PORT/mcp" -m "$model" -n 100 $prompt_flag "$(cat "$pfile")" >"$logfile" 2>&1; then
            errors=$(tail -c +$((log_offset + 1)) "$MCP_LOG" 2>/dev/null | grep -c "Tool error\|Tool failed" || true)
            errors=${errors:-0}
            iterations=$(grep -o "Iterations: [0-9]*" "$logfile" | grep -o "[0-9]*" || true)
            time_s=$(grep -o "Time:.*s" "$logfile" | grep -o "[0-9.]*" || true)
            echo "  Model done: ${iterations:-?} iterations, ${time_s:-?}s, ${errors} tool errors"
        else
            model_rc=$?
            echo "  Model FAILED (navra exit code $model_rc)"
            fail "$step" "navra run exited $model_rc" "$model_rc" "$(tail -20 "$logfile" 2>/dev/null || true)"
            continue
        fi

        # Extract tool calls from MCP server log for the judge (only this run's portion)
        tool_calls_file="$RESULTS_DIR/${RUN_ID}-${slug}-${scenario}.tools.log"
        tail -c +$((log_offset + 1)) "$MCP_LOG" 2>/dev/null \
            | sed 's/\x1b\[[0-9;]*m//g' \
            | grep "Tool call\|Tool ok\|Tool fail" \
            >"$tool_calls_file" 2>/dev/null || true
        [[ -f "$tool_calls_file" ]] || : >"$tool_calls_file"

        # Judge with Claude Opus via Vertex AI
        if [[ "$SKIP_JUDGE" == "true" ]]; then
            echo "  SKIP: judge (--skip-judge)"
            skip "judge [$slug] $scenario" "--skip-judge"
            continue
        fi

        judgefile="$RESULTS_DIR/${RUN_ID}-${slug}-${scenario}.judge.log"
        jsonlfile="$RESULTS_DIR/${RUN_ID}-${slug}-${scenario}.jsonl"
        rubric_file="$SCRIPT_DIR/scenarios/${scenario}.yaml"
        judge_template="$SCRIPT_DIR/judge.md"

        # A missing rubric/template used to be a silent SKIP that still counted
        # as a PASS; it is now a hard failure.
        if [[ ! -f "$rubric_file" || ! -f "$judge_template" ]]; then
            fail "judge [$slug] $scenario" "missing rubric ($rubric_file) or judge template ($judge_template)" 1 ""
            continue
        fi

        echo "  Judging..."
        if max_score=$(rubric_max "$rubric_file" 2>&1); then
            :
        else
            rubric_rc=$?
            fail "judge [$slug] $scenario" "could not read rubric weights" "$rubric_rc" "$max_score"
            continue
        fi
        export RUN_ID MODEL="$model" SCENARIO="$scenario" GWS_TEST_FOLDER_ID
        PROMPT_CONTENT="$(cat "$pfile")"
        RUBRIC_CONTENT="$(cat "$rubric_file")"
        TOOL_CALLS="$(cat "$tool_calls_file")"
        MAX_SCORE="$max_score"
        export PROMPT_CONTENT RUBRIC_CONTENT TOOL_CALLS MAX_SCORE
        if judge_prompt=$(render_template "$judge_template" 2>&1); then
            :
        else
            render_rc=$?
            fail "judge [$slug] $scenario" "could not render $judge_template" "$render_rc" "$judge_prompt"
            continue
        fi

        if MCPD_TOKEN="$MCPD_TOKEN" navra run -e "http://127.0.0.1:$NAVRA_PORT/mcp" -m claude-opus-4-6@default -n 50 "$judge_prompt" >"$judgefile" 2>&1; then
            judge_rc=0
        else
            judge_rc=$?
        fi
        if [[ $judge_rc -ne 0 ]]; then
            echo "  Judge FAILED (navra exit code $judge_rc)"
            fail "judge [$slug] $scenario" "judge navra run exited $judge_rc" "$judge_rc" "$(tail -20 "$judgefile" 2>/dev/null || true)"
            continue
        fi

        grep '^{' "$judgefile" > "$jsonlfile" 2>/dev/null || true
        total_line=$(grep '"_total"' "$jsonlfile" 2>/dev/null || true)
        if [[ -z "$total_line" ]]; then
            # No _total line means the run cannot be scored; that is a failed
            # assertion, not a pass.
            echo "  Judge score: no total"
            fail "judge [$slug] $scenario" "judge output contained no _total line" 0 "$(tail -20 "$judgefile" 2>/dev/null || true)"
            continue
        fi
        if score_str=$(total_score "$total_line" 2>&1); then
            :
        else
            score_rc=$?
            echo "  Judge score: unparseable"
            fail "judge [$slug] $scenario" "could not parse the _total line" "$score_rc" "$total_line"
            continue
        fi
        if [[ ! "$score_str" =~ ^[0-9.]+/[0-9.]+$ ]]; then
            echo "  Judge score: unparseable ($score_str)"
            fail "judge [$slug] $scenario" "incomplete _total line (missing points/max)" 0 "$total_line"
            continue
        fi

        echo "  Judge score: $score_str"
        pass "judge [$slug] $scenario" "$score_str"
    done
done
