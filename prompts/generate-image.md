---
name: generate-image
description: Generate an image with Gemini and optionally insert it into a Google Doc or Slides
arguments:
  - name: prompt
    description: Text description of the image to generate
    required: true
  - name: document_id
    description: Google Doc ID to insert the image into
    required: false
  - name: presentation_id
    description: Google Slides presentation ID to insert the image into
    required: false
---

Use `gws_generate_image` to generate images from text prompts using Google Gemini, and optionally embed them directly into Google Docs or Slides.

## Standalone image generation

Generate an image and return it as base64 data:

```json
{
  "name": "gws_generate_image",
  "arguments": {
    "prompt": "{{prompt}}",
    "aspect_ratio": "16:9",
    "image_size": "2K"
  }
}
```

Options:
- `aspect_ratio`: `1:1`, `3:4`, `4:3`, `9:16`, `16:9` (default: square)
- `image_size`: `1K` or `2K` (default: `1K`)
- `model`: defaults to `gemini-2.5-flash-image`

## Insert into a Google Doc

Generate and embed in one call:

```json
{
  "name": "gws_generate_image",
  "arguments": {
    "prompt": "{{prompt}}",
    "document_id": "{{document_id}}",
    "position": "end",
    "width_pt": 400,
    "height_pt": 250
  }
}
```

The image is generated, then inserted at the specified position using the same pipeline as `gws_docs_insert_image`.

## Insert into a Google Slides presentation

Generate and place on a specific slide:

```json
{
  "name": "gws_generate_image",
  "arguments": {
    "prompt": "{{prompt}}",
    "presentation_id": "{{presentation_id}}",
    "slide_object_id": "slide_0",
    "width_pt": 400,
    "height_pt": 300
  }
}
```

The image is uploaded to Google Drive, shared publicly (the presentation caches it), and inserted via `createImage`.

## Authentication

The Gemini API requires one of:
- `GEMINI_API_KEY` environment variable (simplest)
- OAuth2 credentials with the `generative-language` scope (uses the existing credential chain)

If neither is configured, the tool returns a clear error explaining the options.

## Writing good prompts

Be specific and descriptive:
- **Good**: "A clean, modern architecture diagram showing three microservices connected by arrows, flat design, white background, no text labels"
- **Bad**: "architecture diagram"

For technical illustrations, specify style (flat, isometric, hand-drawn), colors, and whether to include text.
