# mcp-google-workspace

MCP server for Google Workspace APIs with per-project safety policies.
Written in Rust, uses direct Google REST API calls (not a CLI wrapper).

## Architecture

```
main.rs           — CLI arg parsing, templates, interactive wizard, policy checker
handler.rs        — rmcp ServerHandler impl: tools, prompts, resources, completions, tasks, elicitation, subscriptions, lazy tool discovery
server.rs         — Tool dispatch, shared utilities (policy_for_folder, check_api_result), image gen, batch, tasks
calendar_helpers.rs — Google Calendar enrichment (6 tools): list, get, create, update, delete, freebusy + RSVP status
tools.rs          — Builds MCP tool list, lazy filtering by activated services, compact schema mode
execute.rs        — HTTP execution: URL rendering, params, pagination, resumable uploads, smart field defaults, rate limiting
format.rs         — Format transformers: Markdown/Plain → Docs batchUpdate, doc → Markdown reverse converter
helpers.rs        — Google Docs enrichment (9 tools): write/read/replace, outline, find, table, image, format
sheets_helpers.rs — Google Sheets enrichment (14 tools): read/write/append/clear, info, tabs, formatting, validation, named ranges, CSV, dimensions, formula analysis
drive_helpers.rs  — Google Drive enrichment (9 tools): list, find, create folder, copy, rename, move, share, trash, info
slides_helpers.rs — Google Slides enrichment (9 tools): read, add, update, duplicate, delete, reorder, Marp import, templates, image gen
gmail_helpers.rs  — Gmail enrichment (10 tools): search, read, thread, attachment, contacts, forward, draft, send, reply, labels + RFC 2822 builder, MIME decoder
cache.rs          — LRU + TTL in-memory cache for Sheets values.get responses
rate_limit.rs     — Per-service sliding-window rate limiter for Google API quotas
resources.rs      — MCP resources: gws:// URI scheme, resource templates from Discovery Documents
completions.rs    — MCP completions: autocomplete for resource URIs and prompt arguments
elicitation.rs    — MCP elicitation: structured user input (folder selection, overwrite confirmation)
subscriptions.rs  — MCP subscriptions: Google Drive watch channels, webhook notifications
prompts.rs        — MCP prompts: load external Markdown files, argument substitution
policy.rs         — JSON policy engine: constraints, method denylists, read-only mode, body-write-only, recursive parent ancestry
auth.rs           — OAuth2 chain: env var → credentials file → service account → ADC/gcloud
audit.rs          — Structured JSONL audit log writer with tool name tracking
http.rs           — Hybrid Axum server: rmcp StreamableHttpService + health/metrics/webhooks
tasks.rs          — Task lifecycle for resumable uploads and chunked downloads
metrics.rs        — Prometheus counters, histograms, gauges (including rate limit waits)
meta.rs           — Request metadata (W3C Trace Context) for Google API header propagation
image_gen.rs      — Gemini image generation and Drive upload
marp.rs           — Marp Markdown to Google Slides conversion
```

## Tools (59 in eager mode, 2 in lazy mode)

| Service | Tools | Notes |
|---------|-------|-------|
| Meta | `gws_discover`, `gws_batch` | Always available. gws_discover activates services in lazy mode. |
| Drive | 9 tools (`gws_drive_*`) | list, find_folder, info, create_folder, copy, rename, move, share, trash |
| Docs | 9 tools (`gws_docs_*`) | write (creates or updates), read, replace_section, outline, find, insert_table, insert_image, read_table, format |
| Sheets | 14 tools (`gws_sheets_*`) | read, write (creates or updates), append, clear, info, manage_tabs, trace, explain, formulas, format, validate, named_range, csv, dimensions |
| Slides | 9 tools (`gws_slides_*`) | read, add, update, duplicate, delete, reorder, import_marp, templates, generate_image |
| Gmail | 10 tools (`gws_gmail_*`) | search, read, thread, attachment, contacts, forward, draft, send, reply, labels |
| Calendar | 6 tools (`gws_calendar_*`) | list, get, create, update, delete, freebusy |

## Key Design Decisions

- **Discovery-driven**: Fetches Google Discovery Documents at runtime.
  New API endpoints appear automatically.
- **Policy-as-code**: JSON file scopes per-project access.
  Constraints, method denylists, read-only mode, body-write-only parent restrictions
  with recursive ancestry checking. Gmail `allowed_labels` scopes message access to
  specific labels (search injection, read/modify/reply verification, label target restriction).
- **Lazy tool discovery**: Default: only `gws_discover` + `gws_batch` visible.
  When model calls `gws_discover(service="sheets")`, sheets helpers are activated
  and `ToolListChangedNotification` is sent. `--eager-tools` flag loads all at startup.
- **No generic service tools**: Services with helpers (drive, docs, sheets, slides, gmail, calendar)
  suppress their generic tool. Models use helpers only — no ambiguity.
- **Create-on-write**: `gws_docs_write` and `gws_sheets_write` create new files
  when `title` is provided instead of `document_id`/`spreadsheet_id`. Same pattern
  for both services.
- **Sheet name resolution**: Advanced sheets tools accept `sheet` (tab name) as
  alternative to `sheet_id`. Server resolves name to numeric ID automatically.
- **Sheet caching**: LRU + TTL in-memory cache for Sheets reads (20 entries, 5 min TTL).
  Transparent — checked before API call, invalidated after writes.
- **Formula analysis**: Local parsing — regex cell reference extraction, function-to-English
  translation, dependency tracing. Only API calls are spreadsheets.values.get.
- **Small model optimizations**: Short descriptions, smart defaults (range defaults to Sheet1),
  auto-detection (folder ID vs spreadsheet ID), prescriptive error messages with examples,
  data normalization (array-of-objects → array-of-arrays).
- **MCP prompts**: Workflow recipes in `prompts/` with JSON tool call examples.
  Fetched via MCP `prompts/get` — user-controlled, not auto-injected.
  Context-expensive (~1K tokens each) — hurt 8K-context models.
- **Tool response logging**: Every tool call logs ok/error/failed with duration.
  Sheets create-on-write logs data write results.

## Build and Test

```bash
cargo check                      # Type-check
cargo test                       # 361 unit tests
cargo build --release            # Release binary

# E2E tests (requires Google auth + navra + Ollama)
export GWS_TEST_FOLDER_ID=... GWS_PROJECT_ID=... MCPD_TOKEN=...
bash tests/e2e/run.sh            # Full matrix: 5 scenarios × 5 models with Opus judge
bash tests/e2e/run.sh --model gemma4:e4b --scenario full-e2e  # Single run
```

## Running

```bash
# Lazy discovery (default) — 2 tools initially, more on demand
./target/release/mcp-google-workspace --policy gws-policy.json --prompts-dir ./prompts

# Eager mode — all 37 tools at startup
./target/release/mcp-google-workspace --policy gws-policy.json --eager-tools

# HTTP transport
./target/release/mcp-google-workspace --policy gws-policy.json --http 127.0.0.1:3100 --eager-tools
```

## E2E Test Harness

All test config in `tests/e2e/`:
- `policy.template.json` — policy with env var placeholders
- `navra.toml` — navra config for local + Vertex AI models
- `run.sh` — builds, starts servers, runs models, judges with Claude Opus
- `scenarios/` — YAML rubrics with quality criteria
- `judge.md` — Opus judge prompt template (envsubst variables)

The judge uses the same GWS MCP tools to verify what was actually created in Drive.

## navra integration

See `tests/e2e/navra.toml`. Key points:
- HTTP transport pointing at `127.0.0.1:3100/mcp`
- Tool classifications for all 59 tools (network domain, read/write)
- Models: gemma4:e4b, gemma4:26b, qwen3:8b, qwen3.6:35b, claude-sonnet-4-5, claude-opus-4-6
- MCP prompts injectable via `--upstream-prompt google-workspace:work-with-spreadsheet`

## Code Conventions

- No comments unless the why is non-obvious
- Error handling via `google_workspace::error::GwsError`
- Tracing goes to stderr, MCP JSON-RPC to stdout
- Policy tests use `#[cfg(test)]` inline in each module
- Tool descriptions: short (one sentence), direct, matching across services
- Tool schemas: required params only, examples in param descriptions
