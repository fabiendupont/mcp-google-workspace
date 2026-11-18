---
name: validate-before-execute
description: Preview and validate API calls before committing
arguments:
  - name: service
    description: Google service to validate against (e.g., drive, gmail)
    required: false
---

Use these validation features to preview, verify, and debug API calls before they execute.

## Dry run

Set `dry_run: true` to see the HTTP request that would be sent without actually executing it. This is supported for generic REST proxy tools (services without a curated helper set, e.g. `people`), `gws_batch`, `gws_docs_*`, `gws_slides_*`, and `gws_generate_image`:

```json
{
  "name": "{{service|people}}",
  "arguments": {
    "resource": "people",
    "method": "connections",
    "params": { "pageSize": 10 },
    "dry_run": true
  }
}
```

Curated helper tools without a preview (Drive, Sheets, Gmail, Calendar, Tasks) reject `dry_run` on mutating calls instead of pretending to preview them, so you can never mistake an executed write for a preview.

The response shows:
- The full URL that would be called
- The HTTP method (GET, POST, etc.)
- The OAuth scopes required
- The request body (if any)

Use dry run to verify parameter names, URL construction, and scope requirements before committing to a real API call.

## Schema validation

The server validates all parameters against the Google Discovery Document schema before sending any request. This catches errors locally:

- **Invalid enum values**: If a parameter accepts specific values (like `orderBy` or `corpora`), passing an unrecognized value returns an error with the list of valid options.
- **Missing required parameters**: Path parameters like `fileId` are caught before the request is sent.
- **Type mismatches**: Passing a string where an integer is expected is flagged with the expected type.

No configuration is needed. Validation runs automatically on every call.

## Smart field selection

For common operations, the server automatically injects optimized `fields` parameters to reduce response size. For example:
- `drive files.list` returns `id, name, mimeType, modifiedTime` instead of the full file resource.
- `gmail messages.get` returns headers and snippet instead of the raw payload.

You can override this by passing your own `fields` parameter explicitly.

## Policy simulation

Use the `--simulate` CLI flag to test hypothetical operations against a policy file without credentials or network access:

```bash
mcp-google-workspace --policy gws-policy.json --simulate scenarios.json
```

The scenarios file contains an array of operations to test. Each operation is checked against the policy and the result (allowed or denied with reason) is printed. Use this to validate policy changes before deploying them.

## Error recovery

When an API call fails, the response includes structured recovery hints:
- **Retry guidance**: Whether the error is transient and the recommended backoff.
- **Scope suggestions**: If the error is a 403, which OAuth scope is likely missing.
- **Parameter hints**: If the error points to a specific field, what value was expected.

These hints appear in the error response alongside the raw API error, so you can adjust and retry without re-discovering the schema.
