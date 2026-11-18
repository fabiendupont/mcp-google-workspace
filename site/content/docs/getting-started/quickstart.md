+++
title = "Quick start"
description = "From zero to working in 5 minutes"
date = 2026-06-12T00:00:00+00:00
updated = 2026-11-18T00:00:00+00:00
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
mcp-google-workspace init --template assistant > policy.json
```

Or run the interactive wizard:

```bash
mcp-google-workspace init
```

The wizard writes the policy to the current directory: it refuses to write
through a symlink and asks before overwriting an existing file (answer `n` to
abort and keep the old policy). The `assistant` template grants Docs read-only;
`init --template list` describes every preset.

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
mcp-google-workspace check-policy policy.json
```

This shows a summary of services, constraints, and security warnings for risky configurations. It **exits non-zero when warnings are present**, so you can gate CI on it. Warnings cover a `parents` constraint without `"recursive": true` (by-ID scoping inactive), a non-read-only service with no constraints, an unconstrained `calendar`, and Gmail settings/forwarding methods that are not denied.

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

In eager mode, every helper tool **the policy permits** is loaded at startup;
helpers for services absent from the policy are filtered out, exactly as in lazy
mode.

**HTTP (for remote access):**

```bash
export GWS_API_TOKEN="$(openssl rand -hex 32)"

mcp-google-workspace --policy policy.json --http 127.0.0.1:3000 \
  --api-token "$GWS_API_TOKEN" --eager-tools
```

On a loopback bind a token is optional, but any non-loopback bind (`0.0.0.0`,
a pod IP, a hostname) refuses to start without one. Clients must send
`Authorization: Bearer $GWS_API_TOKEN` to `/mcp` and `/metrics`.

**Container:**

```bash
podman run -p 3000:3000 \
  -e GWS_API_TOKEN="$GWS_API_TOKEN" \
  -v ./policy.json:/etc/mcp-google-workspace/policy.json:ro,Z \
  -v ./credentials.json:/etc/mcp-google-workspace/credentials.json:ro,Z \
  ghcr.io/fabiendupont/mcp-google-workspace:0.9.0
```

> On Fedora and RHEL with SELinux enabled, the `:Z` flag is required for bind mounts.

## 4. Test with a request

If running in HTTP mode, send a test request:

```bash
curl -s -X POST http://127.0.0.1:3000/mcp \
  -H "Authorization: Bearer $GWS_API_TOKEN" \
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
  -H "Authorization: Bearer $GWS_API_TOKEN" \
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
| Eager (`--eager-tools`) | All helper tools the policy permits | Loaded at startup | Claude Code, full-featured agents |

In lazy mode, services with helpers (Drive, Docs, Sheets, Slides, Gmail) suppress their generic tool. The model uses only the purpose-built helpers.

## Next steps

- [Policy reference](../../configuration/policy-reference/) -- all configuration options, constraints, templates, and CLI flags
- [Security model](../../security/model/) -- how the policy engine enforces access control
- [Deployment guide](../../deployment/guide/) -- container and Kubernetes deployment
