#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
RESULTS_DIR="$SCRIPT_DIR/results"
BINARY="$PROJECT_DIR/target/release/mcp-google-workspace"
MCP_PORT=3100
NAVRA_PORT=9320

ALL_MODELS="gemma4:e4b gemma4:26b qwen3:8b qwen3.6:35b-a3b claude-sonnet-4-5@20250929"
ALL_SCENARIOS="drive-workflow docs-workflow docs-full full-e2e sheets-advanced slides-workflow slides-template gmail-workflow"

# --- Parse args ---
MODELS=""
SCENARIOS=""
SKIP_BUILD=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --model)     MODELS="$2"; shift 2 ;;
        --scenario)  SCENARIOS="$2"; shift 2 ;;
        --skip-build) SKIP_BUILD=true; shift ;;
        --help|-h)
            echo "Usage: $0 [--model MODEL] [--scenario SCENARIO] [--skip-build]"
            echo ""
            echo "Required env vars:"
            echo "  GWS_TEST_FOLDER_ID   Google Drive folder ID for test output"
            echo "  GWS_PROJECT_ID       Google Cloud project ID"
            echo "  MCPD_TOKEN           navra authentication token"
            echo ""
            echo "Models: $ALL_MODELS"
            echo "Scenarios: $ALL_SCENARIOS"
            exit 0
            ;;
        *) echo "Unknown arg: $1"; exit 1 ;;
    esac
done

MODELS="${MODELS:-$ALL_MODELS}"
SCENARIOS="${SCENARIOS:-$ALL_SCENARIOS}"

# --- Check env vars ---
fail=0
for var in GWS_TEST_FOLDER_ID GWS_PROJECT_ID MCPD_TOKEN; do
    if [[ -z "${!var:-}" ]]; then
        echo "ERROR: $var is not set"
        fail=1
    fi
done
[[ $fail -eq 1 ]] && exit 1

# --- Check prerequisites ---
command -v navra >/dev/null 2>&1 || { echo "ERROR: navra not found"; exit 1; }
command -v ollama >/dev/null 2>&1 || { echo "ERROR: ollama not found"; exit 1; }
command -v gws >/dev/null 2>&1 || { echo "ERROR: gws CLI not found"; exit 1; }

# --- Build ---
if [[ "$SKIP_BUILD" == "false" ]]; then
    echo "Building release binary..."
    (cd "$PROJECT_DIR" && cargo build --release 2>&1 | tail -1)
fi
[[ -x "$BINARY" ]] || { echo "ERROR: Binary not found at $BINARY"; exit 1; }

# --- Generate policy from template ---
POLICY="$SCRIPT_DIR/policy.generated.json"
sed -e "s|\\\$GWS_TEST_FOLDER_ID|$GWS_TEST_FOLDER_ID|g" \
    -e "s|\\\$GWS_PROJECT_ID|$GWS_PROJECT_ID|g" \
    "$SCRIPT_DIR/policy.template.json" > "$POLICY"
echo "Policy generated: $POLICY"

# --- Start MCP server ---
echo "Starting MCP server on port $MCP_PORT..."
"$BINARY" --policy "$POLICY" --prompts-dir "$PROJECT_DIR/prompts" --http "127.0.0.1:$MCP_PORT" --eager-tools >/dev/null 2>/tmp/mcp-e2e-server.log &
MCP_PID=$!
sleep 4
if ! curl -s "http://127.0.0.1:$MCP_PORT/health" >/dev/null 2>&1; then
    echo "ERROR: MCP server failed to start"
    tail -5 /tmp/mcp-e2e-server.log
    exit 1
fi
echo "MCP server ready (PID $MCP_PID)"

# --- Start navra ---
echo "Starting navra on port $NAVRA_PORT..."
navra serve -c "$SCRIPT_DIR/navra-9320.toml" &>/tmp/navra-e2e-serve.log &
NAVRA_PID=$!
for i in $(seq 1 15); do
    sleep 1
    curl -s "http://127.0.0.1:$NAVRA_PORT/health" >/dev/null 2>&1 && break
done
if ! curl -s "http://127.0.0.1:$NAVRA_PORT/health" >/dev/null 2>&1; then
    echo "ERROR: navra failed to start"
    tail -5 /tmp/navra-e2e-serve.log
    kill $MCP_PID 2>/dev/null
    exit 1
fi
echo "navra ready (PID $NAVRA_PID)"

# --- Cleanup function ---
cleanup() {
    echo "Stopping servers..."
    kill $NAVRA_PID 2>/dev/null || true
    kill $MCP_PID 2>/dev/null || true
    wait $NAVRA_PID 2>/dev/null || true
    wait $MCP_PID 2>/dev/null || true
}
trap cleanup EXIT

# --- Run timestamp ---
RUN_ID=$(date -u +%Y-%m-%dT%H-%M-%S)
echo ""
echo "=== E2E Test Run: $RUN_ID ==="
echo "Models:    $MODELS"
echo "Scenarios: $SCENARIOS"
echo ""

# --- Map scenario names to prompt files ---
prompt_file() {
    case "$1" in
        drive-workflow) echo "$PROJECT_DIR/prompts/test-drive-workflow.md" ;;
        docs-workflow)  echo "$PROJECT_DIR/prompts/test-docs-workflow.md" ;;
        docs-full)      echo "$PROJECT_DIR/prompts/test-docs-full.md" ;;
        full-e2e)       echo "$PROJECT_DIR/prompts/test-e2e.md" ;;
        sheets-advanced) echo "$PROJECT_DIR/prompts/test-sheets-advanced.md" ;;
        slides-workflow) echo "$PROJECT_DIR/prompts/test-slides-workflow.md" ;;
        slides-template) echo "$PROJECT_DIR/prompts/test-slides-template.md" ;;
        gmail-workflow)  echo "$PROJECT_DIR/prompts/test-gmail-workflow.md" ;;
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
        full-e2e)        echo "google-workspace:create-document" ;;
        sheets-advanced) echo "google-workspace:work-with-spreadsheet" ;;
        slides-workflow) echo "" ;;
        slides-template) echo "google-workspace:create-presentation" ;;
        gmail-workflow)  echo "google-workspace:work-with-email" ;;
        *) echo "" ;;
    esac
}

# --- Clean test folder ---
clean_test_folder() {
    local did
    did=$(gws drive files list --params "{\"q\": \"name = 'Project Alpha Deliverables' and '$GWS_TEST_FOLDER_ID' in parents and trashed = false\", \"fields\": \"files(id)\"}" 2>/dev/null \
        | python3 -c "import sys,json; f=json.loads(sys.stdin.read())['files']; print(f[0]['id'] if f else '')" 2>/dev/null || true)
    if [[ -n "$did" ]]; then
        gws drive files update --params "{\"fileId\":\"$did\"}" --json '{"trashed":true}' >/dev/null 2>&1 || true
    fi
    # Also clean stray test files
    for pattern in "Drive Test Output" "AI Infrastructure Report" "Quarterly Review" "Sales Tracker" "Template Test"; do
        local fid
        fid=$(gws drive files list --params "{\"q\": \"name = '$pattern' and '$GWS_TEST_FOLDER_ID' in parents and trashed = false\", \"fields\": \"files(id)\"}" 2>/dev/null \
            | python3 -c "import sys,json; f=json.loads(sys.stdin.read())['files']; print(f[0]['id'] if f else '')" 2>/dev/null || true)
        if [[ -n "$fid" ]]; then
            gws drive files update --params "{\"fileId\":\"$fid\"}" --json '{"trashed":true}' >/dev/null 2>&1 || true
        fi
    done
}

# --- Run tests ---
PASS=0
FAIL=0
TOTAL=0

for scenario in $SCENARIOS; do
    pfile=$(prompt_file "$scenario")
    if [[ -z "$pfile" || ! -f "$pfile" ]]; then
        echo "SKIP: Unknown scenario '$scenario'"
        continue
    fi

    for model in $MODELS; do
        TOTAL=$((TOTAL + 1))
        slug=$(model_slug "$model")
        logfile="$RESULTS_DIR/${RUN_ID}-${slug}-${scenario}.log"

        echo "--- [$slug] $scenario ---"
        clean_test_folder

        # Mark log position before this run
        log_offset=$(wc -c < /tmp/mcp-e2e-server.log 2>/dev/null || echo 0)

        # Preload local model
        if [[ "$model" != *"claude"* && "$model" != *"vertex"* ]]; then
            curl -s http://localhost:11434/api/generate -d "{\"model\":\"$model\",\"prompt\":\"hi\",\"stream\":false}" >/dev/null 2>&1 || true
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
            errors=$(tail -c +$((log_offset + 1)) /tmp/mcp-e2e-server.log 2>/dev/null | grep -c "Tool error\|Tool failed" || echo "0")
            iterations=$(grep -o "Iterations: [0-9]*" "$logfile" | grep -o "[0-9]*" || echo "?")
            time_s=$(grep -o "Time:.*s" "$logfile" | grep -o "[0-9.]*" || echo "?")
            echo "  Model done: ${iterations} iterations, ${time_s}s, ${errors} tool errors"
        else
            echo "  Model FAILED (navra exit code $?)"
            FAIL=$((FAIL + 1))
            continue
        fi

        # Extract tool calls from MCP server log for the judge (only this run's portion)
        tool_calls_file="$RESULTS_DIR/${RUN_ID}-${slug}-${scenario}.tools.log"
        tail -c +$((log_offset + 1)) /tmp/mcp-e2e-server.log \
            | sed 's/\x1b\[[0-9;]*m//g' \
            | grep "Tool call\|Tool ok\|Tool fail" \
            > "$tool_calls_file" 2>/dev/null || true

        # Judge with Claude Opus via Vertex AI
        judgefile="$RESULTS_DIR/${RUN_ID}-${slug}-${scenario}.judge.log"
        rubric_file="$SCRIPT_DIR/scenarios/${scenario}.yaml"
        judge_template="$SCRIPT_DIR/judge.md"
        if [[ -f "$rubric_file" && -f "$judge_template" ]]; then
            echo "  Judging..."
            max_score=$(python3 -c "
import yaml, sys
with open('$rubric_file') as f:
    d = yaml.safe_load(f)
print(sum(c.get('weight', 1.0) for c in d.get('rubric', [])))
" 2>/dev/null || echo "?")
            export RUN_ID MODEL="$model" SCENARIO="$scenario" GWS_TEST_FOLDER_ID
            export PROMPT_CONTENT="$(cat "$pfile")"
            export RUBRIC_CONTENT="$(cat "$rubric_file")"
            export TOOL_CALLS="$(cat "$tool_calls_file")"
            export MAX_SCORE="$max_score"
            judge_prompt=$(envsubst < "$judge_template")

            if MCPD_TOKEN="$MCPD_TOKEN" navra run -e "http://127.0.0.1:$NAVRA_PORT/mcp" -m claude-opus-4-6@default -n 50 "$judge_prompt" >"$judgefile" 2>&1; then
                grep '^{' "$judgefile" > "$RESULTS_DIR/${RUN_ID}-${slug}-${scenario}.jsonl" 2>/dev/null || true
                total_line=$(grep '"_total"' "$RESULTS_DIR/${RUN_ID}-${slug}-${scenario}.jsonl" 2>/dev/null || echo "")
                if [[ -n "$total_line" ]]; then
                    score_str=$(echo "$total_line" | python3 -c "import sys,json; d=json.loads(sys.stdin.read()); print(f'{d.get(\"points\",d.get(\"score\",\"?\"))}/{d.get(\"max\",\"?\")}')" 2>/dev/null)
                    echo "  Judge score: ${score_str:-?}"
                else
                    echo "  Judge score: no total"
                fi
                PASS=$((PASS + 1))
            else
                echo "  Judge FAILED"
                PASS=$((PASS + 1))
            fi
        else
            echo "  No rubric or judge template — skipping judge"
            PASS=$((PASS + 1))
        fi

        # (log offset tracking replaces log truncation between runs)
    done
done

echo ""
echo "=== Summary ==="
echo "Total: $TOTAL  Pass: $PASS  Fail: $FAIL"
echo "Results in: $RESULTS_DIR/"
