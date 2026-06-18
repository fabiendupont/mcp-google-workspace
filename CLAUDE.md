# mcp-google-workspace

MCP server for Google Workspace APIs with per-project safety policies.
Written in Rust, uses direct Google REST API calls (not a CLI wrapper).

## Architecture

```
main.rs       — CLI arg parsing, templates, interactive wizard, policy checker
server.rs     — JSON-RPC stdio loop, dual-era MCP dispatch, request explanation
protocol.rs   — Typed JSON-RPC layer: request parsing, error codes, response construction
meta.rs       — Request metadata extraction (_meta, W3C Trace Context)
tools.rs      — Builds MCP tool list from Google Discovery Documents, handles gws_discover
execute.rs    — HTTP execution: URL template rendering, params, pagination, auto-resumable uploads
policy.rs     — JSON policy engine: generic constraints, method denylists, read-only mode
auth.rs       — OAuth2 chain: env var → credentials file → service account → ADC/gcloud
audit.rs      — Structured JSONL audit log writer
http.rs       — Axum HTTP server, SSE streaming, rate limiter, SIGHUP reload, session IDs
tasks.rs      — Task lifecycle for resumable uploads and chunked downloads
metrics.rs    — Prometheus counters, histograms, gauges
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
- **Direct API calls**: Uses `reqwest` + `yup-oauth2` to call googleapis.com
  directly. The `google-workspace` crate (path dependency) provides Discovery
  Document types, service registry, HTTP client with retry, and validation.

## Dependencies

- `google-workspace` crate: git dependency from `github.com/googleworkspace/cli` (pinned to rev `a3768d0`).
- `dialoguer` crate: interactive terminal prompts for the policy wizard.
- OAuth2 credentials: Requires one of the 7 sources in the credential chain.

## Build and Test

```bash
cargo check          # Type-check
cargo test           # 185 unit tests across all modules
cargo build --release
```

## Running

```bash
# With policy file (recommended)
./target/release/mcp-google-workspace --policy gws-policy.json

# With template
./target/release/mcp-google-workspace --init-policy --template assistant > policy.json

# With service list (no constraints)
./target/release/mcp-google-workspace --services drive,gmail,calendar

# HTTP transport with audit log
./target/release/mcp-google-workspace --policy gws-policy.json --http 127.0.0.1:3000 --audit-log audit.jsonl
```

## Claude Code integration

Add to `.claude/settings.json`:
```json
{
  "mcpServers": {
    "google-workspace": {
      "command": "/path/to/mcp-google-workspace",
      "args": ["--policy", "/path/to/gws-policy.json"]
    }
  }
}
```

## Code Conventions

- No comments unless the why is non-obvious
- Error handling via `google_workspace::error::GwsError`
- Tracing goes to stderr, MCP JSON-RPC to stdout
- Policy tests use `#[cfg(test)]` inline in each module
