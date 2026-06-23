---
name: create-presentation
description: Full workflow for creating a Google Slides presentation from Marp Markdown
arguments:
  - name: title
    description: Presentation title
    required: false
  - name: folder_id
    description: Drive folder ID to create the presentation in
    required: false
  - name: template
    description: Presentation ID to copy as a template (slide masters and layouts)
    required: false
---

**IMPORTANT: Compose the ENTIRE presentation as one Marp Markdown string and import it in a SINGLE `gws_slides_import_marp` call.** Do not call the tool multiple times for different slides.

## Marp Markdown syntax

Marp is a Markdown-based presentation format. Key syntax:

- `---` on its own line separates slides
- `# Heading` on the first line of a slide becomes the slide title
- Body text supports **bold**, *italic*, `inline code`, and [links](url)
- `- item` for bullet lists, `1. item` for numbered lists
- ` ```language ... ``` ` for code blocks (rendered in Courier New)
- `![](url)` for inline images with optional sizing: `![w:200 h:150](url)`
- `![bg](url)` for full-slide background images
- `<!-- notes -->` followed by text for speaker notes (not visible in presentation, only in presenter view)
- `<!-- _backgroundColor: #hex -->` for per-slide background color
- `<!-- backgroundColor: #hex -->` for global background color

### YAML frontmatter (optional)

```
---
marp: true
theme: default
paginate: true
backgroundColor: "#ffffff"
color: "#333333"
---
```

## Basic example

```json
{
  "name": "gws_slides_import_marp",
  "arguments": {
    "marp": "---\nmarp: true\npaginate: true\n---\n\n# {{title|Project Overview}}\n\nTeam presentation — Q3 2026\n\n---\n\n# Agenda\n\n- Background\n- Key findings\n- Recommendations\n- Next steps\n\n---\n\n# Key Findings\n\nWe discovered **three critical issues**:\n\n1. Performance degradation in the API layer\n2. Missing error handling in auth flow\n3. Stale cache invalidation\n\n```python\ndef fix_cache():\n    cache.invalidate_all()\n```\n\n<!-- notes -->\nRemember to demo the cache fix live\n\n---\n\n<!-- _backgroundColor: #1a1a2e -->\n\n# Questions?\n\nThank you for your time.",
    "title": "{{title|Project Overview}}",
    "folder_id": "{{folder_id}}"
  }
}
```

## Using a Red Hat template

Pass `template` to copy an existing presentation as the base. The template's slide masters and layouts are preserved, giving your content the Red Hat brand look:

```json
{
  "name": "gws_slides_import_marp",
  "arguments": {
    "marp": "# My Presentation\n\nContent here.\n\n---\n\n# Slide 2\n\nMore content.",
    "title": "{{title|My Presentation}}",
    "template": "{{template}}"
  }
}
```

Any Google Slides presentation can serve as a template — its slide masters, layouts, and color scheme are preserved in the copy. Ask the user which presentation to use as a style reference, or check the policy file for pre-approved template IDs.

Templates should be listed in the policy file with `"mode": "protect"` and `"access": "read-only"` to prevent accidental modification while allowing reads and copies. See `policy.example.json` for the pattern.

## Create-or-update semantics

- Omit `presentation_id` and provide `title` to create a new presentation (or update an existing one with the same title in the same folder).
- Provide `presentation_id` to replace all slides in an existing presentation.
- The response includes the `presentation_id` and a `url` to open it in Google Slides.

## Image workflow

To include images in slides:

- **Public URL**: Use `![](https://example.com/image.png)` directly in Marp Markdown.
- **Background**: Use `![bg](https://example.com/bg.jpg)` for full-slide backgrounds.
- **Generated images**: Use `gws_generate_image` to create an image from a text prompt, then reference it by URL or insert it into the presentation directly.
