+++
title = "Quick start"
description = "From zero to working in 5 minutes"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-12T00:00:00+00:00
draft = false
weight = 30
sort_by = "weight"
template = "docs/page.html"

[extra]
lead = "Create a policy, start the server, and make your first API call."
toc = true
top = false
+++

## Prerequisites

- Google credentials configured (see [Credentials](../credentials/))
- A policy file (we create one below)

## 1. Create a policy file

Save this as `policy.toml`:

```toml
[server]
project_id = "your-project-id"

[[services]]
name = "drive"

[[services]]
name = "gmail"
denied_methods = ["messages.delete", "messages.trash"]

[[services]]
name = "calendar"

[[services.calendars]]
id = "primary"
access = "read-write"
```

This enables Drive (full access), Gmail (read and send, no delete), and Calendar (primary calendar only, read-write).

## 2. Start the server

**Stdio (for Claude Code):**

```bash
mcp-google-workspace --policy policy.toml
```

**HTTP (for remote access):**

```bash
mcp-google-workspace --policy policy.toml --http 127.0.0.1:3000
```

**Container:**

```bash
podman run -p 3000:3000 \
  -v ./policy.toml:/etc/mcp-google-workspace/policy.toml:ro,Z \
  -v ./credentials.json:/etc/mcp-google-workspace/credentials.json:ro,Z \
  ghcr.io/fabiendupont/mcp-google-workspace:0.1.0
```

> On Fedora and RHEL with SELinux enabled, the `:Z` flag is required for bind mounts.

## 3. Test with a request

If running in HTTP mode, send a test request:

```bash
curl -s -X POST http://127.0.0.1:3000/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"ping"}' | python3 -m json.tool
```

Expected response:

```json
{
    "id": 1,
    "jsonrpc": "2.0",
    "result": {}
}
```

Then list your Drive files:

```bash
curl -s -X POST http://127.0.0.1:3000/mcp \
  -H 'Content-Type: application/json' \
  -d '{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/call",
    "params": {
      "name": "drive",
      "arguments": {
        "resource": "files",
        "method": "list",
        "params": {"pageSize": 5, "fields": "files(id,name,mimeType)"}
      }
    }
  }' | python3 -m json.tool
```

## 4. Connect Claude Code

Add to `.claude/settings.json`:

```json
{
  "mcpServers": {
    "google-workspace": {
      "command": "/path/to/mcp-google-workspace",
      "args": ["--policy", "/path/to/policy.toml"]
    }
  }
}
```

Claude Code can now use your Google Workspace data through the MCP tools.

## Next steps

- [Policy reference](../../configuration/policy-reference/) — all configuration options
- [Security model](../../security/policy-engine/) — how the policy engine enforces access control
- [Deployment guide](../../deployment/container/) — container and Kubernetes deployment
