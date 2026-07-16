---
name: e2e-judge
description: "Run E2E test scenarios through models via navra, then judge results using GWS tools"
arguments:
  - name: models
    description: "Comma-separated list of model names to test (e.g. gemma4:26b,qwen3.6:35b)"
    required: true
  - name: scenarios
    description: "Comma-separated scenario names to run (default: all in tests/e2e/scenarios/)"
    required: false
---

You are running E2E semantic regression tests for the mcp-google-workspace MCP server.

## Setup

1. Read the project root to find `tests/e2e/scenarios/*.yaml`
2. Parse the `models` argument into a list
3. If `scenarios` is provided, filter to only those; otherwise run all
4. Create a run ID from the current timestamp: `YYYY-MM-DDTHH-MM-SS`

## For each model × scenario combination

### Step 1: Run the test model

Execute the scenario's prompt through the model via navra embedded mode:

```
navra run -m <model> "$(cat <prompt_file>)"
```

Wait for completion. If it fails or times out, record all criteria as score 0.0 with note "model failed/timed out".

### Step 2: Judge each criterion

For each criterion in the scenario's rubric:

1. Read the `check` field to understand what to verify
2. Use the GWS MCP tools (gws_drive_list, gws_docs_read, gws_docs_outline, gws_sheets_read, etc.) to inspect the actual state in Google Drive
3. Score the criterion:
   - **1.0** — fully met
   - **0.5** — partially met (e.g. table exists but has fewer rows than required)
   - **0.0** — not met
4. Write a brief note (under 100 chars) explaining the score

### Step 3: Record results

Append one JSONL line per criterion to `tests/e2e/results/<run_id>-<model_slug>.jsonl`:

```json
{"run_id":"<run_id>","model":"<model>","scenario":"<scenario_name>","criterion_id":"<id>","score":<score>,"max":<weight>,"pass":<score >= weight>,"note":"<note>"}
```

Where `model_slug` replaces `:` and `/` with `-` (e.g. `gemma4-26b`).

### Step 4: Cleanup

For each cleanup action in the scenario:
- `action: trash` — use `gws_drive_list` to find files matching `name_pattern`, then `gws_drive_trash` each one

## After all combinations

Write a markdown summary to `tests/e2e/results/<run_id>-summary.md`:

```markdown
# E2E Test Run — <run_id>

## Configuration
- Models: <list>
- Scenarios: <list>
- Test folder: <folder_id>

## Results

### <scenario_name>

| Criterion | <model_1> | <model_2> | ... |
|-----------|-----------|-----------|-----|
| <criterion> | PASS / FAIL (note) | ... | ... |
| **Total** | **X.X / Y.Y** | ... | ... |

## Regressions

Compare against the most recent previous summary in `tests/e2e/results/` (if any).
List any criterion where a model's score decreased from the previous run.
If no previous run exists, note "No baseline — this is the first run."
```

## Scoring guidelines

- Judge objectively based on what the GWS tools return, not on what the model said it did
- A document that exists but has wrong content is a partial pass (0.5), not a full pass
- "No hallucinated data" requires cross-referencing against source documents when applicable
- Tool usage criteria (e.g. "model used gws_docs_outline") can be verified from the navra output log
