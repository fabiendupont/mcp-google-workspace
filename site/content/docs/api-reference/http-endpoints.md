+++
title = "HTTP endpoints"
description = "HTTP endpoint reference"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-18T00:00:00+00:00
draft = false
weight = 30
template = "docs/page.html"
[extra]
lead = "MCP, probes, and metrics endpoints."
toc = true
top = false
+++

## MCP

| Method | Path | Content-Type | Description |
|--------|------|-------------|-------------|
| `POST /mcp` | `/mcp` | `application/json` | MCP JSON-RPC endpoint (synchronous response) |
| `POST /mcp` | `/mcp` | `text/event-stream` | MCP JSON-RPC with SSE streaming (when `Accept: text/event-stream`) |
| `GET /mcp` | `/mcp` | `text/event-stream` | SSE notification stream (requires `Accept: text/event-stream`) |

### Streamable HTTP

When the client sends `Accept: text/event-stream` on a POST request, the server returns an SSE stream instead of a single JSON response. Notifications are sent inline as `event: notification` messages, followed by the final result as `event: message`:

```
event: notification
data: {"jsonrpc":"2.0","method":"notifications/progress","params":{"progress":50,"total":100}}

event: message
data: {"jsonrpc":"2.0","id":1,"result":{"content":[...]}}
```

Clients that send `Accept: application/json` (or no Accept header) receive the standard synchronous JSON response.

### Response headers

| Header | Description |
|--------|-------------|
| `Mcp-Session-Id` | Session identifier, returned on every POST response |

## Probes

| Path | Description |
|------|-------------|
| `GET /healthz` | Always returns `{"status":"ok"}` |
| `GET /readyz` | 200 after Discovery Docs loaded, 503 during startup |
| `GET /livez` | Always returns `{"status":"alive"}` |

## Metrics

`GET /metrics` returns Prometheus text format:

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `mcp_gws_mcp_requests_total` | counter | `method`, `status` | Total requests |
| `mcp_gws_mcp_request_duration_seconds` | histogram | `method` | Latency |
| `mcp_gws_mcp_errors_total` | counter | `method`, `error_type` | Errors |
| `mcp_gws_active_tasks` | gauge | — | Active upload/download tasks |
