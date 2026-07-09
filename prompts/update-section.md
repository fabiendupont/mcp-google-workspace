---
name: update-section
description: Replace a section in an existing Google Doc
arguments:
  - name: document_id
    description: The Google Docs document ID
    required: true
  - name: section
    description: Heading text of the section to replace
    required: true
---

Use `gws_docs_write` with `document_id` and `section` to replace a specific section in an existing document.

## How it works

The `section` parameter identifies a heading by its exact text. The server finds that heading and replaces everything from it to the next heading of the same or higher level with your new Markdown content.

## Usage

```json
{
  "name": "gws_docs_write",
  "arguments": {
    "document_id": "{{document_id}}",
    "section": "{{section}}",
    "content": "## {{section}}\n\nUpdated content goes here.\n\n- New finding\n- Another finding\n\nSee [updated report](https://example.com) for details."
  }
}
```

## Reading a section first

To inspect the current content before replacing:

```json
{
  "name": "gws_docs_read",
  "arguments": {
    "document_id": "{{document_id}}",
    "section": "{{section}}"
  }
}
```

## Important details

- The `section` value must match the heading text exactly, including case.
- The replacement content should include the heading itself. If the section is `## Engineering`, start your content with `## Engineering`.
- The section spans from the matched heading to the next heading at the same level or higher.
- All other sections are preserved unchanged.
- All Markdown formatting works: `**bold**`, `*italic*`, `` `code` ``, `[links](url)`, bullet lists, numbered lists, tables, and images.
- If the section is not found, the operation returns an error. Use `gws_docs_outline` to see available headings.
