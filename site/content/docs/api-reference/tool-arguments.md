+++
title = "Tool reference"
description = "Helper tools and generic tool arguments"
date = 2026-06-12T00:00:00+00:00
updated = 2026-07-28T00:00:00+00:00
draft = false
weight = 20
template = "docs/page.html"
[extra]
lead = "59 purpose-built helper tools across 7 services, plus discovery and batch meta-tools."
toc = true
top = false
+++

## Tool discovery

The server operates in two modes:

| Mode | Initial tools | Activation |
|------|---------------|------------|
| **Lazy** (default) | `gws_discover`, `gws_batch` | Model calls `gws_discover(service="sheets")` to activate a service |
| **Eager** (`--eager-tools`) | All 59 tools | Loaded at startup |

Services with helpers (Drive, Docs, Sheets, Slides, Gmail) suppress their generic tool. The model uses only the purpose-built helpers.

## Meta tools

### `gws_discover`

Introspects the API schema and activates services in lazy mode.

```json
{"name": "gws_discover", "arguments": {"service": "drive"}}
{"name": "gws_discover", "arguments": {"service": "drive", "resource": "files"}}
{"name": "gws_discover", "arguments": {"service": "drive", "resource": "files", "method": "list"}}
```

### `gws_batch`

Executes multiple tool calls in a single request.

## Drive helpers (9 tools)

| Tool | Description |
|------|-------------|
| `gws_drive_list` | List files and folders |
| `gws_drive_find_folder` | Find a folder by name |
| `gws_drive_info` | Get file metadata |
| `gws_drive_create_folder` | Create a new folder |
| `gws_drive_copy` | Copy a file |
| `gws_drive_rename` | Rename a file |
| `gws_drive_move` | Move a file to a different folder |
| `gws_drive_share` | Share a file with a user or group |
| `gws_drive_trash` | Move a file to the trash |

### Example: List files

```json
{
  "name": "gws_drive_list",
  "arguments": {
    "folder_id": "folder-abc",
    "page_size": 10
  }
}
```

## Docs helpers (9 tools)

| Tool | Description |
|------|-------------|
| `gws_docs_write` | Create or update a document (accepts Markdown or plain text) |
| `gws_docs_read` | Read document content as Markdown |
| `gws_docs_replace_section` | Replace a section by heading |
| `gws_docs_outline` | Get document outline (headings) |
| `gws_docs_find` | Search for text in a document |
| `gws_docs_insert_table` | Insert a table |
| `gws_docs_insert_image` | Insert an image by URL |
| `gws_docs_read_table` | Read a table as structured data |
| `gws_docs_format` | Apply formatting (bold, italic, color, font) |

### Example: Create a document

```json
{
  "name": "gws_docs_write",
  "arguments": {
    "title": "Meeting Notes",
    "body": "# Meeting Notes\n\n## Agenda\n\n- Item 1\n- Item 2"
  }
}
```

## Sheets helpers (14 tools)

| Tool | Description |
|------|-------------|
| `gws_sheets_read` | Read cell values from a range |
| `gws_sheets_write` | Create or update a spreadsheet |
| `gws_sheets_append` | Append rows to a sheet |
| `gws_sheets_clear` | Clear a range of cells |
| `gws_sheets_info` | Get spreadsheet metadata |
| `gws_sheets_manage_tabs` | Create, rename, delete, or reorder tabs |
| `gws_sheets_trace` | Trace cell dependencies |
| `gws_sheets_explain` | Explain a formula in plain English |
| `gws_sheets_formulas` | Analyze formulas in a range |
| `gws_sheets_format` | Apply formatting (colors, borders, alignment) |
| `gws_sheets_validate` | Add data validation rules |
| `gws_sheets_named_range` | Create or manage named ranges |
| `gws_sheets_csv` | Import or export CSV data |
| `gws_sheets_dimensions` | Resize rows and columns |

### Example: Read a range

```json
{
  "name": "gws_sheets_read",
  "arguments": {
    "spreadsheet_id": "1BxiMVs0XRA...",
    "range": "Sheet1!A1:D10"
  }
}
```

## Slides helpers (9 tools)

| Tool | Description |
|------|-------------|
| `gws_slides_read` | Read presentation content |
| `gws_slides_add` | Add a new slide |
| `gws_slides_update` | Update slide content |
| `gws_slides_duplicate` | Duplicate a slide |
| `gws_slides_delete` | Delete a slide |
| `gws_slides_reorder` | Reorder slides |
| `gws_slides_import_marp` | Import a Marp Markdown presentation |
| `gws_slides_templates` | List and apply slide templates |
| `gws_slides_generate_image` | Generate an image with Gemini and insert it |

### Example: Import a Marp presentation

```json
{
  "name": "gws_slides_import_marp",
  "arguments": {
    "title": "Q3 Review",
    "marp": "---\nmarp: true\n---\n\n# Q3 Review\n\n---\n\n## Revenue\n\n- Up 15% YoY"
  }
}
```

## Gmail helpers (10 tools)

| Tool | Description |
|------|-------------|
| `gws_gmail_search` | Search messages by query |
| `gws_gmail_read` | Read a message (headers, body, attachment list) |
| `gws_gmail_thread` | Read all messages in a thread |
| `gws_gmail_attachment` | Download an attachment or save it to Drive |
| `gws_gmail_contacts` | Look up contacts via People API |
| `gws_gmail_forward` | Forward a message |
| `gws_gmail_draft` | Create a draft (supports Markdown body) |
| `gws_gmail_send` | Send a message or a previously created draft |
| `gws_gmail_reply` | Reply to a message in a thread |
| `gws_gmail_labels` | List, create, or manage labels |

### Example: Search and read

```json
{"name": "gws_gmail_search", "arguments": {"query": "from:alice@example.com after:2026/07/01"}}
{"name": "gws_gmail_read", "arguments": {"message_id": "18a1b2c3d4e5f6"}}
```

### Example: Draft with Markdown

```json
{
  "name": "gws_gmail_draft",
  "arguments": {
    "to": ["bob@example.com"],
    "subject": "Project update",
    "body": "## Status\n\n- Task A: **complete**\n- Task B: in progress",
    "format": "markdown"
  }
}
```

## Calendar helpers (6 tools)

| Tool | Description |
|------|-------------|
| `gws_calendar_list` | List upcoming events (defaults to primary calendar, from now) |
| `gws_calendar_get` | Get full event details |
| `gws_calendar_create` | Create an event with title, time, attendees |
| `gws_calendar_update` | Update an existing event (partial update) |
| `gws_calendar_delete` | Delete (cancel) an event |
| `gws_calendar_freebusy` | Check free/busy times for one or more calendars |

Events include `myStatus` (accepted/declined/tentative) so models can filter declined events from schedule summaries.

### Example: List events

```json
{"name": "gws_calendar_list", "arguments": {"max_results": 5}}
```

### Example: Find free time and create event

```json
{"name": "gws_calendar_freebusy", "arguments": {"time_min": "2026-07-30T08:00:00Z", "time_max": "2026-07-30T18:00:00Z"}}
```

```json
{"name": "gws_calendar_create", "arguments": {"summary": "Team sync", "start": "2026-07-30T14:00:00+02:00", "end": "2026-07-30T14:30:00+02:00"}}
```

## Generic tool (fallback)

For services without helpers (Calendar, Admin, Chat, etc.), a generic tool is available with these arguments:

| Argument | Type | Required | Description |
|----------|------|----------|-------------|
| `resource` | string | Yes | API resource (e.g., `events`, `users`) |
| `method` | string | Yes | API method (e.g., `list`, `get`, `create`) |
| `params` | object | No | Query and path parameters |
| `body` | object | No | Request body (empty `{}` silently dropped) |
| `page_all` | boolean | No | Auto-paginate and return all pages |
| `media_data` | string | No | Base64-encoded file content (up to 10 MB) |
| `media_content_type` | string | No | MIME type (default: `application/octet-stream`) |
| `media_upload_init` | boolean | No | Start resumable upload for files over 10 MB |
| `media_total_size` | integer | No | Total file size for resumable uploads |
| `upload_handle` | string | No | Handle from `media_upload_init` |
| `media_chunk` | string | No | Base64-encoded chunk for resumable uploads |
| `download_handle` | string | No | Handle from large file download |

### Example: List Calendar events

```json
{
  "name": "calendar",
  "arguments": {
    "resource": "events",
    "method": "list",
    "params": {
      "calendarId": "primary",
      "maxResults": 10
    }
  }
}
```

Services with helpers (Drive, Docs, Sheets, Slides, Gmail) block the generic tool. Attempting to call the generic `drive` tool returns an error directing the model to use the `gws_drive_*` helpers instead.
