---
name: doc-to-slides
title: Document to Presentation
description: Turn a Google Doc into a slide deck — each H2 section becomes a slide
arguments:
  - name: document_id
    description: Google Docs document ID to convert
    required: false
  - name: title
    description: Presentation title (defaults to the document's first heading)
    required: false
  - name: template
    description: Presentation template name (list with gws_templates)
    required: false
  - name: folder_id
    description: Drive folder ID for the new presentation
    required: false
---

Turn the document into a presentation.

1. If no document_id was provided, find the document first:

```json
{"name": "gws_drive_list", "arguments": {"query": "<user's search terms>"}}
```

2. Read the document as Markdown:

```json
{"name": "gws_docs_read", "arguments": {"document_id": "{{document_id}}", "format": "markdown"}}
```

3. Restructure the Markdown for slides. The reliable way is to write
Marp source where each slide starts at an H1 or H2 heading and slides
are separated by `---`:

```markdown
# <presentation title>

## <section heading>

- key point
- key point

<!-- optional speaker notes -->

---

## <next section>

...
```

Guidelines:
- One idea per slide; split long sections into multiple slides.
- Turn dense paragraphs into bullets; keep speaker notes in <!-- -->.
- Tables and images pass through to slides.

4. Create the presentation (split_headings also works if you pass the
document Markdown as-is, but restructuring it yourself gives better
decks):

```json
{
  "name": "gws_slides_import_marp",
  "arguments": {
    "title": "{{title}}",
    "template": "{{template}}",
    "folder_id": "{{folder_id}}",
    "marp": "<the Marp Markdown from step 3>"
  }
}
```

Notes:
- Check available templates first with `gws_templates` when the user
  wants a specific look.
- Use dry_run on gws_slides_import_marp to preview the deck.
- If confirm_writes is enabled, the import step will ask for approval.
