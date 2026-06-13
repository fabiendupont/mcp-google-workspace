+++
title = "Policy reference"
description = "All TOML configuration fields and environment variables"
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

The policy file is a TOML file with two sections: `[server]` for global settings and `[[services]]` for per-service configuration.

## `[server]` section

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `read_only` | boolean | `false` | Block all write operations across all services |
| `max_request_bytes` | integer | `16777216` (16 MB) | Maximum request body size in bytes |
| `rate_limit_rpm` | integer | unlimited | Max requests per minute per client IP (HTTP only) |
| `allowed_origins` | list of strings | `[]` (localhost only) | Allowed HTTP Origin hostnames. When set, only listed hostnames are allowed. |
| `credentials_file` | string | — | Path to Google credentials JSON (authorized_user or service_account) |
| `project_id` | string | — | Google Cloud project ID for quota and billing |

```toml
[server]
read_only = false
rate_limit_rpm = 120
allowed_origins = ["internal.corp.com"]
credentials_file = "/path/to/credentials.json"
project_id = "my-project-123456"
```

## `[[services]]` section

Only services listed here are exposed as MCP tools. Everything else is denied.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Service alias (see table below) |
| `read_only` | boolean | inherits from `[server]` | Override global read_only for this service |
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

```toml
[[services]]
name = "gmail"
denied_methods = ["messages.delete", "messages.trash"]

[[services]]
name = "sheets"
read_only = true

[[services]]
name = "docs"
read_only = true
```

## `[[services.folders]]` — Drive folder ACLs

When folders are configured, all Drive operations are constrained to those folders.

| Field | Type | Description |
|-------|------|-------------|
| `id` | string | Folder ID (no API call needed at startup) |
| `path` | string | Human-readable path, resolved to ID at startup. `My Drive/` prefix is optional. |
| `access` | string | `read-only` or `read-write` |

> **Important:** When folder restrictions are configured, write operations without `parents` in the request body are denied. The agent must specify which folder to write to.

```toml
[[services]]
name = "drive"

[[services.folders]]
id = "1ABC-shared-references"
access = "read-only"

[[services.folders]]
path = "Projects/current-project/output"
access = "read-write"
```

## `[[services.calendars]]` — Calendar ACLs

When calendars are configured, operations without a `calendarId` parameter are denied.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `id` | string | required | Calendar ID. Use `primary` for the default calendar. |
| `access` | string | `read-write` | `read-only` or `read-write` |

```toml
[[services]]
name = "calendar"

[[services.calendars]]
id = "primary"
access = "read-write"

[[services.calendars]]
id = "company-holidays@group.calendar.google.com"
access = "read-only"
```

## Environment variables

Policy TOML fields take precedence where both are available.

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
