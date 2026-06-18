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
A JSON policy file scopes what each project can access — folder-level Drive
ACLs, per-calendar permissions, method denylists, and global read-only mode.

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
- **Folder-level Drive ACLs**: Per-folder `read-only` or `read-write` access, enforced
  on queries, body `parents`, and `addParents`/`removeParents` params.
- **Per-calendar permissions**: Per-calendar access levels. Unknown calendar IDs rejected.
- **Method denylists**: Block specific methods (e.g., `messages.delete`).
- **Read-only mode**: Global or per-service.
- **Rate limiting**: Per-client-IP sliding window (HTTP transport).
- **Request size limit**: Configurable max request body size.
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

Create a `gws-policy.json` to scope agent access. See
[`policy.example.json`](policy.example.json) for a full example.

```json
{
  "server": {
    "read_only": false,
    "rate_limit_rpm": 120
  },
  "services": [
    {
      "name": "drive",
      "folders": [
        { "path": "Projects/current-project", "access": "read-write" }
      ]
    },
    {
      "name": "gmail",
      "denied_methods": ["messages.delete", "messages.trash"]
    },
    {
      "name": "calendar",
      "calendars": [
        { "id": "primary", "access": "read-write" }
      ]
    }
  ]
}
```

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
