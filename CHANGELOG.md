# Changelog

## [0.9.0] - 2026-11-17

### Added
- **Google Tasks service**: 8 curated `gws_tasks_*` tools (list task lists, list/get/create/update/complete/delete tasks, clear completed) with task-list auto-resolution and proper read-only/destructive annotations
- **Write confirmation (`confirm_writes`)**: new `server.confirm_writes` policy option requiring explicit user approval via MCP elicitation before any mutating operation — data-driven from tool annotations, with identifying argument summaries (recipients, titles, IDs); `dry_run` skips confirmation, writes are denied when elicitation is unavailable, `gws_batch` is disabled in this mode
- **Polling subscriptions**: `subscribe` now works without `--external-url` (stdio mode) by re-reading the resource on a schedule and emitting `notifications/resources/updated` on content change; works for any service with a `get` method, not just Drive; interval configurable via `server.subscription_poll_secs` (default 60s)
- **Multi-user HTTP mode**: `--users-dir` serves many users from one process — each with isolated OAuth token cache, policy, credentials, subscriptions, tool cache, and a per-user audit log at `<user_dir>/audit.jsonl`; per-user endpoints at `/mcp/<user>` with optional `Authorization: Bearer` authentication via `server.api_token`; per-user SIGHUP policy reload; Drive webhook fan-out
- **Workflow recipes**: `prompts/thread-to-doc.md` and `prompts/doc-to-slides.md` few-shot recipes for multi-service workflows (agents orchestrate primitive tools; recipes are user-editable without recompiling)
- **`split_headings` option on `gws_slides_import_marp`**: deterministic plain-Markdown → slides conversion (each H1/H2 heading starts a slide)
- **Registry manifests**: submission-ready `server.json` (official MCP Registry schema, validated), `glama.json`, Docker MCP Catalog payload, and a submissions checklist in `registry/`; 6 CI guards prevent manifest drift from the crate version
- **`server.api_token`** and **`server.subscription_poll_secs`** policy options

### Fixed
- **Security advisories in transitive dependencies**: rustls 0.23.46 (RUSTSEC-2026-0285, RUSTSEC-2026-0346), rustls-webpki 0.103.17, tokio 1.54.1 (AsyncFd unsoundness), pulldown-cmark 0.13.6 (RUSTSEC-2026-0414/0433), rmcp 3.10.0 (RUSTSEC-2026-0457/0461/0462); chrono unmaintained advisory (RUSTSEC-2026-0543) ignored with rationale in deny.toml

### Changed
- **rmcp 3.2 → 3.10**: migrated deprecated `ServerInfo` alias to `ServerConfig`
- **Multi-user mode limitation**: per-policy `rate_limit_rpm` is not enforced across users (logged as a warning at startup)

## [0.8.2] - 2026-09-02

### Added
- **Policy templates**: `content-writer` (Drive/Docs/Slides/Sheets RW, Gmail draft-only) and `it-admin` (all services, most read-only, Calendar RW)
- **Integration tests**: 54 tests covering policy enforcement, template round-trips, origin security, and policy.example.json validation

### Fixed
- **IPv6 origin matching**: `http://[::1]:3000` now correctly matches the loopback allowlist
- **Doc/code drift**: architecture.md module map aligned with actual source, TOML→JSON references corrected, README CLI syntax updated to subcommand form

### Changed
- **rmcp 2.2 → 3.2**: Migrated to TaskManager API, updated return types (CallToolResponse, ReadResourceResponse, GetPromptResponse), added SEP-2322 result_type fields
- **Dependencies**: base64 0.23.1, clap 4.6.6, tower-http 0.7.1, h2 0.4.19 (RUSTSEC-2026-0258)

## [0.1.0] - 2026-06-12

### Added

- **MCP Protocol**: Full 2026-07-28 RC compliance with 2024-11-05 / 2025-11-25 backward compatibility
  - `server/discover` for modern capability negotiation
  - `_meta` extraction with W3C Trace Context forwarding
  - Cursor-based pagination on `tools/list` with `ttlMs` / `cacheScope`
  - Tool `title`, `annotations` (readOnlyHint, destructiveHint, etc.)
  - `structuredContent` in tool call results
  - Dual-era client detection (legacy `initialize` + modern `_meta`)
  - Pre-initialization blocking for legacy clients

- **Transports**: Stdio (default) and Streamable HTTP (`--http <addr:port>`)
  - SSE notification stream on `GET /mcp`
  - Graceful shutdown on SIGTERM/SIGINT

- **Policy Engine**: JSON-based per-project access control
  - Service allow-list (only listed services are exposed)
  - Folder-level Drive ACLs (read-only / read-write per folder)
  - Per-calendar permissions with access levels
  - Method denylists (e.g., block `messages.delete`)
  - Global and per-service read-only mode
  - Rate limiting (per-client-IP sliding window)
  - Request body size limit (configurable)
  - Origin validation (configurable allowlist, default localhost)
  - `credentials_file` and `project_id` fields

- **Media**: Upload and download with no size cap
  - Multipart upload for files ≤ 10MB
  - Resumable upload for larger files (chunked via `upload_handle`)
  - Chunked download via `download_handle`
  - Base64 encoding for MCP protocol transport

- **Tasks Extension**: `io.modelcontextprotocol/tasks`
  - Upload and download sessions as tasks
  - `tasks/get`, `tasks/result`, `tasks/cancel`, `tasks/list`
  - Task lifecycle: working → completed / failed / cancelled

- **Credentials**: Multi-source credential chain
  - Raw token via env var
  - Policy `credentials_file`
  - GWS CLI keyring via `gws auth export --unmasked`
  - Application Default Credentials
  - Service account keys
  - Token caching (3500s TTL, avoids rebuilding authenticator per request)

- **Observability**
  - Structured logging via `tracing` (stderr)
  - OpenTelemetry trace export via OTLP/HTTP
  - Prometheus metrics at `/metrics` (request count, latency, errors, active tasks)
  - Health/readiness/liveness probes (`/healthz`, `/readyz`, `/livez`)

- **Security**
  - Origin validation with proper URL hostname parsing (no substring bypass)
  - CRLF injection prevention in multipart headers
  - Deny-by-default for Drive writes without `parents`
  - Deny-by-default for Calendar operations without `calendarId`
  - Hard-fail on authentication errors

- **Distribution**
  - Multi-arch container image (linux/amd64 + linux/arm64) on UBI 9 Micro
  - Optimized release binary (~5 MB stripped with LTO)
  - Kubernetes manifests with kustomize
  - GitHub Actions CI (check, test, clippy, fmt, cargo-deny)
  - Release workflow with Trivy scan and auto-generated release notes
  - Dependabot for Cargo, GitHub Actions, and Dockerfile
