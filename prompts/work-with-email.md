---
name: work-with-email
description: Full workflow for searching, reading, drafting, and sending Gmail messages
arguments:
  - name: query
    description: Gmail search query to start with
    required: false
  - name: to
    description: Default recipient email address
    required: false
---

## Searching email

Search messages using Gmail search syntax:

```json
{
  "name": "gws_gmail_search",
  "arguments": {
    "query": "{{query|is:unread}}",
    "max_results": 10
  }
}
```

Common query operators: `from:`, `to:`, `subject:`, `is:unread`, `is:starred`, `has:attachment`, `after:2024/01/01`, `label:`.

The response includes message IDs, subjects, senders, dates, and snippets.

## Reading a message

```json
{
  "name": "gws_gmail_read",
  "arguments": { "message_id": "MESSAGE_ID" }
}
```

Returns decoded body text, parsed headers, and attachment list. Use `"format": "metadata"` for headers only.

## Reading a full thread

```json
{
  "name": "gws_gmail_thread",
  "arguments": { "thread_id": "THREAD_ID" }
}
```

Returns all messages in chronological order with decoded bodies (quoted text stripped), headers, and attachments. Use `threadId` from search results or a message read.

## Getting an attachment

Read text content of an attachment:

```json
{
  "name": "gws_gmail_attachment",
  "arguments": { "message_id": "MESSAGE_ID", "attachment_id": "ATTACHMENT_ID" }
}
```

Save attachment directly to Google Drive:

```json
{
  "name": "gws_gmail_attachment",
  "arguments": {
    "message_id": "MESSAGE_ID",
    "attachment_id": "ATTACHMENT_ID",
    "folder_id": "DRIVE_FOLDER_ID"
  }
}
```

Text attachments (txt, csv, json, ics, xml) return decoded content. Binary attachments (pdf, images) return base64 data, or save to Drive with `folder_id`. The `attachmentId` is in the `gws_gmail_read` response.

## Finding a contact's email

```json
{
  "name": "gws_gmail_contacts",
  "arguments": { "query": "Alice" }
}
```

Searches Google Contacts by name prefix. Returns names, email addresses, and organizations. Use this before drafting/sending when you have a name but no email address.

## Creating a draft

```json
{
  "name": "gws_gmail_draft",
  "arguments": {
    "to": "{{to|recipient@example.com}}",
    "subject": "Meeting Follow-up",
    "body": "Hi,\n\nFollowing up on our discussion...",
    "cc": "team@example.com"
  }
}
```

The response returns a `draft_id` that can be sent later.

## Sending email

Send a new message directly:

```json
{
  "name": "gws_gmail_send",
  "arguments": {
    "to": "{{to|recipient@example.com}}",
    "subject": "Quick Update",
    "body": "Here's the update you requested."
  }
}
```

Or send an existing draft:

```json
{
  "name": "gws_gmail_send",
  "arguments": { "draft_id": "DRAFT_ID" }
}
```

## Replying to a message

```json
{
  "name": "gws_gmail_reply",
  "arguments": {
    "message_id": "MESSAGE_ID",
    "body": "Thanks for the update. I'll review and get back to you."
  }
}
```

Sets In-Reply-To and References headers automatically. Uses the original thread.

## Managing labels

List all labels:

```json
{
  "name": "gws_gmail_labels",
  "arguments": { "action": "list" }
}
```

Add a label to a message:

```json
{
  "name": "gws_gmail_labels",
  "arguments": {
    "action": "add",
    "message_id": "MESSAGE_ID",
    "label": "STARRED"
  }
}
```

Remove a label:

```json
{
  "name": "gws_gmail_labels",
  "arguments": {
    "action": "remove",
    "message_id": "MESSAGE_ID",
    "label": "UNREAD"
  }
}
```

System labels: INBOX, SENT, DRAFT, TRASH, SPAM, STARRED, UNREAD, IMPORTANT. Custom labels are matched by name.

## Forwarding a message

```json
{
  "name": "gws_gmail_forward",
  "arguments": {
    "message_id": "MESSAGE_ID",
    "to": "colleague@example.com",
    "comment": "FYI — see the update below."
  }
}
```

Forwards the original message with headers (From, Date, Subject, To) quoted below an optional comment.
