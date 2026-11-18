# Security Policy

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.9.x   | Yes       |
| < 0.9   | No        |

## Reporting a vulnerability

If you find a security vulnerability, please report it privately via
[GitHub Security Advisories](https://github.com/fabiendupont/mcp-google-workspace/security/advisories/new).

Do not open a public issue for security vulnerabilities.

You should receive a response within 72 hours. If the vulnerability is confirmed,
a fix will be released as a patch version and credited in the release notes.

## Security model

This project enforces access control through a JSON policy engine. See the
[security documentation](https://fabiendupont.io/mcp-google-workspace/docs/security/model/)
for details on the defense-in-depth model, credential chain, and policy evaluation order.

The HTTP transport additionally requires `Authorization: Bearer` whenever
`--api-token`, `GWS_API_TOKEN`, or `server.api_token` is configured, validates
present `Origin` headers against `server.allowed_origins`, enforces
`server.max_request_bytes`, and rate-limits in two buckets: a fixed
pre-authentication bucket keyed on the client IP (about 60/min) that bounds
credential guessing, and an authenticated bucket keyed on the credential that
uses `server.rate_limit_rpm` (default 600/min). The client IP is the socket peer
unless `server.trusted_proxies` lists the peer's CIDR, in which case the
rightmost untrusted `X-Forwarded-For` hop is used. Trusted proxies are resolved
once at listener construction, so a change needs a restart rather than a SIGHUP
reload. A non-loopback bind without a
token refuses to start; `--insecure-no-auth` is the explicit opt-out for trusted
networks. The authentication requirement is latched at boot, so a SIGHUP reload
that removes the only token locks the listener out (`401`, including
`/metrics`) instead of opening it; `--api-token`/`GWS_API_TOKEN` are captured at
boot and are not affected by reload.

`/webhooks/drive` is a Google push target and is therefore not bearer
authenticated: it validates the channel id, channel token, and resource state
before use, rejects invalid values with `400` without logging them, logs only a
short prefix plus a hash of the channel id, never reads the request body, and has
its own per-client-IP bucket.
