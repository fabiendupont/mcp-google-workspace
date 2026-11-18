+++
title = "Policy reference"
description = "All JSON configuration fields, CLI flags, and environment variables"
date = 2026-06-12T00:00:00+00:00
updated = 2026-11-18T00:00:00+00:00
draft = false
weight = 10
template = "docs/page.html"
[extra]
lead = "Every field in the policy file, CLI flag, and environment variable, explained."
toc = true
top = false
+++

The policy file is a JSON file with two top-level keys: `"server"` for global settings and `"services"` for per-service configuration.

## `"server"` object

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `read_only` | boolean | `false` | Block all write operations across all services |
| `confirm_writes` | boolean | `false` | Require MCP elicitation approval before every mutating operation; `gws_batch` is rejected in this mode |
| `max_request_bytes` | integer | `16777216` (16 MiB) | Maximum HTTP request body size. Must be greater than `0` (a zero value refuses to start); omit the field for the default. Declared oversize gets `413`; chunked oversize is capped by the transport at the boot-time value |
| `rate_limit_rpm` | integer | `600` | Authenticated HTTP request budget, keyed on the presented credential; the pre-authentication per-IP bucket is a separate fixed ~60/min. Also the fallback for outbound per-service budgets. Must be ≥ 1 — `0` is a startup error, not "unlimited" |
| `rate_limits` | object | built-in defaults | **Outbound** per-service overrides (requests per minute), e.g. `{"sheets": 60}` — not the HTTP request budget. Unknown service keys and zero values are startup errors |
| `trusted_proxies` | list of strings | `[]` | CIDRs of reverse proxies whose `X-Forwarded-For` may be trusted for the rate-limit client key. Empty = header ignored, socket peer used. Resolved once at listener start, so a change needs a restart (SIGHUP does not rebuild the middleware). Malformed entries are rejected at load; in multi-user mode the intersection of all users' lists is used |
| `allowed_origins` | list of strings | `[]` (localhost only) | Allowed HTTP `Origin` hosts. Entries may be bare hosts (`internal.corp.com`), `host:port`, or full URLs (`https://internal.corp.com`); matching is case-insensitive on the host. A present `Origin` must match or gets `403`; absent `Origin` is allowed |
| `allowed_subjects` | list of strings | `[]` (delegation disabled) | Addresses a client may request through `_meta.user_email`. Empty rejects all client-requested subjects |
| `delegate_subject` | string | -- | Operator-controlled fallback subject used when the client does not request one |
| `api_token` | string | -- | Bearer token for `/mcp` and `/metrics`; overridden by `--api-token` / `GWS_API_TOKEN` |
| `subscription_poll_secs` | integer | `60` | Polling interval for subscriptions without webhook support |
| `credentials_file` | string | -- | Path to Google credentials JSON |
| `project_id` | string | -- | Google Cloud project ID for quota and billing |

```json
{
  "server": {
    "read_only": false,
    "confirm_writes": false,
    "max_request_bytes": 16777216,
    "rate_limit_rpm": 120,
    "rate_limits": { "sheets": 60 },
    "allowed_origins": ["internal.corp.com"],
    "allowed_subjects": ["agent@example.com"],
    "delegate_subject": "service@example.com",
    "credentials_file": "/path/to/credentials.json",
    "project_id": "my-project-123456"
  }
}
```

The outbound service limiter uses `rate_limits[service]`, then
`rate_limit_rpm`, then the built-in default (Drive 600, Sheets 240, Docs 300,
Slides 300, Gmail 200, Calendar 300). Policy values cap the built-in defaults
rather than being ignored. `rate_limit_rpm` is also the **inbound authenticated**
budget on `/mcp`; `0` is a startup error.

## `"services"` array

Only services listed here are exposed as MCP tools. Everything else is denied.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Service alias (see table below) |
| `read_only` | boolean | inherits from `"server"` | Override global read_only for this service |
| `denied_methods` | list of strings | `[]` | Methods to block. Families move together: `messages.send` also blocks `users.drafts.send`, `messages.delete` also blocks `batchDelete`, and `files.delete` also blocks `files.emptyTrash`. `gws_drive_trash` is destructive for policy purposes (it runs `files.update {trashed:true}`) and checks the whole `files.trash`/`files.delete`/`files.emptyTrash` family, refusing with a "Trash blocked" policy error before any request |
| `allowed_resources` | list of strings | all resources | Restrict resources for tool calls, `gws://` URIs, and subscriptions. Entries also allow dotted descendants (`users.messages` allows `users.messages.attachments`); they never match partial names (`files` does not allow `filesXYZ`) |
| `allowed_labels` | list of strings | all labels | Gmail only: restrict message access to specific labels. Fails closed when no configured label resolves. Search hits are checked individually; thread reads validate **every** returned message, drop non-matching ones and report `partialRead`/`omittedMessageCount`; `messageCount` counts returned messages |
| `constraints` | list of objects | `[]` | Parameter-level access controls (see below) |

Helper tools call a few supporting resources internally. If you use an
`allowed_resources` allowlist, include them: `permissions` for `gws_drive_share`
and Docs image insertion, and `users.labels` for Gmail label resolution
(`allowed_labels` and custom-label tools). System labels (`INBOX`, `TRASH`,
`SPAM`, ...) resolve without a labels call.

### Available services

Any Google Workspace API discoverable through the [Google Discovery Service](https://developers.google.com/discovery) can be used. Common aliases:

| Alias | Google API | Description |
|-------|-----------|-------------|
| `drive` | Google Drive API v3 | Files, folders, permissions, comments |
| `gmail` | Gmail API v1 | Messages, threads, labels, drafts, contacts |
| `calendar` | Google Calendar API v3 | Events, calendars, settings |
| `sheets` | Google Sheets API v4 | Spreadsheets, values, charts |
| `docs` | Google Docs API v1 | Documents, content, styles |
| `slides` | Google Slides API v1 | Presentations, pages, elements |
| `people` | People API v1 | Contacts, profiles, directory |
| `admin` | Admin SDK Directory API | Users, groups, organizational units |
| `chat` | Google Chat API v1 | Spaces, messages, memberships |

### Gmail label policy

The `allowed_labels` field restricts which Gmail labels the agent can access. When set, the server injects a grouped label filter into search queries, validates each returned message against the allowed list, and refuses label changes outside the list. If none of the configured labels resolve against the mailbox, the Gmail helper tools fail closed instead of silently disabling the restriction. For thread reads, messages outside the allowed labels are dropped and the result is marked `"partialRead": true` with `"omittedMessageCount": N`; `messageCount` counts only the messages actually returned.

```json
{
  "services": [
    {
      "name": "gmail",
      "allowed_labels": ["INBOX", "SENT", "IMPORTANT"],
      "denied_methods": ["messages.delete", "messages.trash"]
    }
  ]
}
```

### Per-service rate limits

Override the outbound budget for specific services to respect Google API quotas.
This object lives in the `server` section; policy values take precedence over the
built-in defaults:

```json
{
  "server": {
    "rate_limits": {
      "sheets": 60
    }
  }
}
```

## `"constraints"` -- Parameter-level access controls

Constraints let you restrict which values a parameter can take. Each constraint specifies a parameter name, a list of allowed values, and an access level.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `param` | string | required | Parameter name (from the Google API Discovery Document) |
| `values` | list of strings | required | Allowed values for this parameter |
| `access` | string | `read-write` | `read-only` or `read-write` |
| `location` | string | auto-detect | `body` for request body fields, `body-write-only` to require the field only on writes, or omit for path/query params |
| `mode` | string | `restrict` | `protect` denies writes to listed values without restricting reads |
| `recursive` | boolean | `false` | For Drive `parents`, walk the folder ancestry and enforce by-ID scoping for **reads and writes**. Without it both checks are inactive and startup/`check-policy` warn |

Constraints are generic -- the same mechanism works for any service and any parameter. The server validates constraint values against the actual API call at runtime.

### How constraints work

- **Path/query parameters**: the value provided by the agent must be in the constraint's `values` list. If the constraint is `read-only`, write operations are denied.
- **Body parameters** (with `"location": "body"`): on write operations, the body field must be present and its values must all be in the allowed list. On read/list operations, the server injects a grouped query filter to restrict results.
- **`body-write-only`**: solve the same problem without filtering reads; the field is required on create/copy (so new files cannot land outside the allowed tree) but may be omitted on updates that do not move the resource.
- **`mode: "protect"`**: the listed values are read-only for writes, and Drive `addParents`/`removeParents` cannot move a file into or out of a protected folder.
- **Parent ancestry**: For Drive folder constraints with `"recursive": true`, the server walks parent folders; a file in a subfolder of an allowed folder inherits access. Reads that must resolve to a root: `files.get`, `files.export`, `files.copy` (source), `documents.get`, `spreadsheets.get`, `spreadsheets.values.get`, `presentations.get`. By-ID writes too: `permissions.create`, `files.update`/`delete`, `documents.batchUpdate`, `spreadsheets.values.update`/`append`/`clear`, `spreadsheets.batchUpdate`, `presentations.batchUpdate`. A `parents` constraint no longer wrongly blocks `permissions.create`/`files.update`, which carry no parent of their own. Files with no visible parent fail closed. Set `"recursive": true` — without it the ancestry and by-ID checks are inactive.
- Multiple constraints on the same parameter are merged -- all values across constraints are allowed, and each value inherits the access level of its constraint.

### Examples

**Drive -- restrict to specific folders:**

```json
{
  "services": [
    {
      "name": "drive",
      "constraints": [
        { "param": "parents", "values": ["folder-abc"], "access": "read-only", "location": "body" },
        { "param": "parents", "values": ["folder-xyz"], "access": "read-write", "location": "body" }
      ]
    }
  ]
}
```

**Calendar -- restrict to specific calendars:**

```json
{
  "services": [
    {
      "name": "calendar",
      "constraints": [
        { "param": "calendarId", "values": ["primary"], "access": "read-write" },
        { "param": "calendarId", "values": ["holidays@group.calendar.google.com"], "access": "read-only" }
      ]
    }
  ]
}
```

**Sheets -- restrict to a specific spreadsheet:**

```json
{
  "services": [
    {
      "name": "sheets",
      "constraints": [
        { "param": "spreadsheetId", "values": ["1BxiMVs0XRA..."], "access": "read-only" }
      ]
    }
  ]
}
```

**Slides -- protect templates:**

```json
{
  "services": [
    {
      "name": "slides",
      "constraints": [
        { "param": "presentationId", "values": ["template-id-123"], "access": "read-only" }
      ]
    }
  ]
}
```

**Gmail -- block dangerous methods:**

```json
{
  "services": [
    {
      "name": "gmail",
      "allowed_labels": ["INBOX", "SENT"],
      "denied_methods": [
        "messages.delete", "messages.trash", "messages.batchDelete",
        "settings.updateAutoForwarding",
        "settings.delegates.create",
        "settings.forwardingAddresses.create"
      ]
    }
  ]
}
```

### Evaluation order

Every request passes through these checks in order. **All checks must pass** -- the first failure stops evaluation and denies the request.

1. **Delegation subject** -- Is a client-requested `_meta.user_email` in `server.allowed_subjects`? If not, deny; otherwise fall back to `delegate_subject`.
2. **Service allow-list** -- Is the service listed in `"services"`? Unlisted services are denied entirely.
3. **Resource allowlist** -- When `allowed_resources` is set, is the resource in it? This applies to tool calls, `gws://` resources, and subscriptions.
4. **Read-only** -- Is the service (or server) read-only? If yes, non-GET methods are denied.
5. **Denied methods** -- Is the method (or any method-family alias) in the `denied_methods` list? Suffix matching applies: `"messages.send"` in the denylist matches API method `"users.messages.send"`.
6. **Label policy** -- For Gmail, is the message's label in `allowed_labels`?
7. **Constraints** -- Do the parameter values match the allowed list? Is the access level sufficient? Is a by-ID target (read **or write**) inside a recursive parent root?

There is no override mechanism -- a method denied at step 4 cannot be re-allowed at step 7. Each layer can only narrow access, never widen it.

Every denial includes a `Fix:` hint with the exact JSON snippet to add or change in the policy file.

> **Note:** This policy engine is specific to this MCP server. The upstream `google-workspace` crate (gws CLI) has no policy or permission model -- it relies solely on OAuth scopes.

## CLI flags

### Server

| Flag | Description |
|------|-------------|
| `--policy <path>` | Path to a JSON policy file |
| `--services <list>` | Comma-separated service names; applies safe baseline denials (no constraints) |
| `--http <addr:port>` | Run as HTTP server instead of stdio |
| `--api-token <token>` | Bearer token for `/mcp` and `/metrics`; overrides `GWS_API_TOKEN` and `server.api_token` |
| `--insecure-no-auth` | Explicitly allow a non-loopback HTTP bind without a token (trusted networks only) |
| `--users-dir <path>` | Multi-user HTTP mode: serve each users-directory entry at `/mcp/<user>` |
| `--eager-tools` | Load every helper tool **the policy permits** at startup instead of lazy discovery; helpers for services absent from the policy are filtered out |
| `--compact-schemas` | Emit compact tool schemas to reduce token usage |
| `--prompts-dir <path>` | Directory of Markdown prompt files for MCP prompts |
| `--audit-log <path>` | Write JSONL audit log of all API calls. `/dev/stdout` works in containers (records land in the pod log); the file grows unbounded, so rotate externally |
| `--external-url <url>` | Public URL for Drive webhook callbacks |

### Policy tools

| Flag | Description |
|------|-------------|
| `init` | Guided setup: check auth, pick services, folders, labels, calendars. Refuses to write through a symlink and asks before overwriting an existing file |
| `--template <name>` | Use a preset: `analyst`, `assistant`, `admin-readonly`, `content-writer`, `it-admin`. Use `--template list` for details |
| `check-policy <path>` | Validate a policy file, print security warnings under `Warnings (N):`, and **exit non-zero when any warning is present** so CI can gate on it |
| `--verify` | With `check-policy`: test credentials against Google APIs |

### Templates

| Template | Services | Description |
|----------|----------|-------------|
| `analyst` | Drive (RO), Sheets (RO), Docs (RO), Gmail (RW, no delete/forwarding) | Safe for data analysis. Gmail can read, send, and label, but delete/trash and forwarding/delegation are denied |
| `assistant` | Drive (RW), Gmail (RW, no delete/forwarding), Calendar (RW, primary), Tasks (RW), Sheets (RW), Docs (RO), People (RO) | General-purpose assistant |
| `admin-readonly` | All services | Global read-only, safe for auditing |
| `content-writer` | Drive (RW), Docs (RW), Slides (RW), Sheets (RW), Gmail (RW, no send) | Content production without sending. Denying `messages.send` also blocks `drafts.send` |
| `it-admin` | Drive/Gmail/Sheets/Docs/Slides/People (RO), Calendar (RW) | Administrative troubleshooting; calendar is restricted to the primary calendar |

## Live policy reload

When running with `--http` and `--policy`, the server reloads the policy file on `SIGHUP` without restarting:

```bash
kill -HUP $(pidof mcp-google-workspace)
```

Failed reloads log an error and keep the current policy. Not available in stdio mode.

Token rotation applies immediately: the auth middleware re-reads the policy on
every request, so the old and new `server.api_token` are both accepted. Revocation
differs — the *requirement* to authenticate is latched at boot, so a reload that
removes the only token is a **lockout**: every request to `/mcp` and `/metrics`
returns `401` and an error is logged until a token is restored or the process
restarts. `--api-token`/`GWS_API_TOKEN` are captured at boot.
`server.trusted_proxies` is not reloaded either; changing it needs a restart.

## Audit log

The `--audit-log` flag writes a JSONL file with one entry per API call and one
entry per `tools/call`. `action`/`outcome` are `allowed`, `denied`, or `error`:
every policy refusal is `denied` with `status: 403` and a `reason`; a call the
policy allowed that then failed for another reason is `error` with its real
status and a reason, never `allowed`. A tool-level line uses `resource: "tool"`
and records the tool name in `method`/`tool`:

```json
{"timestamp":"1718745600.000","action":"allowed","outcome":"allowed","service":"drive","resource":"files","method":"list","http_method":"GET","status":200,"duration_ms":142,"tool":"gws_drive_list"}
{"timestamp":"1718745601.000","action":"allowed","outcome":"allowed","service":"gmail","resource":"tool","method":"gws_gmail_send","http_method":"MCP","status":200,"duration_ms":38,"tool":"gws_gmail_send","subject":"alice@example.com"}
{"timestamp":"1718745602.000","action":"denied","outcome":"denied","service":"gmail","resource":"tool","method":"gws_gmail_send","status":403,"reason":"Operation not allowed by policy","subject":"alice@example.com"}
```

- Every `tools/call` produces exactly one tool-level line, including helper tools
  that never reach the generic API executor.
- `status` is the real upstream status for `allowed`/`error` lines and `403` for
  denials; an upstream 4xx/5xx is `error`, never `allowed`.
- `reason` is JSON-escaped, so a newline cannot forge a second audit line.
- `subject` appears when `_meta.user_email` or `server.delegate_subject` was used,
  including rejected client-requested identities, which is what an audit trail
  needs to show.

## Request explanation

Every write operation (POST, PUT, PATCH, DELETE) includes an `_explanation` field in the response's `structuredContent`:

```json
{
  "structuredContent": {
    "_explanation": "Create drive/files.create (POST): name=\"report.pdf\", in folder folder-xyz",
    "id": "...",
    "name": "report.pdf"
  }
}
```

This helps agents surface what happened to the user in plain language.

## Environment variables

Policy JSON fields take precedence where both are available.

### Credentials

| Variable | Description |
|----------|-------------|
| `GOOGLE_WORKSPACE_CLI_TOKEN` | Raw OAuth2 access token (highest priority) |
| `GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE` | Path to credentials JSON |
| `GOOGLE_APPLICATION_CREDENTIALS` | Path to credentials JSON (ADC standard) |

### Quota and project

| Variable | Description |
|----------|-------------|
| `GOOGLE_WORKSPACE_PROJECT_ID` | Google Cloud project ID. Overridden by `project_id` in policy. |

### HTTP transport

| Variable | Description |
|----------|-------------|
| `GWS_API_TOKEN` | Bearer token for `/mcp` and `/metrics`; used when `--api-token` is not passed and overrides `server.api_token` |
| `GWS_ALLOWED_HOSTS` | Comma-separated extra hosts accepted in the `Host` header (for example Kubernetes Service DNS names) |

### Observability

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Log level filter (e.g., `info`, `debug`, `mcp_google_workspace=debug`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP endpoint for trace export (e.g., `http://localhost:4318`) |
