+++
title = "Security model"
description = "Policy engine, credential chain, and security properties"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-18T00:00:00+00:00
draft = false
weight = 10
template = "docs/page.html"
[extra]
lead = "How the server enforces access control and handles credentials."
toc = true
top = false
+++

## Defense in depth

Every request passes through multiple checks before reaching Google:

1. **Origin validation** — HTTP origins validated by exact hostname comparison
2. **Rate limiting** — Per-client-IP sliding window
3. **Body size check** — Configurable max request size
4. **Service allow-list** — Only listed services exposed
5. **Method denylist** — Blocked methods return an error
6. **Read-only check** — Non-GET methods blocked when enabled
7. **Parameter constraints** — Values validated against allowed lists with per-value access levels
8. **Request explanation** — Write operations include a plain-English description of what they do

A denied request never reaches Google.

## Policy engine

The policy engine evaluates every `tools/call` against the JSON policy file. Key behaviors:

- **Deny by default** — Services not listed are blocked
- **Generic constraints** — Any parameter can be constrained with an allowlist of values and per-value access levels (read-only or read-write)
- **Body constraints** — On reads, inject query filters to restrict results. On writes, require values from the allowed list.
- **Actionable errors** — Every denial includes a `Fix:` hint with the exact JSON to add to the policy file
- **Origin parsing** — URLs are parsed and hostnames compared exactly (no substring matching)
- **CRLF prevention** — Media content types are validated to reject CR/LF characters

See [Policy reference](../../configuration/policy-reference/) for all configuration options.

## Request explanation

Every write operation includes an `_explanation` field in `structuredContent` that describes the operation in plain English. This lets agents show the user what happened:

```
Create drive/files.create (POST): name="report.pdf", in folder folder-xyz
Delete drive/files.delete (DELETE): fileId=abc123
Create calendar/events.insert (POST): summary="Team standup", on calendar "primary"
```

## Audit log

The `--audit-log` flag writes a JSONL file recording every API call with timestamps, services, methods, status, and duration. Denied requests include the denial reason. See [Operations guide](../../operations/guide/) for details.

## Credential chain

The server checks 7 sources in priority order:

| # | Source |
|---|--------|
| 1 | `GOOGLE_WORKSPACE_CLI_TOKEN` env var |
| 2 | `credentials_file` in policy JSON |
| 3 | `GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE` env var |
| 4 | `~/.config/gws/credentials.json` |
| 5 | `gws auth export --unmasked` (OS keyring) |
| 6 | `GOOGLE_APPLICATION_CREDENTIALS` env var |
| 7 | `~/.config/gcloud/application_default_credentials.json` |

### Token caching

Once a token is obtained, it is cached in memory for 3,500 seconds (just under the 3,600-second OAuth2 lifetime). Subsequent requests reuse the cached token.

### Security properties

- Hard-fail on auth errors (no silent unauthenticated requests)
- `gws auth export` output is never written to disk or logged
- Container image is `FROM scratch` with no shell or package manager
- Live policy reload via SIGHUP keeps the current policy on failure
