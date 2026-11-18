+++
title = "Security model"
description = "Policy engine, credential chain, and security properties"
date = 2026-06-12T00:00:00+00:00
updated = 2026-11-18T00:00:00+00:00
draft = false
weight = 10
template = "docs/page.html"
[extra]
lead = "How the server enforces access control and handles credentials."
toc = true
top = false
+++

## Defense in depth

Every HTTP request passes through these layers before it reaches the MCP
protocol; every tool call then passes through policy before reaching Google:

1. **Bearer authentication** -- `/mcp` and `/metrics` require
   `Authorization: Bearer` whenever a token is configured (`--api-token`,
   `GWS_API_TOKEN`, or `server.api_token`). A non-loopback bind without a token
   refuses to start (exit 1); a loopback bind warns; `--insecure-no-auth` is the
   explicit opt-out. Probes stay open. The requirement is latched at boot: SIGHUP
   can rotate the token, but a reload that removes the only token locks the
   listener out (`401`, `/metrics` included) instead of opening it.
2. **Origin validation** -- A present `Origin` header must match
   `server.allowed_origins` by host, otherwise `403`. Absent `Origin` is allowed;
   an empty allowlist accepts localhost origins only.
3. **Body size check** -- `server.max_request_bytes` rejects declared oversized
   bodies with `413`; oversized chunked bodies are capped by the transport.
4. **Rate limiting** -- Two buckets: a fixed pre-authentication per-client-IP
   budget (~60/min) that bounds credential guessing, and an authenticated budget
   keyed on the credential that uses `server.rate_limit_rpm` (default 600/min).
   Both return `429` with `Retry-After`, with 1024 tracked clients. The client
   address is the socket peer unless `server.trusted_proxies` lists the peer's
   CIDR; only then is the rightmost untrusted `X-Forwarded-For` hop used. Trusted
   proxies are resolved once when the listener starts, so changing them requires
   a restart, not a SIGHUP reload. A
   separate per-service outbound limiter protects Google quotas.
   `rate_limit_rpm: 0` is a startup error.
5. **Host allowlist** -- rmcp rejects unexpected `Host` headers; `GWS_ALLOWED_HOSTS`
   adds Service DNS names.
6. **Delegation subject** -- `_meta.user_email` must be listed in
   `server.allowed_subjects`, otherwise it is rejected; `server.delegate_subject`
   is the operator-controlled fallback.
7. **Service allow-list** -- Only listed services exposed.
8. **Resource allowlist** -- `allowed_resources` applies to tool calls, `gws://`
   resources, and subscriptions.
9. **Method denylist** -- Blocked methods and their method-family aliases return
   an error (suffix matching: `"messages.send"` matches
   `"users.messages.send"`).
10. **Read-only check** -- Non-GET methods blocked when enabled.
11. **Label policy** -- Gmail messages restricted to allowed labels, failing
    closed if the labels do not resolve. Thread reads validate **every** returned
    message against `allowed_labels`, drop non-matching ones, and mark the result
    `"partialRead": true` with `"omittedMessageCount": N`; `messageCount` counts
    the messages actually returned.
12. **Parameter constraints** -- Values validated against allowed lists with
    per-value access levels, including recursive Drive parent scoping for reads
    and by-ID writes.
13. **Generic tool bypass prevention** -- Services with helpers block the generic
    tool, and `gws_batch` rejects helper-only services.
14. **Request explanation** -- Write operations include a plain-English
    description of what they do.

A denied request never reaches Google.

### Delegation trust model

`_meta.user_email` is client-controlled, so it is not trusted on its own. When a
client supplies one, the server accepts it only if it case-insensitively matches
an entry in `server.allowed_subjects`; otherwise the request is denied and the
attempt is audited. When the client supplies no subject, `server.delegate_subject`
is used if configured. With an empty `allowed_subjects` (the default), per-request
delegation is disabled entirely.

### Drive by-ID scoping (reads and writes)

When a recursive `parents` constraint exists, Drive reads and the Docs, Sheets,
and Slides readers are not exempt from folder policy: `files.get`,
`files.export`, `files.copy` (source), `documents.get`,
`spreadsheets.get`, `spreadsheets.values.get`, and `presentations.get` all
resolve the file's parents, walk the ancestry when needed, and deny files that do
not resolve to an allowed root. By-ID **writes** are ancestry-checked too:
`permissions.create`, `files.update`, `files.delete`, `documents.batchUpdate`,
`spreadsheets.values.update`/`append`/`clear`, `spreadsheets.batchUpdate`, and
`presentations.batchUpdate`. A `parents` constraint no longer wrongly blocks
`permissions.create` or `files.update`, which carry no parent of their own. Files
with no visible parent fail closed. The metadata lookup uses its own Drive
read-only token, so a Docs- or Sheets-scoped request cannot be denied for lack of
Drive scope.

## Policy engine

The policy engine evaluates every `tools/call` against the JSON policy file. Key behaviors:

- **Deny by default** -- Services not listed are blocked
- **Resource allowlist** -- `allowed_resources` restricts tool execution, `gws://` resources, and subscriptions
- **Generic constraints** -- Any parameter can be constrained with an allowlist of values and per-value access levels (read-only or read-write)
- **Body constraints** -- On reads, inject grouped query filters to restrict results. On writes, require values from the allowed list. `body-write-only` requires the field on create/copy but not on unrelated updates.
- **Parent ancestry** -- Drive folder constraints with `recursive` check parent folders. A file in a subfolder of an allowed folder inherits access; read-by-ID calls (`files.get`, `files.export`, `files.copy`, plus the Docs, Sheets, and Slides readers) and by-ID writes (`permissions.create`, `files.update`/`delete`, `documents.batchUpdate`, `spreadsheets.values.update`/`append`/`clear`, `spreadsheets.batchUpdate`, `presentations.batchUpdate`) must resolve to one of those roots. `recursive: false` disables by-ID scoping (the list filter still applies), and the server warns about it at startup and in `check-policy`.
- **Protect mode** -- `mode: "protect"` keeps a document or presentation read-only and prevents moving Drive files into or out of a protected folder (`addParents`/`removeParents`).
- **Method-family denials** -- Equivalent destructive calls move together: `messages.send`/`users.drafts.send`, `messages.delete`/`batchDelete`, and `files.delete`/`files.emptyTrash`. The `--services` shorthand applies baseline denials too. `gws_drive_trash` is destructive for policy purposes even though it is implemented as `files.update {trashed: true}`: it checks the whole `files.trash`/`files.delete`/`files.emptyTrash` family and refuses with a `Trash blocked` policy error without issuing a request, so denying `files.delete` also blocks trashing.
- **Drive ID validation** -- Drive IDs (`file_id`, `folder_id`, `parent_id`, `to_folder_id`) are restricted to `^[A-Za-z0-9_-]+$` (minimum 20 characters, plus the `root` alias) before any request, because they are interpolated into the Drive search grammar; pasted URLs and values containing `/` or a space are rejected. Free-text query values are escaped (backslash, then `'`, control characters stripped). `gws_drive_share` accepts only `reader`/`commenter`/`writer` and derives the permission `type` from `email` vs `domain`, so `anyoneWithLink` sharing is unreachable.
- **Gmail label policy** -- The `allowed_labels` field restricts which labels the agent can access. Search queries are grouped, each hit is checked, and thread reads validate every returned message, dropping non-matching ones and reporting `partialRead`/`omittedMessageCount`. The tools fail closed if none of the configured labels resolve.
- **Write approval** -- With `confirm_writes`, every mutating tool (including `gws_generate_image`) raises an elicitation whose summary always shows the security-relevant arguments: share recipient/role/type, Gmail to/cc/bcc plus subject with body length and first line, calendar id/start/attendee count, and Sheets spreadsheet id/range/row-column counts/first row. Values are control-character-sanitised and newlines render as `⏎`.
- **Scoping warnings** -- Startup and `check-policy` warn about `recursive: false` on a `parents` constraint, a non-read-only service with no constraints, an unconstrained `calendar`, and Gmail settings/forwarding methods that are not denied. `check-policy` exits non-zero when any warning is present.
- **Helper tool enforcement** -- Services with helpers (Drive, Docs, Sheets, Slides, Gmail, Calendar, Tasks) suppress the generic tool and are rejected by `gws_batch`, which directs callers to the helpers. `--eager-tools` filters helper tools through the policy's allowed services, so only permitted services appear in `tools/list`.
- **Actionable errors** -- Every denial includes a `Fix:` hint with the exact JSON to add to the policy file
- **Origin parsing** -- URLs are parsed and hostnames compared exactly (no substring matching)
- **CRLF prevention** -- Media content types and Gmail headers are validated to reject CR/LF characters; MIME boundaries are unique per message.
- **CSV safety** -- Sheets CSV export quotes embedded CR/LF and prefixes formula-leading cells by default (`sanitize_formulas: true`).
- **Write preview scope** -- `dry_run` is honored only by tools with a real preview; on a mutating tool without one it is rejected rather than silently ignored, so approval still applies and the caller cannot mistake an executed write for a preview. `gws_batch` accepts `dry_run` on the batch call and on an individual entry; a sub-request that returns an API error is reported `"status":"error"`, excluded from `succeeded`, and counted in `failed`.

See [Policy reference](../../configuration/policy-reference/) for all configuration options.

## Request explanation

Every write operation includes an `_explanation` field in `structuredContent` that describes the operation in plain English. This lets agents show the user what happened:

```
Create drive/files.create (POST): name="report.pdf", in folder folder-xyz
Delete drive/files.delete (DELETE): fileId=abc123
Create calendar/events.insert (POST): summary="Team standup", on calendar "primary"
```

## Audit log

The `--audit-log` flag writes a JSONL file recording every API call with timestamps, services, methods, tool names, status, and duration. Every policy refusal is classified from a typed signal, so it is guaranteed to appear as `"action":"denied"` with `"status":403` and a `"reason"` — including param-level protected-value denials, constraint violations, and folder-scope failures. A call that policy allowed but that failed for another reason is `"action":"error"` with its real status and a reason, never an `allowed` line with a 500. See [Operations guide](../../operations/guide/) for details.

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
