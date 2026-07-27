You are judging the results of an E2E test of the mcp-google-workspace MCP server.

A model just ran a test scenario. Your job is to verify what actually exists in Google Drive — not what the model claimed to do.

## Context

- Run ID: $RUN_ID
- Model: $MODEL
- Scenario: $SCENARIO
- Test folder ID: $GWS_TEST_FOLDER_ID

## Prompt that was given to the model

$PROMPT_CONTENT

## Rubric

$RUBRIC_CONTENT

## Model tool calls

The following tool calls were recorded by the MCP server during the model's run. Use this to verify behavior-based criteria (e.g. "Model called gws_slides_read with slide_number").

$TOOL_CALLS

## Instructions

For each criterion in the rubric:

1. Use GWS tools to verify state-based criteria. Examples:
   - gws_drive_list to check if files/folders exist
   - gws_docs_read to check document content and structure
   - gws_docs_outline to verify headings
   - gws_sheets_read to verify spreadsheet data
   - gws_sheets_info to verify spreadsheet structure
   - gws_slides_read to verify presentation slides, titles, body text, notes
2. Use the "Model tool calls" section above to verify behavior-based criteria (e.g. "Model called X", "Model used parameter Y")
3. Score: 1.0 (fully met), 0.5 (partial), 0.0 (not met)
4. Write a brief note (under 100 chars) explaining the score

## Output format

Output ONLY JSONL lines — one per criterion, no other text:

{"run_id":"$RUN_ID","model":"$MODEL","scenario":"$SCENARIO","criterion_id":"<id>","score":<score>,"weight":<weight>,"points":<score * weight>,"note":"<note>"}

Where:
- score: 0.0 to 1.0 (proportion of criterion met)
- weight: from the rubric (importance of this criterion)
- points: score multiplied by weight (the actual points earned)

After all criteria, output one summary line (max is pre-computed from the rubric weights — use it exactly):

{"run_id":"$RUN_ID","model":"$MODEL","scenario":"$SCENARIO","criterion_id":"_total","points":<sum of all points>,"max":$MAX_SCORE}

Example: a criterion with weight 0.5 scored at 1.0 (full pass) earns 0.5 points.
A criterion with weight 1.0 scored at 0.5 (partial) earns 0.5 points.
Total can never exceed $MAX_SCORE.

## Scoring guidelines

- Judge state-based criteria on what GWS tools return, not what the model said
- Judge behavior-based criteria on the tool call log
- A document that exists but has wrong content is 0.5, not 1.0
- Empty spreadsheet (exists but no data) is 0.5 for "created", 0.0 for "has data"
- Cross-reference report content against source documents for "no hallucinated data"

## Cleanup

After judging, run cleanup actions from the rubric:
- action: trash — use gws_drive_list to find files matching name_pattern in the test folder, then gws_drive_trash each one
