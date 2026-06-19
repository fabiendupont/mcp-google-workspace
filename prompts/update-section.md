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

Use `gws_docs_import_markdown` with the `document_id` and `section` parameters to replace a specific section in an existing document.

## How it works

The `section` parameter identifies a heading in the document by its exact text. The server finds that heading and replaces everything from it to the next heading of the same or higher level with your new Markdown content.

## Usage

```json
{
  "name": "gws_docs_import_markdown",
  "arguments": {
    "document_id": "{{document_id}}",
    "section": "{{section}}",
    "markdown": "## {{section}}\n\nUpdated content goes here.\n\n- New finding\n- Another finding\n\nSee [updated report](https://example.com) for details."
  }
}
```

## Important details

- The `section` value must match the heading text exactly, including case. `"Engineering"` will not match a heading that says `"engineering"`.
- The replacement Markdown should include the heading itself. If the section heading is `## Engineering`, start your Markdown with `## Engineering`.
- The section spans from the matched heading to the next heading at the same level or higher. For example, if you target `## Engineering`, the section ends at the next `##` or `#` heading.
- All other sections in the document are preserved unchanged.
- All Markdown formatting works in the replacement: `**bold**`, `*italic*`, `` `code` ``, `[links](url)`, bullet lists, numbered lists, blockquotes, and images.
- If the section is not found, the operation returns an error with the heading text that was searched for.
