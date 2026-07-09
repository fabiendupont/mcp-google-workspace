---
name: create-document
description: Full workflow for creating a formatted Google Doc
arguments:
  - name: title
    description: Document title
    required: false
  - name: folder_id
    description: Drive folder ID to create the document in
    required: false
---

**IMPORTANT: Generate ALL content as one Markdown string and write it in a SINGLE `gws_docs_write` call.** Do NOT call the tool multiple times for different sections — that wastes tokens on 15+ round-trips when one call suffices. Compose the full document in Markdown first, then write once.

## Writing a document (single call)

Compose the ENTIRE document as one Markdown string, then write in a single call:

```json
{
  "name": "gws_docs_write",
  "arguments": {
    "content": "# {{title|Project Report}}\n\n## Summary\n\nThis report covers **key findings** and *recommendations*.\n\n- Finding one\n- Finding two\n\n1. First action\n2. Second action\n\n> Important note for stakeholders\n\nSee [the dashboard](https://example.com) for details.",
    "title": "{{title|Project Report}}",
    "folder_id": "{{folder_id}}"
  }
}
```

Supported Markdown syntax:
- `# Heading 1` through `###### Heading 6` for headings
- `**bold**` and `*italic*` for emphasis
- `` `inline code` `` and fenced code blocks for monospace text
- `[link text](url)` for hyperlinks
- `- item` for bullet lists, `1. item` for numbered lists
- `> text` for blockquotes
- `~~strikethrough~~` for strikethrough
- `| col | col |` pipe tables with `|---|---|` separator — converted to native Google Docs tables with populated cells and bold headers
- `![alt](url)` for inline images

Compose the full document — all headings, paragraphs, lists, tables, code blocks — as one Markdown string. One call creates the doc and renders all formatting. If no `document_id` is provided, the response returns the new document ID.

### Template styling

Pass `template_id` to copy named styles (fonts, colors, heading formats) from an existing Google Doc:

```json
{
  "name": "gws_docs_write",
  "arguments": {
    "content": "# Styled Report\n\nContent here.",
    "title": "{{title|Styled Report}}",
    "template_id": "TEMPLATE_DOC_ID"
  }
}
```

Any Google Doc can serve as a template — its named styles (heading fonts, colors, paragraph spacing) are copied to the new document.

## Reading and inspecting documents

### Get document outline

```json
{
  "name": "gws_docs_outline",
  "arguments": { "document_id": "DOC_ID" }
}
```

Returns a compact structure: headings, tables, images, with character indexes. Use before editing to understand the document layout.

### Read content (full or section)

```json
{
  "name": "gws_docs_read",
  "arguments": { "document_id": "DOC_ID" }
}
```

Returns the full document as Markdown. To read a single section:

```json
{
  "name": "gws_docs_read",
  "arguments": { "document_id": "DOC_ID", "section": "Executive Summary" }
}
```

### Find text positions

```json
{
  "name": "gws_docs_find",
  "arguments": { "document_id": "DOC_ID", "text": "key findings" }
}
```

Returns `startIndex` and `endIndex` for use with `gws_docs_format`.

## Inserting complex objects

### Insert a table from data

```json
{
  "name": "gws_docs_insert_table",
  "arguments": {
    "document_id": "DOC_ID",
    "headers": ["Platform", "GPU Support", "Cost"],
    "rows": [["OpenShift AI", "NVIDIA, AMD", "Subscription"], ["Cloud", "Various", "Pay-per-use"]]
  }
}
```

### Insert an image

Use `drive_file_id` to embed an image from Google Drive without public sharing:

```json
{
  "name": "gws_docs_insert_image",
  "arguments": {
    "document_id": "DOC_ID",
    "drive_file_id": "DRIVE_FILE_ID",
    "width_pt": 400,
    "height_pt": 250
  }
}
```

Alternatives: `image_url` for a public URL, or `image_data` (base64) with `image_content_type`.

### Format existing text

Apply styling to text by searching for it or using character indexes:

```json
{
  "name": "gws_docs_format",
  "arguments": {
    "document_id": "DOC_ID",
    "text": "key findings",
    "bold": true,
    "foreground_color": "#CC0000"
  }
}
```
