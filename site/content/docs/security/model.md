+++
title = "Security model"
description = "Policy engine, credential chain, and security properties"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-12T00:00:00+00:00
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
7. **Folder ACLs** — Drive queries constrained, writes require parents
8. **Calendar ACLs** — Operations must specify an allowed calendarId

A denied request never reaches Google.

## Policy engine

The policy engine evaluates every `tools/call` against the TOML policy file. Key behaviors:

- **Deny by default** — Services not listed are blocked
- **Folder-restricted writes** — When folders are configured, writes without `parents` are denied
- **Calendar-restricted ops** — When calendars are configured, operations without `calendarId` are denied
- **Origin parsing** — URLs are parsed and hostnames compared exactly (no substring matching)
- **CRLF prevention** — Media content types are validated to reject CR/LF characters

See [Policy reference](../../configuration/policy-reference/) for all configuration options.

## Credential chain

The server checks 7 sources in priority order:

| # | Source |
|---|--------|
| 1 | `GOOGLE_WORKSPACE_CLI_TOKEN` env var |
| 2 | `credentials_file` in policy TOML |
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
