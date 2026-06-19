---
name: explore-api
description: Explore Google API schemas using the gws_discover tool
arguments:
  - name: service
    description: Google service to explore (e.g., drive, gmail, calendar)
    required: false
---

Use `gws_discover` to explore the schema of any enabled Google API before making calls.

## Step 1: List resources

Call `gws_discover` with just the `service` argument to see all available resources:

```json
{ "name": "gws_discover", "arguments": { "service": "{{service|drive}}" } }
```

The response lists every resource (e.g., files, permissions, comments) and its methods.

## Step 2: List methods on a resource

Add the `resource` argument to see what methods a resource supports:

```json
{ "name": "gws_discover", "arguments": { "service": "{{service|drive}}", "resource": "files" } }
```

Each method shows its HTTP verb (GET, POST, etc.) and a short description. Sub-resources like `files.revisions` are also listed here.

## Step 3: Get full parameter schema

Add the `method` argument to get the complete parameter schema for a specific method:

```json
{ "name": "gws_discover", "arguments": { "service": "{{service|drive}}", "resource": "files", "method": "list" } }
```

The response includes every parameter with its type, whether it is required, its location (path or query), and a description. It also shows whether the method supports media upload or download.

## Example: exploring Drive files.list

1. `gws_discover` with `service: "drive"` reveals resources including `files`, `permissions`, `drives`, `comments`.
2. `gws_discover` with `service: "drive", resource: "files"` reveals methods: `list`, `get`, `create`, `update`, `delete`, `copy`, `export`.
3. `gws_discover` with `service: "drive", resource: "files", method: "list"` reveals parameters like `q` (search query), `fields` (response fields), `pageSize`, `orderBy`, `corpora`, and more.

Use this workflow before any unfamiliar API call to understand the exact parameter names, types, and constraints.
