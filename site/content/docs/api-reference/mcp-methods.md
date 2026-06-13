+++
title = "MCP methods"
description = "Supported JSON-RPC methods"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-12T00:00:00+00:00
draft = false
weight = 10
template = "docs/page.html"
[extra]
lead = "Protocol, tool, and task methods."
toc = true
top = false
+++

## Protocol methods

| Method | Description |
|--------|-------------|
| `initialize` | Legacy (2024-11-05) capability handshake |
| `server/discover` | Modern (2026-07-28) capability discovery |
| `ping` | Returns `{}` |

## Tool methods

| Method | Description |
|--------|-------------|
| `tools/list` | List available tools with schemas (supports pagination) |
| `tools/call` | Execute a tool |

## Task methods

| Method | Description |
|--------|-------------|
| `tasks/get` | Get task status by `taskId` |
| `tasks/result` | Get task result |
| `tasks/cancel` | Cancel a running task |
| `tasks/list` | List all tasks |

## Protocol versions

The server supports `2026-07-28`, `2025-11-25`, and `2024-11-05`. It auto-detects the client era from the first request.
