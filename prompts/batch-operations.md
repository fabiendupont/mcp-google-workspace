---
name: batch-operations
description: When and how to use batch operations for bulk API calls
arguments:
  - name: service
    description: Google service to batch against (e.g., drive, gmail)
    required: false
---

Use `gws_batch` to execute multiple Google API calls in a single request. This is useful for bulk operations like sharing files, updating metadata, or reading multiple resources at once.

## Usage

```json
{
  "name": "gws_batch",
  "arguments": {
    "service": "{{service|drive}}",
    "requests": [
      {
        "resource": "files",
        "method": "get",
        "params": { "fileId": "FILE_ID_1", "fields": "id,name,mimeType" }
      },
      {
        "resource": "files",
        "method": "get",
        "params": { "fileId": "FILE_ID_2", "fields": "id,name,mimeType" }
      }
    ]
  }
}
```

## How it works

- Up to 100 sub-requests per batch.
- All sub-requests are validated against the policy BEFORE any are executed. If any single sub-request violates the policy, the entire batch is rejected.
- Each sub-request requires `resource` and `method`. The `params` and `body` fields are optional.
- Results include per-request status codes and responses, plus a summary showing total, succeeded, and failed counts.

## When to use batch vs. sequential calls

Use `gws_batch` when:
- Sharing a file with multiple users (multiple `permissions.create` calls)
- Reading metadata for several files at once
- Updating labels or properties on many files
- Sending multiple Gmail label changes

Use sequential calls when:
- Operations depend on each other (e.g., create a file, then set its permissions)
- You need to inspect one result before deciding the next call

## When to use gws_docs_import_markdown instead

For document content operations, prefer `gws_docs_import_markdown` over batching raw `documents.batchUpdate` requests. The Markdown tool handles character index arithmetic, paragraph styles, and text formatting automatically. Batching is better for non-content operations like bulk file sharing or metadata updates.

## Example: share a file with three users

```json
{
  "name": "gws_batch",
  "arguments": {
    "service": "drive",
    "requests": [
      {
        "resource": "permissions",
        "method": "create",
        "params": { "fileId": "FILE_ID" },
        "body": { "role": "reader", "type": "user", "emailAddress": "alice@example.com" }
      },
      {
        "resource": "permissions",
        "method": "create",
        "params": { "fileId": "FILE_ID" },
        "body": { "role": "reader", "type": "user", "emailAddress": "bob@example.com" }
      },
      {
        "resource": "permissions",
        "method": "create",
        "params": { "fileId": "FILE_ID" },
        "body": { "role": "commenter", "type": "user", "emailAddress": "carol@example.com" }
      }
    ]
  }
}
```
