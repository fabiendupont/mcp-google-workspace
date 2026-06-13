+++
title = "HTTP endpoints"
description = "HTTP endpoint reference"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-12T00:00:00+00:00
draft = false
weight = 30
template = "docs/page.html"
[extra]
lead = "MCP, probes, and metrics endpoints."
toc = true
top = false
+++

## MCP

| Method | Path | Description |
|--------|------|-------------|
| `POST /mcp` | MCP JSON-RPC endpoint | Send MCP requests |
| `GET /mcp` | SSE notification stream | Requires `Accept: text/event-stream` |

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
