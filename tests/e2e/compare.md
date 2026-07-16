---
name: e2e-compare
description: "Compare two E2E test runs to detect semantic regressions"
arguments:
  - name: previous
    description: "Path to previous summary file (e.g. tests/e2e/results/2026-07-14T10-00-00-summary.md)"
    required: true
  - name: current
    description: "Path to current summary file (e.g. tests/e2e/results/2026-07-15T10-00-00-summary.md)"
    required: true
---

Compare these two E2E test runs and produce a regression report.

## Steps

1. Read both summary files
2. For each model that appears in both runs, compare criterion scores
3. A **regression** is any criterion where the score decreased
4. An **improvement** is any criterion where the score increased
5. A **new criterion** is one that appears only in the current run (no comparison possible)

## Output

Write the comparison to stdout in this format:

```markdown
# Regression Report: <previous_run_id> → <current_run_id>

## Summary
- Regressions: N
- Improvements: N
- Unchanged: N

## Regressions (score decreased)

| Model | Scenario | Criterion | Previous | Current | Delta |
|-------|----------|-----------|----------|---------|-------|
| ... | ... | ... | 1.0 | 0.5 | -0.5 |

## Improvements (score increased)

| Model | Scenario | Criterion | Previous | Current | Delta |
|-------|----------|-----------|----------|---------|-------|
| ... | ... | ... | 0.5 | 1.0 | +0.5 |

## Model score trends

| Model | Previous total | Current total | Delta |
|-------|---------------|---------------|-------|
| ... | 8.5/10.0 | 9.0/10.0 | +0.5 |
```

If either file is missing or empty, report that and stop.

## Interpreting regressions

- A regression on a single model for "no_hallucinated_data" may be a model issue, not a server issue
- Regressions across ALL models for the same criterion suggest a server-side change broke something
- Regressions on tool usage criteria (e.g. "model used gws_docs_outline") after changing tool descriptions are the primary signal for semantic regression
