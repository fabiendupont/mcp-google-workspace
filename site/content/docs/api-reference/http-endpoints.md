+++
title = "HTTP endpoints"
description = "HTTP endpoint reference"
date = 2026-06-12T00:00:00+00:00
updated = 2026-11-18T00:00:00+00:00
draft = false
weight = 30
template = "docs/page.html"
[extra]
lead = "MCP, probes, and metrics endpoints."
toc = true
top = false
+++

## Authentication

The HTTP listener has these security layers in front of the MCP service, applied
in this order (outermost first):

1. **Transport body cap** — the boot-time `server.max_request_bytes` backstop;
   chunked uploads without a `Content-Length` stay capped at that value.
2. **Pre-authentication rate limit** — a **fixed** per-client-IP bucket
   (approximately 60 requests/minute, 1024 tracked clients) returns `429` with
   `Retry-After` before any token is compared, so credential guessing and log
   flooding are bounded.
3. **Origin validation** — a request with an `Origin` header must match
   `server.allowed_origins`; when the list is empty only `localhost` origins are
   accepted. Non-browser clients that send no `Origin` are allowed.
4. **Bearer authentication** — when a token is configured (`--api-token`,
   `GWS_API_TOKEN`, or `server.api_token`), `/mcp` and `/metrics` require
   `Authorization: Bearer <token>` and return `401` with
   `WWW-Authenticate: Bearer` otherwise. The scheme is matched
   case-insensitively (`bearer`, `BEARER`), per RFC 7235.
5. **Authenticated rate limit** — `server.rate_limit_rpm` (default 600/min),
   keyed on the presented credential, returns `429` with `Retry-After`.
6. **Declared body limit** — the current `server.max_request_bytes` rejects an
   oversized declared `Content-Length` body with `413`.

`server.rate_limit_rpm: 0` is a startup error, not "unlimited". The client
address for both buckets is the socket peer, unless `server.trusted_proxies`
(a CIDR list such as `["10.0.0.0/8"]`) contains the peer — only then is the
rightmost untrusted `X-Forwarded-For` hop used. With no trusted proxies the
header is ignored entirely, so a direct client cannot pick its own bucket.
Trusted proxies are resolved once when the listener starts, so changing them
requires a restart; a SIGHUP reload does not rebuild the middleware.

When no token is configured, the endpoints are open — but the process refuses to
start on a non-loopback bind unless `--insecure-no-auth` is passed explicitly.
The *requirement* to authenticate is latched at boot: SIGHUP rotates the token
(old and new accepted immediately), but a reload that **removes** the only token
locks the listener out — every request, `/metrics` included, gets `401` and an
error is logged until a token is restored or the process restarts.
`--api-token` / `GWS_API_TOKEN` are captured at boot and unaffected by reload.

rmcp's own `Host` header validation still runs inside the service; list every
client-visible DNS name in `GWS_ALLOWED_HOSTS`.

## MCP

| Method | Path | Content-Type | Description | Auth |
|--------|------|-------------|-------------|------|
| `POST` | `/mcp` | `application/json` | MCP JSON-RPC endpoint (synchronous response) | Bearer when configured |
| `POST` | `/mcp` | `text/event-stream` | MCP JSON-RPC with SSE streaming (when `Accept: text/event-stream`) | Bearer when configured |
| `GET` | `/mcp` | `text/event-stream` | SSE notification stream (requires `Accept: text/event-stream`) | Bearer when configured |

In multi-user mode the same routes are mounted per user at `/mcp/<user>`.

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

Probes stay unauthenticated so kubelet and load balancers can reach them without
a token.

| Path | Description |
|------|-------------|
| `GET /healthz` | Always returns `200` with body `ok` |
| `GET /readyz` | 200 after Discovery Docs loaded, 503 during startup |
| `GET /livez` | Always returns `200` with body `ok` |

## Metrics

`GET /metrics` returns Prometheus text format. When a bearer token is
configured, scrapers must send `Authorization: Bearer <token>`; in multi-user
mode the shared token or any user's configured token is accepted. Because the
authentication requirement is latched at boot, a policy reload that removes the
only token returns `401` here too (never an unauthenticated metrics page).

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `mcp_gws_mcp_requests_total` | counter | `method`, `status` | Total requests |
| `mcp_gws_mcp_request_duration_seconds` | histogram | `method` | Latency |
| `mcp_gws_mcp_errors_total` | counter | `method`, `error_type` | Errors |
| `mcp_gws_active_tasks` | gauge | — | Active upload/download tasks |

## Drive webhooks

| Method | Path | Description | Auth |
|--------|------|-------------|------|
| `POST` | `/webhooks/drive` | Google Drive push notifications | `X-Goog-Channel-Token` (channel secret) |

This endpoint is a **Google push target, not a bearer-authenticated API**: Google
cannot present an `Authorization` header, so the bearer token does not apply. It
is hardened separately from `/mcp`:

- Every header is validated **before use**: `X-Goog-Channel-Id` ≤128 bytes,
  `X-Goog-Channel-Token` ≤96 bytes, and `X-Goog-Resource-State` from a fixed set
  (`sync`, `exists`, `not_exists`, `update`, `trash`, `remove`, `change`). An
  invalid value returns `400` and is never echoed into a log line.
- Logs carry only a short prefix plus a keyed hash of the channel id, never the
  full client-supplied value.
- The request body is never read (only headers are parsed).
- The route has its own small per-client-IP bucket, separate from the MCP
  budgets.
- Google's secret is verified against the channel registered **by the tenant
  that owns it**. Secrets and channels are tenant-scoped in multi-user mode, and
  each tenant is capped at 256 active channels so one tenant cannot evict
  another's secrets.

| Condition | Response |
|-----------|----------|
| Missing, oversized, or malformed header / unknown resource state | `400 Bad Request` |
| `sync` notification with a valid channel secret | `200 OK` |
| Known channel, bad or missing secret | `403 Forbidden` |
| Unknown channel | `404 Not Found` |

In multi-user mode channel IDs are globally unique, so the notification is
verified against the user that owns the subscription.
