+++
title = "Policy reference"
description = "All JSON configuration fields and environment variables"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-12T00:00:00+00:00
draft = false
weight = 10
template = "docs/page.html"
[extra]
lead = "Every field in the policy file and environment variable, explained."
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
| `allowed_origins` | list of strings | `[]` (localhost only) | Allowed HTTP Origin hostnames. When set, only listed hostnames are allowed. |
| `credentials_file` | string | — | Path to Google credentials JSON (authorized_user or service_account) |
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

### Access controls per service

Not all services support the same ACL types. This table shows what controls are available for each:

| Service | `read_only` | `denied_methods` | Folder ACLs | Calendar ACLs |
|---------|:-----------:|:-----------------:|:-----------:|:-------------:|
| `drive` | Yes | Yes | Yes | — |
| `gmail` | Yes | Yes | — | — |
| `calendar` | Yes | Yes | — | Yes |
| `sheets` | Yes | Yes | — | — |
| `docs` | Yes | Yes | — | — |
| `slides` | Yes | Yes | — | — |
| `admin` | Yes | Yes | — | — |
| `chat` | Yes | Yes | — | — |

Folder ACLs (`"folders"`) are specific to Drive. Calendar ACLs (`"calendars"`) are specific to Calendar. All other services use `read_only` and `denied_methods` for access control.

### Evaluation order

Every request passes through these checks in order. **All checks must pass** — the first failure stops evaluation and denies the request.

1. **Service allow-list** — Is the service listed in `"services"`? Unlisted services are denied entirely.
2. **Read-only** — Is the service (or server) read-only? If yes, non-GET methods are denied.
3. **Denied methods** — Is the method in the service's `denied_methods` list? Matches against both `method` and `resource.method`.
4. **Resource ACLs** — For Drive: are folder restrictions satisfied? For Calendar: is the calendar ID allowed?

There is no override or exception mechanism — a method denied at step 2 cannot be re-allowed at step 4. This makes policies additive-restrictive: each layer can only narrow access, never widen it.

> **Note:** This policy engine is specific to this MCP server. The upstream `google-workspace` crate (gws CLI) has no policy or permission model — it relies solely on OAuth scopes.

```json
{
  "services": [
    {
      "name": "gmail",
      "denied_methods": ["messages.delete", "messages.trash"]
    },
    {
      "name": "sheets",
      "read_only": true
    },
    {
      "name": "docs",
      "read_only": true
    },
    {
      "name": "slides"
    }
  ]
}
```

## `"folders"` — Drive folder ACLs

When folders are configured, all Drive operations are constrained to those folders.

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Folder ID (no API call needed at startup) |
| `path` | string | Human-readable path, resolved to ID at startup. `My Drive/` prefix is optional. |
| `access` | string | `read-only` or `read-write` |

> **Important:** When folder restrictions are configured, write operations without `parents` in the request body are denied. The agent must specify which folder to write to.

```json
{
  "services": [
    {
      "name": "drive",
      "folders": [
        { "id": "1ABC-shared-references", "access": "read-only" },
        { "path": "Projects/current-project/output", "access": "read-write" }
      ]
    }
  ]
}
```

## `"calendars"` — Calendar ACLs

When calendars are configured, operations without a `calendarId` parameter are denied.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `id` | string | required | Calendar ID. Use `primary` for the default calendar. |
| `access` | string | `read-write` | `read-only` or `read-write` |

```json
{
  "services": [
    {
      "name": "calendar",
      "calendars": [
        { "id": "primary", "access": "read-write" },
        { "id": "company-holidays@group.calendar.google.com", "access": "read-only" }
      ]
    }
  ]
}
```

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
