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
API call is made:

- **Service allow-list**: Only listed services are exposed. Everything else is denied.
- **Parameter constraints**: Generic allowlists on any API parameter with per-value access levels.
- **Method denylists**: Block specific methods (e.g., `messages.delete`).
- **Read-only mode**: Global or per-service.
- **Request explanation**: Write operations include a plain-English description of what they do.
- **Audit log**: Structured JSONL trail of every API call, allowed and denied.
- **Live reload**: SIGHUP reloads the policy file without restart (HTTP transport).
- **Rate limiting**: Per-client-IP sliding window (HTTP transport).
- **Origin validation**: Configurable allowlist (default: localhost only).

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

```bash
podman run -p 3000:3000 \
  -v ./gws-policy.json:/etc/mcp-google-workspace/policy.json:ro \
  -v ./credentials.json:/secrets/credentials.json:ro \
  -e GOOGLE_APPLICATION_CREDENTIALS=/secrets/credentials.json \
  ghcr.io/fabiendupont/mcp-google-workspace:latest
```

### From Source

```bash
cargo build --release
./target/release/mcp-google-workspace --policy gws-policy.json
```

### HTTP Transport

```bash
./target/release/mcp-google-workspace --policy gws-policy.json --http 127.0.0.1:3000
```

## Policy File

Use a template for quick setup:

```bash
mcp-google-workspace init --template assistant > gws-policy.json
```

Available templates: `analyst`, `assistant`, `admin-readonly`, `content-writer`, `it-admin`.
Use `mcp-google-workspace init --template list` to see descriptions.

Or run the interactive wizard:

```bash
mcp-google-workspace init
```

See [`policy.example.json`](policy.example.json) for a full example with constraints.

```json
{
  "server": { "read_only": false, "confirm_writes": false },
  "services": [
    {
      "name": "drive",
      "constraints": [
        { "param": "parents", "values": ["folder-id"], "access": "read-write", "location": "body" }
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

### Write confirmation (human in the loop)

Set `"confirm_writes": true` in the `server` section to require explicit user
approval before any mutating operation. The server sends an MCP elicitation
request describing the operation (tool name plus identifying arguments like
recipients, titles, or IDs); writes proceed only after approval. Read-only
tools are never interrupted, `dry_run` calls skip confirmation, and writes are
denied (rather than silently allowed) if the client cannot display approvals.
`gws_batch` is disabled in this mode so every write goes through an
individually confirmable call.

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
  `notifications/resources/updated`.
- Without webhook support (stdio mode, or HTTP without `--external-url`),
  the server falls back to **polling**: it re-reads the resource on a
  schedule and emits `notifications/resources/updated` whenever its
  content changes. This works for any service with a `get` method, not
  just Drive.

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
mcp-google-workspace --http 0.0.0.0:3000 --users-dir /srv/gws-users
```

Each user's agent connects to its own endpoint: `http://host:3000/mcp/alice`,
`http://host:3000/mcp/bob`, and so on. Directory names must be URL-safe
(letters, digits, `-`, `_`).

**Per-user authentication**: set `server.api_token` in a user's policy to
require `Authorization: Bearer <token>` on that user's endpoint:

```json
{ "server": { "api_token": "a-long-random-secret" }, "services": [ ... ] }
```

Requests with a missing or wrong token get `401 Unauthorized`. Users
without an `api_token` have open endpoints — put the server behind a
reverse proxy or network boundary in that case.

Notes and limitations:

- Policies reload on `SIGHUP` per user, without restarting the server.
- Drive webhooks fan out to all users (channel IDs are globally unique).
- Per-policy `rate_limit_rpm` is not enforced across users in this mode
  (the limiter is process-global in single-user mode only).
- stdio mode remains single-user by design: one process, one policy.

Validate before deploying:

```bash
mcp-google-workspace --check-policy gws-policy.json
```

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

Manifests are in [`deploy/kubernetes/`](deploy/kubernetes/). Deploy with:

```bash
# Create the credentials secret first
kubectl create secret generic mcp-gws-credentials \
  --from-file=credentials.json=./your-credentials.json \
  -n mcp-google-workspace

# Deploy
kubectl apply -k deploy/kubernetes/
```

Each user gets their own deployment with their own credentials Secret and policy
ConfigMap. See [docs/architecture.md](docs/architecture.md) for the multi-user
deployment pattern.

### Endpoints

| Path | Purpose |
|------|---------|
| `POST /mcp` | MCP JSON-RPC endpoint |
| `GET /mcp` | SSE notification stream |
| `GET /healthz` | Health check |
| `GET /readyz` | Readiness (discovery docs loaded) |
| `GET /livez` | Liveness (process alive) |
| `GET /metrics` | Prometheus metrics |

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
