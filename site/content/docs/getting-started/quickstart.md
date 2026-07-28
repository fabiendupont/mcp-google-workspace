+++
title = "Quick start"
description = "From zero to working in 5 minutes"
date = 2026-06-12T00:00:00+00:00
updated = 2026-07-28T00:00:00+00:00
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
- The binary installed (see [Installation](../installation/))

## 1. Create a policy file

The fastest way is to use a template:

```bash
mcp-google-workspace --init-policy --template assistant > policy.json
```

Or run the interactive wizard:

```bash
mcp-google-workspace --init-policy
```

Or create `policy.json` manually:

```json
{
  "server": {
    "project_id": "your-project-id"
  },
  "services": [
    { "name": "drive" },
    {
      "name": "gmail",
      "allowed_labels": ["INBOX", "SENT"],
      "denied_methods": ["messages.delete", "messages.trash",
        "settings.updateAutoForwarding", "settings.delegates.create",
        "settings.forwardingAddresses.create"]
    },
    {
      "name": "calendar",
      "constraints": [
        { "param": "calendarId", "values": ["primary"], "access": "read-write" }
      ]
    }
  ]
}
```

## 2. Validate the policy

```bash
mcp-google-workspace --check-policy policy.json
```

This shows a summary of services, constraints, and security warnings for risky configurations.

## 3. Start the server

**Stdio with lazy discovery (default):**

```bash
mcp-google-workspace --policy policy.json
```

In lazy mode, only `gws_discover` and `gws_batch` are visible initially. When the model calls `gws_discover(service="drive")`, the Drive helper tools are activated and a `ToolListChangedNotification` is sent.

**Stdio with eager tools (all tools at startup):**

```bash
mcp-google-workspace --policy policy.json --eager-tools
```

In eager mode, all 53 helper tools are loaded at startup.

**HTTP (for remote access):**

```bash
mcp-google-workspace --policy policy.json --http 127.0.0.1:3000 --eager-tools
```

**Container:**

```bash
podman run -p 3000:3000 \
  -v ./policy.json:/etc/mcp-google-workspace/policy.json:ro,Z \
  -v ./credentials.json:/etc/mcp-google-workspace/credentials.json:ro,Z \
  ghcr.io/fabiendupont/mcp-google-workspace:0.7.0
```

> On Fedora and RHEL with SELinux enabled, the `:Z` flag is required for bind mounts.

## 4. Test with a request

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

Then list your Drive files using the helper tool:

```bash
curl -s -X POST http://127.0.0.1:3000/mcp \
  -H 'Content-Type: application/json' \
  -d '{
    "jsonrpc": "2.0",
    "id": 2,
    "method": "tools/call",
    "params": {
      "name": "gws_drive_list",
      "arguments": {
        "page_size": 5
      }
    }
  }' | python3 -m json.tool
```

## 5. Connect Claude Code

Add to `.claude/settings.json`:

```json
{
  "mcpServers": {
    "google-workspace": {
      "command": "/path/to/mcp-google-workspace",
      "args": ["--policy", "/path/to/policy.json", "--eager-tools"]
    }
  }
}
```

With `--eager-tools`, all helper tools are available to Claude immediately. Without it, Claude uses `gws_discover` to activate services on demand.

## Lazy vs eager tool discovery

| Mode | Initial tools | How services activate | Best for |
|------|---------------|----------------------|----------|
| Lazy (default) | `gws_discover`, `gws_batch` | Model calls `gws_discover(service="sheets")` | Small-context models, selective usage |
| Eager (`--eager-tools`) | All 53 tools | Loaded at startup | Claude Code, full-featured agents |

In lazy mode, services with helpers (Drive, Docs, Sheets, Slides, Gmail) suppress their generic tool. The model uses only the purpose-built helpers.

## Next steps

- [Policy reference](../../configuration/policy-reference/) -- all configuration options, constraints, templates, and CLI flags
- [Security model](../../security/policy-engine/) -- how the policy engine enforces access control
- [Deployment guide](../../deployment/container/) -- container and Kubernetes deployment
