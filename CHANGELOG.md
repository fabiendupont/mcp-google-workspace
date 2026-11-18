# Changelog

## [Unreleased]

### Security

Hardening pass that makes every previously documented security claim real. No
action is required for stdio users; HTTP and container deployments must supply a
bearer token (or explicitly opt out) and may need to renew Drive subscriptions.

**Critical**

- **HTTP transport authentication is enforced.** A configured token
  (`--api-token`, `GWS_API_TOKEN`, or `server.api_token`) now protects both
  `/mcp` and `/metrics`. A non-loopback bind without a token refuses to start
  instead of silently exposing every policy-allowed tool.
- **HTTP authentication cannot be turned off by a reload.** The *requirement* to
  authenticate is latched at boot; SIGHUP still rotates `server.api_token` (both
  tokens accepted immediately), but removing the only token now leaves `/mcp` and
  `/metrics` answering `401` with an error log instead of opening the listener.
  `--api-token` / `GWS_API_TOKEN` are captured at boot and unaffected by reload.
- **Client-requested delegation is allowlisted.** `_meta.user_email` is rejected
  unless the address appears in the new `server.allowed_subjects`; an empty list
  disables per-request delegation. `server.delegate_subject` remains the
  operator-controlled fallback.
- **Policy denials cannot be bypassed by method aliases.** Method-family denials
  now cover equivalent destructive calls (`messages.send` / `drafts.send`,
  `files.delete` / `files.emptyTrash`, `messages.delete` / `batchDelete`), and
  the `--services` shorthand applies baseline denials.
- **Drive read-by-ID is scoped.** `files.get`, `files.export`, `files.copy`
  (source), and the Docs, Sheets, and Slides readers are checked against
  recursive `parents` roots instead of trusting any file ID the credential can
  read. The metadata lookup uses its own Drive read-only token, and `files.list`
  keeps its folder filter even under `body-write-only`.
- **Gmail label policy fails closed** when configured labels do not resolve.
- **Drive webhook notifications are authenticated** with a per-channel secret and
  unpredictable channel IDs; bad tokens are rejected and unknown channels are
  not acknowledged.

**High**

- **`allowed_resources` is enforced on tool execution**, `gws://` resources, and
  subscriptions, not only during discovery.
- **`gws_batch` rejects helper-only services** (Drive, Docs, Sheets, Slides,
  Gmail, Calendar, Tasks) so it cannot bypass tool-level policy.
- **Protect-mode Drive constraints block moves** both into and out of a protected
  folder; `body-write-only` requires `parents` on create and copy.
- **Origin validation is enforced** for any request that carries an `Origin`
  header; an empty allowlist means localhost only.
- **Request body limits are enforced** from `server.max_request_bytes`.
- **Audit coverage is complete**: one line per `tools/call` with the real status
  and optional subject, real HTTP status for generic and batch API calls, and
  policy denials recorded. Denials are classified from a typed signal, so every
  policy refusal — including protected-value denials, constraint violations, and
  folder-scope failures — is `action: "denied"` with `status: 403` and a reason;
  non-policy failures are `action: "error"` with their real status, never a 500
  labelled allowed.
- **Drive webhooks are validated before use.** `/webhooks/drive` checks the
  channel id (≤128 bytes), channel token (≤96 bytes), and resource state (fixed
  set) before any lookup or log line, returns `400` for invalid values without
  echoing them, logs only a short prefix plus a hash, never reads the body, and
  has its own per-client-IP bucket.
- **Temporary public sharing for Docs image insertion is revoked** after the
  insert instead of being left behind, and the grant is refused up front when
  policy would deny the cleanup (`permissions.delete`). A failed revoke is
  reported as an error naming the file and permission.
- **Drive helpers reject malformed IDs and cannot publish to link-anyone.**
  `file_id`, `folder_id`, `parent_id`, and `to_folder_id` must match
  `^[A-Za-z0-9_-]+$` (at least 20 characters, `root` allowed) and free-text Drive
  query values are escaped (backslash, then `'`, control characters stripped), so
  a pasted URL or an ID containing `/` or a space is rejected before any request
  instead of corrupting the Drive query. `gws_drive_share` accepts only
  `reader`/`commenter`/`writer`, derives the permission `type` from `email` vs
  `domain`, and never grants `anyone`/`anyoneWithLink`.
- **`gws_drive_trash` is classified as destructive.** It carries
  `destructiveHint: true` and checks the whole
  `files.trash`/`files.delete`/`files.emptyTrash` family before issuing its
  `files.update {trashed: true}`, returning a `Trash blocked` policy error with
  no request when any of them is denied — so `denied_methods: ["files.delete"]`
  also blocks trashing. Trashing remains recoverable from the Drive trash.

**Medium**

- **Rate limiting is layered.** A fixed pre-authentication bucket keyed on the
  client IP (about 60/min) bounds credential guessing before any token comparison;
  an authenticated bucket keyed on the credential uses `server.rate_limit_rpm`
  (default 600/min). Both return `429` with `Retry-After`, track at most 1024
  clients, and in multi-user mode give each tenant an independent authenticated
  budget. The new `server.trusted_proxies` CIDR list is the only way
  `X-Forwarded-For` is honoured (rightmost untrusted hop); otherwise the socket
  peer is the key. `rate_limit_rpm: 0` is now a startup error instead of
  "unlimited", and `server.rate_limits` is validated as a per-service **outbound**
  map (unknown keys and zero values are startup errors).
- **Recursive `parents` constraints gate by-ID writes too.** `permissions.create`,
  `files.update`/`delete`, `documents.batchUpdate`,
  `spreadsheets.values.update`/`append`/`clear`, `spreadsheets.batchUpdate`, and
  `presentations.batchUpdate` are ancestry-checked, and a `parents` constraint no
  longer wrongly blocks `permissions.create`/`files.update`. `recursive: false`
  still disables by-ID scoping; startup and `check-policy` now warn loudly about
  it, about a non-read-only service with no constraints, an unconstrained
  calendar, and Gmail settings/forwarding that is not denied. `check-policy`
  exits non-zero when warnings are present.
- **Least privilege in eager mode.** `--eager-tools` filters helper tools through
  the policy's allowed services, so only permitted services appear in
  `tools/list` (previously every helper was exposed).
- **Approval prompts show the security-relevant arguments.** Per-tool summaries
  include share recipient/role/type, Gmail to/cc/bcc plus subject with body length
  and first line, calendar id/start/attendee count, and Sheets spreadsheet
  id/range/row-column counts/first row. Values are control-character-sanitised
  and newlines render as `⏎`.
- **`gws_generate_image` is a confirmable write.** It appears in `tools/list`
  with `readOnlyHint: false`, is discoverable through `gws_discover`, requires the
  same `confirm_writes` approval as other mutating tools, and honours the
  delegated `_meta.user_email` subject in multi-user mode.
- **`gws_batch` reports per-entry failures honestly.** A per-entry `dry_run`
  previews without HTTP, and a sub-request whose payload carries an API error is
  `"status":"error"`, excluded from `succeeded`, and counted in `failed`.
- **Gmail thread reads validate every message.** Non-matching messages are
  dropped and the result is marked `partialRead: true` with
  `omittedMessageCount`; `messageCount` counts returned messages.
- **Gmail Markdown rendering is bounded.** `format: "markdown"` bodies are capped
  at about 1 MiB total and 64 KiB per line with a linear renderer; links are
  limited to `http`/`https`/`mailto`/`tel`, other schemes render as text, and
  quoted content is HTML-escaped.
- **`init` cannot be tricked into clobbering.** It refuses to write through a
  symlink and asks before overwriting an existing file. The wizard text states
  that the "folders" answer controls create/copy placement and read-by-ID scoping
  — not by-ID rename/trash, which `denied_methods`/`read_only` govern.
- **Webhook secrets are tenant-scoped.** Multi-user mode keys channel secrets by
  owner, caps each tenant at 256 channels so one tenant cannot evict another's,
  and removes the secret on every unsubscribe path.
- **Policy normalization is consistent.** `allowed_origins` accepts bare hosts,
  `host:port`, and full URLs case-insensitively; `allowed_resources` entries also
  allow dotted descendants; `messages.modify` with the `TRASH`/`SPAM` system
  label is blocked by the `messages.trash` family; `max_request_bytes: 0` fails
  closed at startup.
- **Sheets writes default to `RAW`** with `USER_ENTERED` opt-in; CSV export
  quotes embedded CR/LF and sanitizes formula-leading cells by default.
- **Gmail search is grouped and filtered per hit**; CRLF is rejected in mail
  headers and MIME boundaries are unique per message.
- **`dry_run` is honored only by tools with a real preview**; on other mutating
  tools it is rejected rather than silently ignored, so approval requirements
  still apply and an executed write is never mistaken for a preview.
- **Policy discovery stops at `$HOME` or the first `.git`** and warns instead of
  walking to `/`.
- **`GWS_ALLOWED_HOSTS` extends the transport Host allowlist** for Service DNS
  names.
- **Kubernetes manifests are deployable as shipped, with explicit review
  points**: the ConfigMap uses the real policy schema (`constraints` /
  `denied_methods` / `allowed_resources`, never `folders`/`calendars`), is the
  live startup policy, and defaults to read-only with a `<FOLDER_ID>` placeholder
  that fails closed; the pod passes `--audit-log /dev/stdout`; the namespace
  enforces Pod Security `restricted`; the NetworkPolicy is ingress+egress with
  egress limited to DNS and TCP/443 minus link-local/RFC1918; the image digest is
  an all-zero placeholder that fails closed until replaced; the 0440 Secret
  volume and uid 1001 ownership are documented. Releases publish `SHA256SUMS`
  with a keyless cosign signature and per-arch SBOMs for verification.

### Fixed

- **Helper API failures are reported as errors.** When the underlying Google
  call fails, helper tools now return `isError: true` instead of a successful
  envelope containing an error string, so clients and models cannot treat a
  failed call as success. Audit lines record the upstream status (for example
  `403`) rather than `200`.
- **Denied operations carry a real status.** Policy denials in the audit log now
  include `"status": 403`.
- **`gws://` resource URIs work as advertised.** Templates name the object-id
  parameter (`{id}` for `users.messages`, `{fileId}` for `files`) instead of
  whichever required path parameter the discovery map yielded first, required
  parent parameters are filled with their conventional defaults
  (`userId: "me"`, `calendarId: "primary"`), and `resources/templates/list`
  loads the discovery documents so it returns the full surface on the first
  request instead of an empty list.

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
