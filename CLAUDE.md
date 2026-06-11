# mcp-google-workspace

MCP server for Google Workspace APIs with per-project safety policies.
Written in Rust, uses direct Google REST API calls (not a CLI wrapper).

## Architecture

```
main.rs       — CLI arg parsing (--policy / --services), tokio entrypoint
server.rs     — JSON-RPC stdio loop, dual-era MCP dispatch (2024-11-05 + 2026-07-28)
protocol.rs   — Typed JSON-RPC layer: request parsing, error codes, response construction
meta.rs       — Request metadata extraction (_meta, W3C Trace Context)
tools.rs      — Builds MCP tool list from Google Discovery Documents, handles gws_discover
execute.rs    — HTTP execution: URL template rendering, params, pagination, policy enforcement
policy.rs     — TOML policy engine: per-service, per-folder, per-calendar, method denylists
auth.rs       — OAuth2 chain: env var → credentials file → service account → ADC/gcloud
resolve.rs    — Drive folder path → ID resolution at startup
```

## Key Design Decisions

- **Discovery-driven**: Fetches Google Discovery Documents at runtime to learn each
  API's resources/methods/parameters. No hardcoded API list — new endpoints appear
  automatically.
- **Policy-as-code**: A TOML file scopes what an agent can access per-project.
  Supports folder-level Drive ACLs, per-calendar access, method denylists, and
  global read-only mode. See `policy.example.toml`.
- **One tool per service**: Each Google service (drive, gmail, calendar) is exposed
  as a single MCP tool. The agent specifies `resource` and `method` as arguments.
  `gws_discover` is a meta-tool for schema introspection.
- **Direct API calls**: Uses `reqwest` + `yup-oauth2` to call googleapis.com
  directly. The `google-workspace` crate (path dependency) provides Discovery
  Document types, service registry, HTTP client with retry, and validation.

## Dependencies

- `google-workspace` crate: git dependency from `github.com/googleworkspace/cli` (pinned to rev `a3768d0`).
  This is a library crate from the gws CLI project — it provides Discovery Document
  parsing, service name resolution, shared HTTP client, and input validation.
- OAuth2 credentials: Requires one of: `GWS_ACCESS_TOKEN` env var, `~/.config/gws/credentials.json`,
  service account key, or Application Default Credentials (gcloud auth).

## Build and Test

```bash
cargo check          # Type-check
cargo test           # 33 unit tests across protocol.rs, meta.rs, policy.rs
cargo build --release
```

## Running

```bash
# With policy file (recommended)
./target/release/mcp-google-workspace --policy gws-policy.toml

# With service list (no constraints)
./target/release/mcp-google-workspace --services drive,gmail,calendar
```

## Claude Code integration

Add to `.claude/settings.json`:
```json
{
  "mcpServers": {
    "google-workspace": {
      "command": "/path/to/mcp-google-workspace",
      "args": ["--policy", "/path/to/gws-policy.toml"]
    }
  }
}
```

## What's Done

- Full MCP JSON-RPC server over stdio with dual-era support (2024-11-05 legacy + 2026-07-28 modern)
- Typed JSON-RPC protocol layer with correct error codes (-32700, -32600, -32601, -32602, -32603)
- `server/discover` method for modern capability discovery (SEP-2575)
- `_meta` extraction with W3C Trace Context forwarding (SEP-414)
- Cursor-based pagination on `tools/list` with `ttlMs`/`cacheScope` cache hints (SEP-2549)
- Tool `title`, `annotations` (readOnlyHint, destructiveHint, etc.)
- `structuredContent` in tool call results alongside text `content`
- Media upload (base64 → multipart/related) and download (binary → base64 MCP content)
- Discovery document caching (fetched once, reused across requests)
- Narrowest-scope OAuth selection (prefers `.readonly` suffixes)
- Hard-fail on auth failure (no silent unauthenticated requests)
- Drive folder enforcement for `addParents`/`removeParents` query params
- Pre-initialization blocking for legacy clients
- Request ID uniqueness tracking
- Progress notifications during auto-pagination
- Policy engine with TOML parsing and 21 passing unit tests
- `ping` support
- Sorted, deterministic tool ordering

## What's Missing

No major features remain. Potential future work:
- Resumable uploads for files > 10MB
- Streamable HTTP transport (currently stdio-only)

## Code Conventions

- No comments unless the why is non-obvious
- Error handling via `google_workspace::error::GwsError`
- Tracing goes to stderr, MCP JSON-RPC to stdout
- Policy tests use `#[cfg(test)]` inline in policy.rs
