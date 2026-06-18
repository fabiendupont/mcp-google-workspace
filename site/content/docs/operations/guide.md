+++
title = "Operations guide"
description = "Observability, monitoring, and multi-user deployment"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-12T00:00:00+00:00
draft = false
weight = 10
template = "docs/page.html"
[extra]
lead = "Logging, tracing, metrics, and multi-user patterns."
toc = true
top = false
+++

## Structured logging

Logs go to stderr via the `tracing` crate. Control with `RUST_LOG`:

```bash
RUST_LOG=info mcp-google-workspace --policy policy.json
RUST_LOG=mcp_google_workspace=debug mcp-google-workspace --policy policy.json
```

## OpenTelemetry traces

Set `OTEL_EXPORTER_OTLP_ENDPOINT` to enable trace export:

```bash
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 \
  mcp-google-workspace --policy policy.json
```

Traces include spans for each `execute_tool` call with service, resource, and method fields. W3C Trace Context (`traceparent`, `tracestate`, `baggage`) is forwarded from MCP `_meta` to Google API requests.

## Prometheus metrics

Available at `GET /metrics` (HTTP transport only):

| Metric | Type | Labels |
|--------|------|--------|
| `mcp_gws_mcp_requests_total` | counter | `method`, `status` |
| `mcp_gws_mcp_request_duration_seconds` | histogram | `method` |
| `mcp_gws_mcp_errors_total` | counter | `method`, `error_type` |
| `mcp_gws_active_tasks` | gauge | — |

### Useful PromQL queries

```promql
rate(mcp_gws_mcp_requests_total[5m])                                           # Request rate
histogram_quantile(0.99, rate(mcp_gws_mcp_request_duration_seconds_bucket[5m])) # P99 latency
rate(mcp_gws_mcp_errors_total[5m]) / rate(mcp_gws_mcp_requests_total[5m])       # Error rate
```

## Multi-user deployment

The server runs as a single-user process. For multi-user environments, deploy one instance per user. Each instance gets its own credentials Secret, policy ConfigMap, and Deployment.

This gives per-user isolation: different policies, credentials, rate limits, and no shared state. Each instance uses approximately 50m CPU idle and 20-30 MB memory.
