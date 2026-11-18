+++
title = "Tool reference"
description = "Helper tools and generic tool arguments"
date = 2026-06-12T00:00:00+00:00
updated = 2026-11-18T00:00:00+00:00
draft = false
weight = 20
template = "docs/page.html"
[extra]
lead = "Purpose-built helper tools across seven services, plus discovery and batch meta-tools."
toc = true
top = false
+++

## Tool discovery

The server operates in two modes:

| Mode | Initial tools | Activation |
|------|---------------|------------|
| **Lazy** (default) | `gws_discover`, `gws_batch` | Model calls `gws_discover(service="sheets")` to activate a service |
| **Eager** (`--eager-tools`) | All helper and meta tools the policy permits | Loaded at startup; helper tools for services absent from the policy are filtered out, as in lazy mode |

Services with helpers (Drive, Docs, Sheets, Slides, Gmail, Calendar, Tasks)
suppress their generic tool. The model uses only the purpose-built helpers.

## Meta tools

### `gws_discover`

Introspects the API schema and activates services in lazy mode.

```json
{"name": "gws_discover", "arguments": {"service": "drive"}}
{"name": "gws_discover", "arguments": {"service": "drive", "resource": "files"}}
{"name": "gws_discover", "arguments": {"service": "drive", "resource": "files", "method": "list"}}
```

### `gws_batch`

Executes multiple tool calls in a single request. Each entry names a tool and its
arguments, and may carry its own `dry_run` flag. A sub-request that policy allows
but whose payload carries an API error is reported with `"status":"error"`,
excluded from `succeeded`, and counted in `failed`; a per-entry policy failure
rejects the whole batch before anything is executed.

`gws_batch` refuses to dispatch into the helper-only services (Drive, Docs,
Sheets, Slides, Gmail, Calendar, Tasks), even when the policy enables them: the
helpers enforce label, parent, and tool-level policy that the raw REST proxy
bypasses. Call the `gws_<service>_*` tools directly instead; the batch returns an
error naming the required helper.

## Drive helpers

| Tool | Description |
|------|-------------|
| `gws_drive_list` | List files and folders |
| `gws_drive_find_folder` | Find a folder by name |
| `gws_drive_info` | Get file metadata |
| `gws_drive_create_folder` | Create a new folder |
| `gws_drive_copy` | Copy a file |
| `gws_drive_rename` | Rename a file |
| `gws_drive_move` | Move a file to a different folder |
| `gws_drive_share` | Share a file with a user or domain (never link-anyone) |
| `gws_drive_trash` | Move a file to the trash (destructive; recoverable from trash) |

### Example: List files

```json
{
  "name": "gws_drive_list",
  "arguments": {
    "folder_id": "1AbCdEfGhIjKlMnOpQrStUvWxYz01234",
    "page_size": 10
  }
}
```

### Drive IDs and sharing

Drive IDs (`file_id`, `folder_id`, `parent_id`, `to_folder_id`) are validated
before any request: `^[A-Za-z0-9_-]+$`, at least 20 characters, with the literal
`root` alias also accepted. The charset check *is* the injection control, because
the ID is interpolated verbatim into the Drive search grammar; free-text query
values (names, full-text search) are escaped instead (backslash first, then `'`,
control characters stripped). A pasted browser URL
(`https://drive.google.com/file/d/<id>/view`), a share link, or any value
containing `/` or a space is rejected with a validation error rather than being
silently ignored or truncated.

`gws_drive_trash` is treated as **destructive** for policy and client-annotation
purposes (`destructiveHint: true`), even though it is implemented as
`files.update {trashed: true}`. Trashing is recoverable from the Drive trash —
`files.emptyTrash` is the irreversible step — but it is blocked by any denial in
the delete family: if `files.trash`, `files.delete`, or `files.emptyTrash` is
denied, the helper returns a `Trash blocked: ...` policy error and issues no
request. `denied_methods: ["files.delete"]` therefore stops `gws_drive_trash` as
well as `files.delete`.

`gws_drive_share` validates `role` as `reader`, `commenter`, or `writer` before
the API call and rejects `owner`. The permission `type` is derived from the
target — `user` for `email`, `domain` for `domain` — and never taken from the
caller, so `anyone`/`anyoneWithLink` sharing is unreachable through this tool.
Pass either `email` or `domain`. Its `destructiveHint` is false because a share
never transfers ownership.

## Docs helpers

| Tool | Description |
|------|-------------|
| `gws_docs_write` | Create or update a document (accepts Markdown or plain text) |
| `gws_docs_read` | Read document content as Markdown |
| `gws_docs_replace_section` | Replace a section by heading |
| `gws_docs_outline` | Get document outline (headings, tables, images, with indexes) |
| `gws_docs_find` | Search for text in a document and return its character range |
| `gws_docs_insert_text` | Insert text at an index or position |
| `gws_docs_append_section` | Append a heading with a body and/or bullets |
| `gws_docs_add_bullets` | Convert a character range into a bulleted list |
| `gws_docs_insert_table` | Insert a table |
| `gws_docs_insert_image` | Insert an image from a Drive file, URL, or base64 data |
| `gws_docs_read_table` | Read a table as structured data |
| `gws_docs_format` | Apply formatting (bold, italic, color, font) |
| `gws_docs_format_text` | Alias of `gws_docs_format` for range-based styling |

`gws_docs_insert_image` with `drive_file_id` temporarily grants
anyone-with-link read access so the Docs API can fetch the image, then revokes
that permission immediately after insertion. If revocation fails the tool reports
an **error** that names the file id and permission id and gives a remediation
hint, and the file may remain link-accessible until you remove the permission —
it is never reported as success. Prefer `image_data` when temporary sharing is not
acceptable; in `dry_run` no sharing is created.

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

## Sheets helpers

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

Writes default to `value_input_option: "RAW"`, which stores strings verbatim so
a cell beginning with `=` is text, not a formula. Set `USER_ENTERED` only when
you intend Google Sheets to parse formulas and numbers. `gws_sheets_csv` export
defaults to `sanitize_formulas: true`, prefixing cells that start with `=`, `+`,
`-`, `@`, TAB, or CR with a single quote to defuse spreadsheet formula
injection.

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

## Slides helpers

| Tool | Description |
|------|-------------|
| `gws_slides_read` | Read presentation content |
| `gws_slides_add` | Add a new slide |
| `gws_slides_update` | Update slide content |
| `gws_slides_duplicate` | Duplicate a slide |
| `gws_slides_delete` | Delete a slide |
| `gws_slides_reorder` | Reorder slides |
| `gws_slides_import_marp` | Import a Marp Markdown presentation |
| `gws_templates` | List and apply slide templates |
| `gws_generate_image` | Generate an image with Gemini and insert it into a doc or slide |

`gws_generate_image` is a write tool: it appears in `tools/list` with
`readOnlyHint: false` (when the policy permits the `slides` service, which owns
it, as it does for `gws_templates`), is discoverable through `gws_discover`, and
requires the same `confirm_writes` approval as other mutating tools. In
multi-user mode it honours the delegated `_meta.user_email` subject. It temporarily shares the
generated image for insertion and revokes that permission; if the revoke fails it
reports an error naming the file rather than success.

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

## Gmail helpers

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

### Label-restricted thread reads

`gws_gmail_thread` validates **every** message returned against
`allowed_labels` before handing the thread back. Messages outside the allowed
labels are dropped and the result is marked `"partialRead": true` with
`"omittedMessageCount": N`; `messageCount` counts only the messages actually
returned. A thread whose messages all fall outside the policy is therefore an
empty (but explicit) result, not a leak of the filtered ones.

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

`format: "markdown"` is bounded: the body is capped at about 1 MiB in total and
64 KiB per line, and the renderer is linear. Links are restricted to `http`,
`https`, `mailto`, and `tel`; any other scheme renders as plain text, and quoted
content is HTML-escaped.

## Calendar helpers

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

## Tasks helpers

| Tool | Description |
|------|-------------|
| `gws_tasks_lists` | List all task lists |
| `gws_tasks_list` | List tasks in a task list (defaults to the first list) |
| `gws_tasks_get` | Get full details of a single task |
| `gws_tasks_create` | Create a task with title, optional notes and due date |
| `gws_tasks_update` | Update an existing task |
| `gws_tasks_complete` | Mark a task complete |
| `gws_tasks_clear` | Clear completed tasks from a list |
| `gws_tasks_delete` | Delete a task |

## Generic tool (fallback)

For services without helpers (Admin, Chat, etc.), a generic tool is available with these arguments:

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

### Example: List Chat spaces

```json
{
  "name": "chat",
  "arguments": {
    "resource": "spaces",
    "method": "list",
    "params": {
      "pageSize": 10
    }
  }
}
```

Helper-only services (Drive, Docs, Sheets, Slides, Gmail, Calendar, Tasks) block
the generic tool and `gws_batch`. Attempting to call the generic `drive` tool
returns an error directing the model to use the `gws_drive_*` helpers instead.

## Dry run

`dry_run: true` previews a mutating call without performing it. It is honored
only where the tool genuinely implements a preview:

- `gws_docs_*` and `gws_slides_*` helpers
- `gws_generate_image`
- `gws_batch` (per entry, and on the batch call)
- generic REST proxy tools

For every other helper (Drive, Sheets, Gmail, Calendar, Tasks) the flag is not a
preview: on a mutating call it is rejected with an error explaining that the
tool has no preview mode, so the request cannot be mistaken for one that was not
executed. Read-only helpers ignore it. A successful dry run reports the request
it would have sent; no confirmation prompt is raised and no policy mutation is
applied.
