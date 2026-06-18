+++
title = "Policy reference"
description = "All JSON configuration fields, CLI flags, and environment variables"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-18T00:00:00+00:00
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
| `max_request_bytes` | integer | `16777216` (16 MB) | Maximum request body size in bytes |
| `rate_limit_rpm` | integer | unlimited | Max requests per minute per client IP (HTTP only) |
| `allowed_origins` | list of strings | `[]` (localhost only) | Allowed HTTP Origin hostnames |
| `credentials_file` | string | — | Path to Google credentials JSON |
| `project_id` | string | — | Google Cloud project ID for quota and billing |

```json
{
  "server": {
    "read_only": false,
    "rate_limit_rpm": 120,
    "allowed_origins": ["internal.corp.com"],
    "credentials_file": "/path/to/credentials.json",
    "project_id": "my-project-123456"
  }
}
```

## `"services"` array

Only services listed here are exposed as MCP tools. Everything else is denied.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Service alias (see table below) |
| `read_only` | boolean | inherits from `"server"` | Override global read_only for this service |
| `denied_methods` | list of strings | `[]` | Methods to block (e.g., `messages.delete`) |
| `constraints` | list of objects | `[]` | Parameter-level access controls (see below) |

### Available services

Any Google Workspace API discoverable through the [Google Discovery Service](https://developers.google.com/discovery) can be used. Common aliases:

| Alias | Google API | Description |
|-------|-----------|-------------|
| `drive` | Google Drive API v3 | Files, folders, permissions, comments |
| `gmail` | Gmail API v1 | Messages, threads, labels, drafts |
| `calendar` | Google Calendar API v3 | Events, calendars, settings |
| `sheets` | Google Sheets API v4 | Spreadsheets, values, charts |
| `docs` | Google Docs API v1 | Documents, content, styles |
| `slides` | Google Slides API v1 | Presentations, pages, elements |
| `admin` | Admin SDK Directory API | Users, groups, organizational units |
| `chat` | Google Chat API v1 | Spaces, messages, memberships |

## `"constraints"` — Parameter-level access controls

Constraints let you restrict which values a parameter can take. Each constraint specifies a parameter name, a list of allowed values, and an access level.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `param` | string | required | Parameter name (from the Google API Discovery Document) |
| `values` | list of strings | required | Allowed values for this parameter |
| `access` | string | `read-write` | `read-only` or `read-write` |
| `location` | string | auto-detect | `body` for request body fields, omit for path/query params |

Constraints are generic — the same mechanism works for any service and any parameter. The server validates constraint values against the actual API call at runtime.

### How constraints work

- **Path/query parameters**: the value provided by the agent must be in the constraint's `values` list. If the constraint is `read-only`, write operations are denied.
- **Body parameters** (with `"location": "body"`): on write operations, the body field must be present and its values must all be in the allowed list. On read/list operations, the server injects a query filter to restrict results.
- Multiple constraints on the same parameter are merged — all values across constraints are allowed, and each value inherits the access level of its constraint.

### Examples

**Drive — restrict to specific folders:**

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

**Calendar — restrict to specific calendars:**

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

**Sheets — restrict to a specific spreadsheet:**

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

**Gmail — block dangerous methods:**

```json
{
  "services": [
    {
      "name": "gmail",
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

Every request passes through these checks in order. **All checks must pass** — the first failure stops evaluation and denies the request.

1. **Service allow-list** — Is the service listed in `"services"`? Unlisted services are denied entirely.
2. **Read-only** — Is the service (or server) read-only? If yes, non-GET methods are denied.
3. **Denied methods** — Is the method in the `denied_methods` list?
4. **Constraints** — Do the parameter values match the allowed list? Is the access level sufficient for the operation?

There is no override mechanism — a method denied at step 2 cannot be re-allowed at step 4. Each layer can only narrow access, never widen it.

Every denial includes a `Fix:` hint with the exact JSON snippet to add or change in the policy file.

> **Note:** This policy engine is specific to this MCP server. The upstream `google-workspace` crate (gws CLI) has no policy or permission model — it relies solely on OAuth scopes.

## CLI flags

### Server

| Flag | Description |
|------|-------------|
| `--policy <path>` | Path to a JSON policy file |
| `--services <list>` | Comma-separated service names (no constraints) |
| `--http <addr:port>` | Run as HTTP server instead of stdio |
| `--audit-log <path>` | Write JSONL audit log of all API calls |

### Policy tools

| Flag | Description |
|------|-------------|
| `--init-policy` | Interactive policy wizard (or use with `--services` for quick generation) |
| `--template <name>` | Use a preset: `analyst`, `assistant`, `admin-readonly`. Use `--template list` for details |
| `--check-policy <path>` | Validate a policy file and show security warnings |
| `--verify` | With `--check-policy`: test credentials against Google APIs |

### Templates

| Template | Services | Description |
|----------|----------|-------------|
| `analyst` | Drive (RO), Sheets (RO), Docs (RO), Gmail (send-only) | Safe for data analysis |
| `assistant` | Drive, Gmail (safe), Calendar (primary), Sheets, Docs (RO) | General-purpose assistant |
| `admin-readonly` | All services | Global read-only, safe for auditing |

## Live policy reload

When running with `--http` and `--policy`, the server reloads the policy file on `SIGHUP` without restarting:

```bash
kill -HUP $(pidof mcp-google-workspace)
```

Failed reloads log an error and keep the current policy. Not available in stdio mode.

## Audit log

The `--audit-log` flag writes a JSONL file with one entry per API call:

```json
{"timestamp":"1718745600.000","action":"allowed","service":"drive","resource":"files","method":"list","http_method":"GET","status":200,"duration_ms":142}
{"timestamp":"1718745601.000","action":"denied","service":"docs","resource":"documents","method":"create","reason":"Service 'docs' is read-only"}
```

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

### Observability

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Log level filter (e.g., `info`, `debug`, `mcp_google_workspace=debug`) |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP endpoint for trace export (e.g., `http://localhost:4318`) |
