+++
title = "Operations guide"
description = "Observability, monitoring, and multi-user deployment"
date = 2026-06-12T00:00:00+00:00
updated = 2026-11-18T00:00:00+00:00
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

## Audit log

Track every API call with a structured JSONL audit trail:

```bash
mcp-google-workspace --policy policy.json --audit-log /var/log/mcp-gws/audit.jsonl
```

Each line is a JSON object with an `action`/`outcome` pair that is one of
`allowed`, `denied`, or `error`. Allowed operations record the real HTTP status
and duration; tool calls use `resource: "tool"` and `http_method: "MCP"`:

```json
{"timestamp":"1718745600.000","action":"allowed","outcome":"allowed","service":"drive","resource":"files","method":"list","http_method":"GET","status":200,"duration_ms":142,"tool":"gws_drive_list"}
{"timestamp":"1718745601.000","action":"allowed","outcome":"allowed","service":"gmail","resource":"tool","method":"gws_gmail_send","http_method":"MCP","status":200,"duration_ms":38,"tool":"gws_gmail_send","subject":"alice@example.com"}
{"timestamp":"1718745602.000","action":"denied","outcome":"denied","service":"docs","resource":"documents","method":"create","status":403,"reason":"Service 'docs' is read-only"}
{"timestamp":"1718745603.000","action":"error","outcome":"error","service":"gmail","resource":"tool","method":"gws_gmail_read","http_method":"MCP","status":404,"duration_ms":21,"reason":"Not Found","tool":"gws_gmail_read"}
```

- `status` is the upstream Google API status (or the mapped tool status: `403`
  for a policy denial, `500` for an internal error).
- `tool` is present for tool-level entries; `subject` appears when a delegated
  subject was used via `_meta.user_email` or `server.delegate_subject`.
- Every policy refusal is classified from a typed signal, not message wording, so
  it is always `action: "denied"` with `status: 403` and a `reason` — including
  parameter-level protected-value denials, constraint violations, and
  folder-scope failures.
- A call that policy allowed but that failed for a non-policy reason (upstream
  API error, transport failure, internal bug) is `action: "error"` with its real
  status and a reason. It is never labelled `allowed`.
- `reason` is JSON-escaped, so a newline in an error message cannot forge a
  second audit line.

Use `jq` to query the audit log:

```bash
# All denied requests
jq 'select(.action == "denied")' audit.jsonl

# Failures that were not policy denials
jq 'select(.action == "error")' audit.jsonl

# Drive writes in the last hour
jq 'select(.service == "drive" and .http_method != "GET")' audit.jsonl

# Slowest requests
jq -s 'sort_by(-.duration_ms) | .[0:10]' audit.jsonl

# Delegated activity for one subject
jq 'select(.subject == "alice@example.com")' audit.jsonl
```

## Rate limiting

Two independent buckets guard `/mcp`:

| Bucket | Key | Budget | Purpose |
|--------|-----|--------|---------|
| Pre-authentication | client IP | fixed ~60/min | bound credential guessing and log flooding |
| Authenticated | presented credential | `server.rate_limit_rpm` (default 600/min) | bound a legitimate caller's load |

Both answer `429` with `Retry-After` and track at most 1024 clients (LRU
eviction). `rate_limit_rpm: 0` is a startup error, not "unlimited".

The client address is the socket peer. Behind a reverse proxy, set
`"trusted_proxies": ["10.0.0.0/8"]` in the `server` section: `X-Forwarded-For`
is honoured only when the immediate peer matches one of those CIDRs, and the
rightmost untrusted hop is used as the client. With no trusted proxies the header
is ignored, so a direct client cannot choose its own bucket. Trusted proxies are
resolved once at listener construction, so changing them needs a restart rather
than a SIGHUP reload. The separate
per-service **outbound** budgets come from `server.rate_limits`
(`{"drive": 30}`), which is not an HTTP knob; unknown keys and zero values are
startup errors.

`/webhooks/drive` has its own small per-client-IP bucket (about 120/min) and does
not share the MCP budgets.

## Live policy reload

When running in HTTP mode with `--policy`, the server reloads the policy file on `SIGHUP`:

```bash
# Edit the policy file
vim policy.json

# Reload without restart
kill -HUP $(pidof mcp-google-workspace)
```

The server logs the result:
- Success: `Policy reloaded` with the new service list
- Failure: `Policy reload failed, keeping current policy` with the error

Token rotation works the same way: the auth middleware re-reads the policy on
every request, so changing `server.api_token` and sending `SIGHUP` takes effect
without dropping connections, and both the old and the new token are accepted
immediately after the reload. Revoking is different: the *requirement* to
authenticate is latched at boot, so a reload that removes the only token is a
**lockout**, not a bypass — every request to `/mcp` and `/metrics` returns `401`
and the server logs an error until a token is restored or the process restarts.
`--api-token` / `GWS_API_TOKEN` are captured at boot and are unaffected by a
reload. With `--users-dir`, every user's policy reloads independently.

Not available in stdio mode (the MCP client manages the process lifecycle).

## Multi-user deployment

Two supported patterns:

**One process, many users (`--users-dir`)** — the server serves each subdirectory
as an independent user with its own policy, credentials, audit log, and MCP
endpoint (`/mcp/<user>`). Drive webhook notifications fan out to all users and
are verified against the subscription owner's channel secret. A shared
`--api-token` applies to every endpoint; otherwise set `server.api_token` per
policy and keep the process on a non-loopback bind only when every user has one.

**One instance per user** — each instance gets its own credentials Secret, policy
ConfigMap, and Deployment. This gives the strongest isolation: different
policies, credentials, rate limits, and no shared process state. Each instance
uses approximately 50m CPU idle and 20-30 MB memory.

In both patterns, `/metrics` requires the bearer token when one is configured,
so Prometheus needs the same Secret. Liveness, readiness, and health probes stay
open.
