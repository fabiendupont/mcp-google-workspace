# mcp-google-workspace

MCP server for Google Workspace APIs with per-project safety policies.
Written in Rust, uses direct Google REST API calls (not a CLI wrapper).

## Architecture

```
main.rs           — CLI arg parsing, templates, interactive wizard, policy checker
handler.rs        — rmcp ServerHandler impl: tools, prompts, resources, completions, tasks, elicitation, subscriptions
server.rs         — Tool dispatch business logic, Docs/Slides/Batch helpers, request explanation
tools.rs          — Builds MCP tool list from Discovery Documents, compact schema mode
execute.rs        — HTTP execution: URL rendering, params, pagination, resumable uploads, smart field defaults
format.rs         — Format transformers: Markdown/Plain → Docs batchUpdate, doc → Markdown reverse converter
helpers.rs        — Google Docs enrichment: write/read/table tools, insert text/image, find text, structure outline
resources.rs      — MCP resources: gws:// URI scheme, resource templates from Discovery Documents
completions.rs    — MCP completions: autocomplete for resource URIs and prompt arguments
elicitation.rs    — MCP elicitation: structured user input (folder selection, overwrite confirmation)
subscriptions.rs  — MCP subscriptions: Google Drive watch channels, webhook notifications
prompts.rs        — MCP prompts: load external Markdown files, argument substitution
policy.rs         — JSON policy engine: constraints, method denylists, read-only mode, compact schemas
auth.rs           — OAuth2 chain: env var → credentials file → service account → ADC/gcloud
audit.rs          — Structured JSONL audit log writer
http.rs           — Hybrid Axum server: rmcp StreamableHttpService + health/metrics/webhooks
tasks.rs          — Task lifecycle for resumable uploads and chunked downloads
metrics.rs        — Prometheus counters, histograms, gauges
meta.rs           — Request metadata (W3C Trace Context) for Google API header propagation
```

## Key Design Decisions

- **Discovery-driven**: Fetches Google Discovery Documents at runtime to learn each
  API's resources/methods/parameters. No hardcoded API list — new endpoints appear
  automatically.
- **Policy-as-code**: A JSON file scopes what an agent can access per-project.
  Generic parameter constraints, method denylists, and read-only mode.
  See `policy.example.json`.
- **One tool per service**: Each Google service (drive, gmail, calendar) is exposed
  as a single MCP tool. The agent specifies `resource` and `method` as arguments.
  `gws_discover` is a meta-tool for schema introspection.
- **Consolidated docs tools**: `gws_docs_write` and `gws_docs_read` handle most
  document operations. `gws_docs_write` accepts Markdown or plain text and converts
  to native Google Docs formatting (headings, bullets, tables, bold, italic).
  `gws_docs_read` returns compact structure, Markdown, or plain text.
  `gws_docs_insert_table` creates and populates tables from JSON arrays.
  Legacy individual tools remain available in normal mode.
- **Small model optimizations**: Compact schema mode (`--compact-schemas`) reduces
  tool count from 19 to 10. Smart field defaults auto-select response fields for
  19 common API calls. Google metadata stripping removes 30+ noisy fields. Tool
  descriptions follow MCP spec research (PURPOSE, WHEN TO USE, HOW TO USE, LIMITATIONS).
- **Format transformers**: Pluggable content format conversion via `format.rs`.
  Markdown and Plain text supported. Extensible for RST, AsciiDoc.
- **MCP prompts**: External Markdown files with YAML frontmatter in `prompts/` directory,
  loaded at startup. Teaches models workflow recipes.
- **rmcp-based transport**: Uses the `rmcp` crate (official Rust MCP SDK) for
  protocol handling, stdio transport, and Streamable HTTP server with session management.
- **Full MCP capabilities**: tools, prompts, resources (with subscribe), completions,
  tasks (via OperationProcessor), elicitation, progress notifications.
- **Direct API calls**: Uses `reqwest` + `yup-oauth2` to call googleapis.com
  directly. The `google-workspace` crate (path dependency) provides Discovery
  Document types, service registry, HTTP client with retry, and validation.

## Dependencies

- `rmcp` crate: Official Rust MCP SDK — ServerHandler trait, stdio/HTTP transports,
  elicitation, task management.
- `google-workspace` crate: git dependency from `github.com/googleworkspace/cli` (pinned to rev `a3768d0`).
- `pulldown-cmark` crate: Markdown parsing for the Docs enrichment converter.
- `dialoguer` crate: interactive terminal prompts for the policy wizard.
- OAuth2 credentials: Requires one of the 7 sources in the credential chain.

## Build and Test

```bash
cargo check          # Type-check
cargo test           # 292 unit tests across all modules
cargo build --release
```

## Running

```bash
# With policy file (recommended)
./target/release/mcp-google-workspace --policy gws-policy.json

# With compact schemas for small models (10 tools instead of 19)
./target/release/mcp-google-workspace --policy gws-policy.json --compact-schemas

# HTTP transport with webhooks for subscriptions
./target/release/mcp-google-workspace --policy gws-policy.json --http 127.0.0.1:3000 --external-url https://mcp.example.com

# With MCP prompts directory
./target/release/mcp-google-workspace --policy gws-policy.json --prompts-dir ./prompts

# With service list (no constraints)
./target/release/mcp-google-workspace --services drive,gmail,calendar

# Check credential chain
./target/release/mcp-google-workspace --check-auth
```

## Claude Code integration

Add to `.mcp.json` in your project:
```json
{
  "mcpServers": {
    "google-workspace": {
      "command": "/path/to/mcp-google-workspace",
      "args": [
        "--policy", "/path/to/gws-policy.json",
        "--prompts-dir", "/path/to/prompts"
      ]
    }
  }
}
```

## navra integration (local models via Ollama)

See `navra-gws-test.toml` for a complete navra config. Key points:
- Transport: stdio (navra spawns the MCP server)
- Tool classification: all GWS tools classified as `network` domain
- Use `--compact-schemas` for small models (reduces tool count)
- Tested with: Claude (full), Qwen3.6 35B (full), Gemma4 26B (full with retries)

## Code Conventions

- No comments unless the why is non-obvious
- Error handling via `google_workspace::error::GwsError`
- Tracing goes to stderr, MCP JSON-RPC to stdout
- Policy tests use `#[cfg(test)]` inline in each module
- Tool descriptions follow PURPOSE/WHEN/HOW/LIMITATIONS structure
