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

**IMPORTANT: Generate ALL content as one Markdown string and import it in a SINGLE `gws_docs_import_markdown` call.** Do NOT call the tool multiple times for different sections — that wastes tokens on 15+ round-trips when one call suffices. Compose the full document in Markdown first, then import once.

There are two approaches. Use Option A for most cases; use Option B only when you need precise control over individual elements (e.g., inserting images at specific positions after the document exists).

## Option A: Markdown import (preferred — single call)

Compose the ENTIRE document as one Markdown string, then import in a single call:

```json
{
  "name": "gws_docs_import_markdown",
  "arguments": {
    "markdown": "# {{title|Project Report}}\n\n## Summary\n\nThis report covers **key findings** and *recommendations*.\n\n- Finding one\n- Finding two\n\n1. First action\n2. Second action\n\n> Important note for stakeholders\n\nSee [the dashboard](https://example.com) for details.",
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

Compose the full document — all headings, paragraphs, lists, tables, code blocks — as one Markdown string. One call creates the doc, applies the template, and renders all formatting. If the doc already exists (same title + folder), it is updated in place.

Omit `document_id` to create a new document. The response returns the new document ID.

### Template styling

Pass `template_id` to copy named styles (fonts, colors, heading formats) from an existing Google Doc:

```json
{
  "name": "gws_docs_import_markdown",
  "arguments": {
    "markdown": "# Styled Report\n\nContent here.",
    "title": "{{title|Styled Report}}",
    "template_id": "TEMPLATE_DOC_ID"
  }
}
```

Any Google Doc can serve as a template — its named styles (heading fonts, colors, paragraph spacing) are copied to the new document. Ask the user which document to use as a style reference, or check the policy file for pre-approved template IDs.

## Option B: Individual helper tools

For fine-grained control, use the helper tools one at a time. Each call targets a specific `document_id`.

### Insert text with styling

```json
{
  "name": "gws_docs_insert_text",
  "arguments": {
    "document_id": "DOC_ID",
    "text": "{{title|Project Report}}\n",
    "position": "start",
    "paragraph_style": "HEADING_1"
  }
}
```

Available paragraph styles: `NORMAL_TEXT`, `HEADING_1` through `HEADING_6`, `TITLE`, `SUBTITLE`. Optional styling: `bold`, `italic`, `font_size_pt`, `font_family`, `foreground_color` (hex like `#CC0000`), `background_color`.

### Insert a table

```json
{
  "name": "gws_docs_insert_table",
  "arguments": {
    "document_id": "DOC_ID",
    "rows": 4,
    "columns": 3,
    "position": "end"
  }
}
```

### Insert an image

Use `drive_file_id` to embed an image from Google Drive without needing to share it publicly:

```json
{
  "name": "gws_docs_insert_image",
  "arguments": {
    "document_id": "DOC_ID",
    "drive_file_id": "DRIVE_FILE_ID",
    "position": "end",
    "width_pt": 400,
    "height_pt": 250
  }
}
```

Alternatives: `image_url` for a public URL, or `image_data` (base64) with `image_content_type` for raw image data.

### Format existing text

Apply styling to a character range after insertion:

```json
{
  "name": "gws_docs_format_text",
  "arguments": {
    "document_id": "DOC_ID",
    "start_index": 1,
    "end_index": 15,
    "bold": true,
    "foreground_color": "#CC0000",
    "named_style": "HEADING_2",
    "alignment": "CENTER"
  }
}
```

### Add bullet or numbered lists

```json
{
  "name": "gws_docs_add_bullets",
  "arguments": {
    "document_id": "DOC_ID",
    "start_index": 50,
    "end_index": 120,
    "preset": "BULLET_DISC_CIRCLE_SQUARE"
  }
}
```

Presets include `BULLET_DISC_CIRCLE_SQUARE`, `NUMBERED_DECIMAL_ALPHA_ROMAN`, `BULLET_CHECKBOX`, and others.

## Image workflow

To add images to a document:

1. Upload the image to Google Drive using the `drive` tool with `resource: "files"`, `method: "create"`, and `media_data` (base64-encoded).
2. Insert it into the doc using `gws_docs_insert_image` with the `drive_file_id` from step 1. No public sharing is needed.
