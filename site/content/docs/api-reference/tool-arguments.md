+++
title = "Tool arguments"
description = "Service tool argument reference"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-12T00:00:00+00:00
draft = false
weight = 20
template = "docs/page.html"
[extra]
lead = "All arguments accepted by service tools."
toc = true
top = false
+++

Each Google service is exposed as one MCP tool. All share the same argument schema.

## Arguments

| Argument | Type | Required | Description |
|----------|------|----------|-------------|
| `resource` | string | Yes | API resource (e.g., `files`, `messages`, `events`) |
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

## Example: List Drive files

```json
{
  "name": "drive",
  "arguments": {
    "resource": "files",
    "method": "list",
    "params": {
      "pageSize": 10,
      "fields": "files(id,name,mimeType)",
      "q": "mimeType='application/pdf'"
    }
  }
}
```

## Discovery tool

The `gws_discover` meta-tool introspects the API schema:

```json
{"name": "gws_discover", "arguments": {"service": "drive"}}
{"name": "gws_discover", "arguments": {"service": "drive", "resource": "files"}}
{"name": "gws_discover", "arguments": {"service": "drive", "resource": "files", "method": "list"}}
```
