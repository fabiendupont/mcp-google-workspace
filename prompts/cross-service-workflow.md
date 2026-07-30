---
name: cross-service-workflow
description: Multi-service workflow combining Gmail, Docs, Sheets, and Calendar
arguments:
  - name: folder_id
    description: Drive folder ID for created documents
    required: false
---

## Email → Document summary

1. Search and read the email:

```json
{"name": "gws_gmail_search", "arguments": {"query": "from:alice", "max_results": 1}}
```

```json
{"name": "gws_gmail_read", "arguments": {"message_id": "MSG_ID"}}
```

2. Create a summary doc in Drive:

```json
{
  "name": "gws_docs_write",
  "arguments": {
    "title": "Summary: [subject]",
    "folder_id": "{{folder_id}}",
    "content": "# Email Summary\n\n**From:** ...\n**Date:** ...\n\n## Key Points\n\n- ..."
  }
}
```

## Email → Spreadsheet log

1. Read the email (as above)
2. Append a row to a tracking spreadsheet:

```json
{
  "name": "gws_sheets_append",
  "arguments": {
    "spreadsheet_id": "SPREADSHEET_ID",
    "data": [["2026-07-29", "alice@example.com", "Subject line", "Action needed"]]
  }
}
```

## Email → Calendar follow-up

1. Read the email
2. Check free time:

```json
{"name": "gws_calendar_freebusy", "arguments": {"time_min": "2026-07-30T08:00:00Z", "time_max": "2026-07-30T18:00:00Z"}}
```

3. Create a follow-up meeting in a free slot:

```json
{
  "name": "gws_calendar_create",
  "arguments": {
    "summary": "Follow-up: [subject]",
    "start": "2026-07-30T14:00:00+02:00",
    "end": "2026-07-30T14:30:00+02:00"
  }
}
```

## Contact → Draft email

1. Look up the recipient:

```json
{"name": "gws_gmail_contacts", "arguments": {"query": "Alice"}}
```

2. Draft the email:

```json
{
  "name": "gws_gmail_draft",
  "arguments": {
    "to": "alice@example.com",
    "subject": "Re: Project update",
    "body": "## Summary\n\n- Item 1\n- Item 2",
    "format": "markdown"
  }
}
```

## Tips

- Use `gws_gmail_thread` to read full conversation context before replying
- Use `gws_gmail_attachment` with `folder_id` to save attachments to Drive
- Check `myStatus` on calendar events to filter out declined meetings
- Use `gws_docs_write` with `format: "markdown"` for formatted documents
