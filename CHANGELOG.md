# Changelog

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

- **Policy Engine**: TOML-based per-project access control
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
