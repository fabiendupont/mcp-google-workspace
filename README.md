# mcp-google-workspace

[![CI](https://github.com/fabiendupont/mcp-google-workspace/actions/workflows/ci.yml/badge.svg)](https://github.com/fabiendupont/mcp-google-workspace/actions/workflows/ci.yml)
[![Release](https://github.com/fabiendupont/mcp-google-workspace/actions/workflows/release.yml/badge.svg)](https://github.com/fabiendupont/mcp-google-workspace/actions/workflows/release.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Container](https://img.shields.io/badge/ghcr.io-mcp--google--workspace-blue?logo=github)](https://github.com/fabiendupont/mcp-google-workspace/pkgs/container/mcp-google-workspace)
[![MCP](https://img.shields.io/badge/MCP-2026--07--28_RC-green)](https://modelcontextprotocol.io/)
[![Docs](https://img.shields.io/badge/docs-fabiendupont.github.io-blue)](https://fabiendupont.io/)

MCP server for Google Workspace APIs with per-project safety policies.

**[Documentation](https://fabiendupont.io/)** | **[Quick Start](https://fabiendupont.io/docs/getting-started/quickstart/)** | **[API Reference](https://fabiendupont.io/docs/api-reference/tool-arguments/)**

Gives AI agents controlled access to Drive, Gmail, Calendar, Sheets, Docs, and
other Google services through the [Model Context Protocol](https://modelcontextprotocol.io/).
A JSON policy file scopes what each project can access — parameter-level
constraints, method denylists, and global read-only mode. An interactive wizard
and policy templates make setup easy. Audit logs, SIGHUP reload, and
human-readable write explanations make it production-ready.

## Protocol Support

| MCP Version | Status |
|-------------|--------|
| 2026-07-28 RC | Full support (`server/discover`, `_meta`, `structuredContent`, `ttlMs`) |
| 2025-11-25 | Supported (tool `title`, `annotations`) |
| 2024-11-05 | Supported (`initialize` / `initialized` handshake) |

The server auto-detects the client's protocol era and adapts accordingly.

## Architecture

See [docs/architecture.md](docs/architecture.md) for the full request flow diagram
and multi-user deployment pattern.

## Security Model

The policy engine enforces access control at the MCP layer, before any Google
API call is made. The HTTP transport adds network-level checks in front of it.

Policy enforcement:

- **Service allow-list**: Only listed services are exposed. Everything else is denied.
- **Parameter constraints**: Generic allowlists on any API parameter with per-value access levels.
- **Method denylists**: Block specific methods (e.g., `messages.delete`). Families move
  together: `messages.send` also blocks `users.drafts.send`, `messages.delete` also
  blocks `batchDelete`, and `files.delete` also blocks `files.emptyTrash` — and
  `gws_drive_trash`, which is destructive for policy purposes even though it is
  implemented as `files.update {trashed: true}`.
- **Resource allowlist**: Optional `allowed_resources` per service is enforced on tool
  execution, `gws://` URIs, and subscriptions—not just on discovery.
- **Read-only mode**: Global or per-service.
- **Write confirmation**: Optional MCP elicitation approval before every mutating
  operation (`confirm_writes`).
- **Delegation trust**: A client-supplied `_meta.user_email` is rejected unless it is
  listed in `server.allowed_subjects`. An empty list disables per-request delegation;
  `server.delegate_subject` remains the operator-controlled fallback.
- **Request explanation**: Write operations include a plain-English description of what they do.
- **Audit log**: Structured JSONL trail of every API call, allowed, denied, and failed,
  plus one line for every `tools/call`. Entries carry the real HTTP status, tool name,
  and the delegated subject when one was used. Every policy refusal is classified from
  a typed signal, so it is always `"action":"denied"` with `"status":403` and a
  `"reason"` — including parameter-level protected values, constraint violations, and
  folder-scope failures. A non-policy failure is `"action":"error"` with its real
  status, never an `allowed` line with a 500.
- **Live reload**: SIGHUP reloads the policy file without restart (HTTP transport) and
  applies a rotated `server.api_token` immediately. The *requirement* to authenticate
  is latched at boot: a reload that **removes** the only token is a lockout, not a
  bypass — `/mcp` and `/metrics` answer `401` and an error is logged until a token is
  restored or the process restarts. `--api-token` / `GWS_API_TOKEN` are captured at
  boot and are unaffected by a reload.

HTTP transport layers:

- **Bearer authentication**: `--api-token`, `GWS_API_TOKEN`, or `server.api_token`
  requires `Authorization: Bearer <token>` on `/mcp` and `/metrics`. A non-loopback
  bind without a token **refuses to start** (exit 1); a loopback bind logs a warning.
  `--insecure-no-auth` is the explicit opt-out for trusted networks. Health and
  readiness probes stay open for the orchestrator. The authentication *requirement* is
  latched from the boot policy: SIGHUP can rotate the token (old→new accepted
  immediately) but revoking the only token leaves the listener locked out at `401`
  rather than open — see the lockout note above.
- **Origin validation**: A request that carries an `Origin` header must match
  `server.allowed_origins` by host, otherwise it gets `403`. Requests without an
  `Origin` header keep working for non-browser MCP clients. An empty allowlist accepts
  localhost origins only.
- **Request size limit**: `server.max_request_bytes` rejects declared oversized bodies
  with `413`; oversized chunked bodies are capped by the transport.
- **Rate limiting**: Two buckets. A fixed pre-authentication bucket (about 60/min per
  client IP) bounds credential guessing and log flooding before any token comparison;
  an authenticated bucket keyed on the credential itself uses
  `server.rate_limit_rpm` (default 600/min). Both return `429` with `Retry-After`, and
  the tracked-client map is bounded to 1024 entries. The client address is the socket
  peer unless `server.trusted_proxies` lists the peer's CIDR, in which case the
  rightmost untrusted `X-Forwarded-For` hop is used; with no trusted proxies the
  header is ignored. Trusted proxies are resolved once when the listener starts,
  so changing `server.trusted_proxies` needs a restart; a SIGHUP reload replaces
  policy contents but does not rebuild the middleware. Separately, per-service
  outbound budgets (`server.rate_limits`,
  defaults of 600/240/300/300/200/300 per minute) protect Google quotas.
  `rate_limit_rpm: 0` is a startup error, not "unlimited".
- **Host allowlist**: `GWS_ALLOWED_HOSTS` extends the transport's accepted `Host`
  values, which is how the Kubernetes Service DNS names are allowed.

## Google Credentials Setup

The server needs OAuth2 credentials to call Google APIs. Choose one method:

### Option A: Application Default Credentials (simplest for local dev)

```bash
gcloud auth application-default login \
  --scopes=https://www.googleapis.com/auth/drive,\
https://www.googleapis.com/auth/gmail.modify,\
https://www.googleapis.com/auth/calendar
```

This stores credentials at `~/.config/gcloud/application_default_credentials.json`.
The server finds them automatically.

### Option B: Service Account (recommended for production)

1. Go to [Google Cloud Console](https://console.cloud.google.com/) → **IAM & Admin** → **Service Accounts**
2. Create a service account, download the JSON key file
3. Enable the APIs you need (Drive API, Gmail API, Calendar API, etc.) under **APIs & Services** → **Enabled APIs**
4. Share Drive folders or calendars with the service account's email address
5. Set the credentials path:

```bash
export GOOGLE_APPLICATION_CREDENTIALS=/path/to/service-account-key.json
```

### Option C: OAuth2 Client Credentials (for user-scoped access)

1. Go to **APIs & Services** → **Credentials** → **Create Credentials** → **OAuth client ID**
2. Choose **Desktop application**, download the JSON
3. Use the [gws CLI](https://github.com/googleworkspace/cli) to complete the OAuth flow:

```bash
# Install gws
cargo install google-workspace-cli

# Authenticate (opens browser)
gws auth login --credentials /path/to/client-credentials.json

# Credentials are stored in the OS keyring
# The MCP server reads them automatically via `gws auth export`
```

You can also export credentials to a file and reference it in the policy:

```bash
gws auth export --unmasked > /path/to/credentials.json
```

```json
{
  "server": {
    "credentials_file": "/path/to/credentials.json"
  }
}
```

### Credential Priority

The server checks these locations in order:

1. `GOOGLE_WORKSPACE_CLI_TOKEN` env var (raw access token)
2. `credentials_file` from policy JSON `server` object
3. `GOOGLE_WORKSPACE_CLI_CREDENTIALS_FILE` env var
4. `~/.config/gws/credentials.json`
5. `gws auth export --unmasked` (reads from OS keyring)
6. `GOOGLE_APPLICATION_CREDENTIALS` env var
7. `~/.config/gcloud/application_default_credentials.json`

## Quick Start

### Container (recommended)

The image's default command runs the HTTP transport on `0.0.0.0:3000`, which is a
non-loopback bind: the server refuses to start unless a token is configured. Generate
one and pass it in at run time (never bake it into the image):

```bash
export GWS_API_TOKEN="$(openssl rand -hex 32)"

podman run -p 3000:3000 \
  -e GWS_API_TOKEN="$GWS_API_TOKEN" \
  -v ./gws-policy.json:/etc/mcp-google-workspace/policy.json:ro \
  -v ./credentials.json:/secrets/credentials.json:ro \
  -e GOOGLE_APPLICATION_CREDENTIALS=/secrets/credentials.json \
  ghcr.io/fabiendupont/mcp-google-workspace:0.9.1
```

Pin a version tag — or better, a digest. Never rely on `:latest`: deployment
integrity depends on the image reference being immutable, and a mutable tag can be
re-pointed at different bytes after you review it. Releases publish immutable
per-version tags plus a manifest digest; `:latest` is only a convenience alias,
moved by a separate job that asserts it points at the same digest.

Clients must then send `Authorization: Bearer $GWS_API_TOKEN` to `/mcp` (and to
`/metrics`). For a trusted, isolated network you can pass `--insecure-no-auth`
instead, but that exposes every tool the policy allows to anyone who can reach the
port. For production, pin the image by digest
(`ghcr.io/fabiendupont/mcp-google-workspace:<version>@sha256:<digest>`) and verify
the published `SHA256SUMS` and its cosign signature — see
[Verifying a release](https://fabiendupont.io/docs/getting-started/installation/#verifying-a-release).

### From Source

```bash
cargo build --release
./target/release/mcp-google-workspace --policy gws-policy.json
```

### HTTP Transport

```bash
# Loopback: allowed without a token, but logs a warning.
./target/release/mcp-google-workspace --policy gws-policy.json --http 127.0.0.1:3000

# Authenticated (required for any non-loopback bind):
./target/release/mcp-google-workspace --policy gws-policy.json \
  --http 0.0.0.0:3000 --api-token "$GWS_API_TOKEN"
```

`--api-token` overrides `GWS_API_TOKEN` and the policy's `server.api_token`. A
non-loopback bind with none of those set exits with status 1. Pass
`--insecure-no-auth` only when another layer already restricts who can reach the
port.

## Policy File

Use a template for quick setup:

```bash
mcp-google-workspace init --template assistant > gws-policy.json
```

Available templates: `analyst`, `assistant`, `admin-readonly`, `content-writer`, `it-admin`.
Use `mcp-google-workspace init --template list` to see descriptions. The
`assistant` template grants Docs read-only. The wizard refuses to write through a
symlink and asks before overwriting an existing file.

Or run the interactive wizard:

```bash
mcp-google-workspace init
```

See [`policy.example.json`](policy.example.json) for a full example with constraints.

```json
{
  "server": {
    "read_only": false,
    "confirm_writes": false,
    "rate_limit_rpm": 120,
    "max_request_bytes": 16777216,
    "allowed_origins": ["internal.corp.com"],
    "allowed_subjects": []
  },
  "services": [
    {
      "name": "drive",
      "allowed_resources": ["files", "permissions"],
      "constraints": [
        { "param": "parents", "values": ["folder-id"], "access": "read-write", "location": "body", "recursive": true }
      ]
    },
    {
      "name": "gmail",
      "denied_methods": ["messages.delete", "messages.trash",
        "settings.updateAutoForwarding", "settings.delegates.create"]
    },
    {
      "name": "calendar",
      "constraints": [
        { "param": "calendarId", "values": ["primary"], "access": "read-write" }
      ]
    },
    {
      "name": "tasks"
    }
  ]
}
```

Method denials are family-aware: denying `messages.delete` also denies
`messages.batchDelete`, denying `messages.send` also denies `users.drafts.send`, and
denying `files.delete` also denies `files.emptyTrash` and the `gws_drive_trash`
helper (which checks the whole `files.trash`/`files.delete`/`files.emptyTrash`
family and refuses before issuing a request; trashing is recoverable from the Drive
trash). The `--services` shortcut
applies the same safe baseline denials automatically, while a policy file is still
required for folder, calendar, and label scoping. When `allowed_resources` is set, it
is enforced on tool calls, `gws://` resources, and subscriptions; entries also
allow dotted descendants, and `permissions` is required by `gws_drive_share` and
image insertion.

A recursive `parents` constraint scopes by-ID **reads and writes**: `files.get`,
`files.export`, `files.copy`, the Docs/Sheets/Slides readers, and by-ID mutations
(`permissions.create`, `files.update`/`delete`, `documents.batchUpdate`,
`spreadsheets.values.update`/`append`/`clear`, `spreadsheets.batchUpdate`,
`presentations.batchUpdate`) are ancestry-checked. Without `"recursive": true` the
by-ID checks are inactive; the server logs a startup warning and `check-policy`
reports it. A `parents` constraint no longer wrongly blocks `permissions.create`
or `files.update`.

`rate_limit_rpm` is the authenticated request budget and must be at least 1 —
`0` is a startup error, not "unlimited". `server.rate_limits` is a per-service
**outbound** override map (`{"drive": 30}`), not an HTTP knob; unknown keys and
zero values are startup errors. When the server runs behind a reverse proxy, list
the proxy's CIDRs in `server.trusted_proxies` so the client IP (not the proxy's)
is rate-limited; otherwise `X-Forwarded-For` is ignored. Trusted proxies are
read at listener start, so that change needs a restart, not a SIGHUP.

Startup and `check-policy` also warn about policy shapes that silently widen
access: `recursive: false` on a `parents` constraint, a non-read-only service with
no constraints, an unconstrained `calendar`, and Gmail settings/forwarding methods
that are not denied. `check-policy` exits non-zero when any warning is present, so
CI can gate on it.

### Write confirmation (human in the loop)

Set `"confirm_writes": true` in the `server` section to require explicit user
approval before any mutating operation. The server sends an MCP elicitation
request built from a per-tool summary that always shows the security-relevant
arguments: for a share, the recipient plus role and type; for a Gmail send,
`to`/`cc`/`bcc` plus subject and body length with the first line; for a calendar
create, the calendar id, start time, and attendee count; for a Sheets write, the
spreadsheet id, range, row/column counts, and first row. Values are
control-character-sanitised and newlines are rendered as `⏎`, so a prompt cannot
be forged or flattened by caller-supplied text. Writes proceed only after
approval; read-only tools are never interrupted, and writes are denied (rather
than silently allowed) if the client cannot display approvals. `gws_batch` is
rejected in this mode so every write goes through an individually confirmable
call. `gws_generate_image` is treated as a write (it appears in `tools/list` with
`readOnlyHint: false` and is discoverable through `gws_discover`), and it honours
the delegated `_meta.user_email` subject in multi-user mode.

`dry_run` is **only** honored by tools that implement a real preview: the
generic REST proxy, `gws_batch`, `gws_docs_*`, `gws_slides_*`, and
`gws_generate_image`. On a mutating tool without a preview the flag is rejected
with an explanatory error, so a caller can never believe a write was previewed
when it actually executed; it also cannot be used to skip approval or policy
checks. `gws_batch` accepts `dry_run` on the batch call and on an individual
entry; a sub-request whose payload carries an API error is reported with
`"status":"error"`, excluded from `succeeded`, and counted in `failed`.

### Workflow recipes (MCP prompts)

Multi-service workflows live as **editable prompt recipes** in `prompts/`
(loaded via `--prompts-dir` or a `prompts/` directory next to your policy
file), not as hard-coded tools — agents chain the primitive tools
themselves, which keeps them adaptable and the recipes editable without
recompiling. Shipped recipes include:

- **`thread-to-doc`** — Gmail thread → formatted Google Doc
  (one section per message, participants summary)
- **`doc-to-slides`** — Google Doc → Marp → presentation deck
- **`cross-service-workflow`** — email → document → spreadsheet pipeline

For deterministic Markdown → slides conversion, `gws_slides_import_marp`
accepts `split_headings: true`, which turns plain document Markdown into
slides (each H1/H2 heading starts a new slide) when no `---` separators
are present.

### Google Tasks

The `tasks` service is exposed through curated helper tools:
`gws_tasks_lists`, `gws_tasks_list`, `gws_tasks_get`, `gws_tasks_create`,
`gws_tasks_update`, `gws_tasks_complete`, `gws_tasks_delete`, and
`gws_tasks_clear`. All default to the first task list when no
`tasklist_id` is given.

### Resource subscriptions (with polling fallback)

Clients can subscribe to resources via `gws://service/resource/id` URIs.

- In HTTP mode with `--external-url`, the server registers a Google Drive
  push channel and forwards webhook notifications as
  `notifications/resources/updated`. Channel IDs are random, and each channel
  carries an unguessable secret that Google echoes in
  `X-Goog-Channel-Token`; the server rejects a bad token with `403` and an
  unknown channel with `404`. The `/webhooks/drive` route is
  **Google-push-targeted, not bearer-authenticated** (Google cannot present a
  bearer token), so it is hardened separately: channel id (≤128 bytes), channel
  token (≤96 bytes), and resource state (from a fixed set) are validated before
  use, invalid values get `400` and are never logged, logs carry only a short
  prefix plus a keyed hash of the channel id, the request body is never read,
  and the route has its own small per-client-IP bucket. Existing webhook
  subscriptions must be renewed after upgrading because the old channel records
  have no secret.
- Without webhook support (stdio mode, or HTTP without `--external-url`),
  the server falls back to **polling**: it re-reads the resource on a
  schedule and emits `notifications/resources/updated` whenever its
  content changes. This works for any service with a `get` method, not
  just Drive.

`resources/subscribe` enforces the policy at subscription time: the Drive
service must be enabled and writable (push channels call `files.watch`), and
the resource must be in `allowed_resources` when that allowlist is set.

The polling interval defaults to 60 seconds and can be tuned in the
policy's `server` section:

```json
{ "server": { "subscription_poll_secs": 120 } }
```

### Multi-user mode (single process, many users)

Instead of running one server per user, a single HTTP process can serve
many users, each with **isolated credentials, policy, token cache, audit
log, and subscriptions**:

```
users-dir/
├── alice/
│   ├── gws-policy.json     # alice's services, constraints, credentials
│   └── audit.jsonl         # created automatically: alice's audit trail
└── bob/
    └── gws-policy.json
```

```bash
mcp-google-workspace --http 0.0.0.0:3000 --users-dir /srv/gws-users \
  --api-token "$GWS_API_TOKEN"
```

Each user's agent connects to its own endpoint: `http://host:3000/mcp/alice`,
`http://host:3000/mcp/bob`, and so on. Directory names must be URL-safe
(letters, digits, `-`, `_`).

**Authentication**: a single `--api-token` (or `GWS_API_TOKEN`) applies to every
user endpoint and overrides per-policy tokens. Alternatively, set
`server.api_token` in each user's policy to require
`Authorization: Bearer <token>` on that user's endpoint:

```json
{ "server": { "api_token": "a-long-random-secret" }, "services": [ ... ] }
```

Requests with a missing or wrong token get `401 Unauthorized`. The process
refuses to start on a non-loopback bind unless every user either has an
`api_token` or the shared `--api-token` is set; `--insecure-no-auth` is the
explicit opt-out. A user without any token has an open endpoint, so put the
server behind a reverse proxy or network boundary in that case. `/metrics`
accepts the shared token or any user's configured token.

Notes and limitations:

- Policies reload on `SIGHUP` per user, without restarting the server. Token
  rotation applies immediately because the middleware re-reads the policy; a
  reload that removes a user's only token locks that endpoint out (401) instead
  of opening it, until a token is restored or the process restarts.
- Drive webhooks fan out to all users (channel IDs are globally unique), and each
  channel's secret is verified against the user that owns the subscription.
- The outbound per-service limiter is initialized in multi-user mode with the
  **lowest** per-user `rate_limit_rpm` as its fallback; per-user
  `server.rate_limits` overrides are not merged. The pre-auth IP bucket is
  process-wide per client address, and the authenticated bucket reads each
  user's endpoint policy independently and is keyed on the credential.
- stdio mode remains single-user by design: one process, one policy.

Validate before deploying:

```bash
mcp-google-workspace check-policy gws-policy.json
```

It prints a service summary plus security warnings, and **exits non-zero when any
warning is present**, so CI can gate on it. Add `--verify` to test the credentials
against Google.

## Registry listings

The repository ships submission-ready manifests for the MCP registries:
`server.json` (official MCP Registry / GitHub MCP Registry format),
`glama.json` (Glama), and `registry/docker-mcp-catalog.yaml` (Docker MCP
Catalog — requires mirroring the image to Docker Hub, see the file
header). Submission status and directory checklist:
[`registry/SUBMISSIONS.md`](registry/SUBMISSIONS.md). A CI test keeps the
manifests in sync with the crate version.

## Claude Code Integration

Add to `.claude/settings.json`:

```json
{
  "mcpServers": {
    "google-workspace": {
      "command": "/path/to/mcp-google-workspace",
      "args": ["--policy", "/path/to/gws-policy.json"]
    }
  }
}
```

## Kubernetes Deployment

Manifests are in [`deploy/kubernetes/`](deploy/kubernetes/). Before applying:

1. **Replace the image digest.** The Deployment pins the image by digest, and the
   digest in the manifest is an all-zero **placeholder that fails closed** — the
   kubelet cannot pull it, so the pod sits in `ImagePullBackOff` until you replace
   it with the digest of a released image. Get it with
   `docker buildx imagetools inspect ghcr.io/fabiendupont/mcp-google-workspace:<version>`
   (or `crane digest …`) and pin
   `image: ghcr.io/fabiendupont/mcp-google-workspace:<version>@sha256:<digest>`.
   Keeping a mutable `:latest` tag next to a digest is *not* a pin. See
   [Verifying a release](https://fabiendupont.io/docs/getting-started/installation/#verifying-a-release).
2. **Replace the policy placeholders.** The shipped ConfigMap is the **live
   startup policy** — it is mounted at
   `/etc/mcp-google-workspace/policy.json` and passed with `--policy`, overriding
   the image command. Replace `<FOLDER_ID>` (and any calendar id) with real
   values; until then the placeholders match nothing and Drive writes fail
   closed. It ships with `server.read_only: true`. Only
   `constraints`/`denied_methods`/`allowed_resources` (and the other documented
   keys) are schema; `folders`, `calendars`, and `paths` are **not** keys and are
   silently dropped.
3. **Create the API token Secret.** The server refuses to start on its
   non-loopback bind without a token. The Deployment reads key `api-token` from
   Secret `mcp-gws-api-token`.
4. **Create the credentials Secret.** The Secret volume is mounted `0440` for
   root/fsGroup; the process runs as uid 1001 with fsGroup 1001, and the
   Deployment's `runAsUser` overrides the image's `USER`.
5. **Review the NetworkPolicy selectors.** Ingress is default-deny with only the
   labelled client namespace/pod rules; probes need no rule because
   node-originated traffic is always allowed under ingress isolation. Egress is
   restricted to DNS (UDP and TCP/53) and TCP/443 with link-local
   `169.254.0.0/16` and the RFC1918 ranges excluded. The policy is a no-op on a
   CNI that does not enforce NetworkPolicy (for example plain flannel).
6. **Keep `GWS_ALLOWED_HOSTS` in sync** with the Service DNS names clients use.
   Without them, rmcp rejects the request's `Host` header with `403`.
7. **Audit sink.** The Deployment passes `--audit-log /dev/stdout` by default, so
   JSONL records land in the pod log (`kubectl logs`) with no writable volume and
   work under `readOnlyRootFilesystem: true`. For a file instead, mount an
   `emptyDir` at `/var/log/mcp-gws` (contents are lost on restart). `--users-dir`
   also needs a writable per-user directory (an `emptyDir`), because a per-user
   audit-open failure is fatal.

```bash
kubectl create namespace mcp-google-workspace

# Bearer token for /mcp and /metrics (required)
kubectl create secret generic mcp-gws-api-token \
  --from-literal=api-token="$(openssl rand -hex 32)" \
  -n mcp-google-workspace

# Google credentials
kubectl create secret generic mcp-gws-credentials \
  --from-file=credentials.json=./your-credentials.json \
  -n mcp-google-workspace

# Deploy
kubectl apply -k deploy/kubernetes/
```

The shipped `ServiceMonitor` authenticates its scrape with the same
`mcp-gws-api-token` Secret, because `/metrics` now requires the token. Liveness,
readiness, and health probes stay unauthenticated. Each user gets their own
deployment with their own credentials Secret and policy ConfigMap. See
[docs/architecture.md](docs/architecture.md) for the multi-user deployment
pattern.

### Endpoints

| Path | Purpose | Auth |
|------|---------|------|
| `POST /mcp` | MCP JSON-RPC endpoint | Bearer token when configured |
| `GET /mcp` | SSE notification stream | Bearer token when configured |
| `GET /healthz` | Health check | Open |
| `GET /readyz` | Readiness (discovery docs loaded) | Open |
| `GET /livez` | Liveness (process alive) | Open |
| `GET /metrics` | Prometheus metrics | Bearer token when configured |
| `POST /webhooks/drive` | Drive push notifications | `X-Goog-Channel-Token` channel secret (Google push target; not bearer-authenticated) |

## Observability

- **Structured logging** via `tracing` (stderr, `RUST_LOG` for filtering)
- **OpenTelemetry traces** via OTLP/HTTP (set `OTEL_EXPORTER_OTLP_ENDPOINT`)
- **Prometheus metrics** at `/metrics` (request count, latency, errors, active tasks)

## How It Works

The server is **discovery-driven**: it fetches Google Discovery Documents at
runtime to learn each API's resources, methods, and parameters. No hardcoded API
list — new endpoints appear automatically.

Each enabled Google service is exposed as a single MCP tool. The agent specifies
`resource` and `method` as arguments:

```json
{
  "name": "drive",
  "arguments": {
    "resource": "files",
    "method": "list",
    "params": { "q": "mimeType='application/pdf'", "fields": "files(id,name)" }
  }
}
```

A `gws_discover` meta-tool lets the agent introspect available resources,
methods, and parameter schemas before making calls.

## License

Apache-2.0
