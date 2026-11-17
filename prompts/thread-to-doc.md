---
name: thread-to-doc
title: Email Thread to Document
description: Convert a Gmail thread into a formatted Google Doc, one section per message
arguments:
  - name: thread_id
    description: Gmail thread ID (from gws_gmail_search results)
    required: false
  - name: doc_title
    description: Title for the new document (defaults to 'Thread: <subject>')
    required: false
  - name: folder_id
    description: Drive folder ID for the new document
    required: false
---

Convert the email thread into a well-structured document.

1. If no thread_id was provided, find the thread first:

```json
{"name": "gws_gmail_search", "arguments": {"query": "<user's search terms>", "max_results": 5}}
```

2. Read the whole thread as a conversation:

```json
{"name": "gws_gmail_thread", "arguments": {"thread_id": "THREAD_ID"}}
```

3. Compose a document in Markdown with this structure (adapt it to the
content — add a TL;DR or an action-items section when useful):

```markdown
# Thread: <subject>

## <sender> — <date>

<message body, quotes already stripped>

## <sender> — <date>

...

---

**Participants:** <unique senders>
```

4. Create the document:

```json
{
  "name": "gws_docs_write",
  "arguments": {
    "title": "{{doc_title}}",
    "folder_id": "{{folder_id}}",
    "content": "<the Markdown from step 3>"
  }
}
```

Notes:
- Let the user review the Markdown before writing if they seem unsure.
- If confirm_writes is enabled, the write step will ask for approval.
- Use dry_run on gws_docs_write to preview without creating.
