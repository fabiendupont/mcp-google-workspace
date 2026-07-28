+++
title = "Deployment guide"
description = "Run locally, in containers, or on Kubernetes"
date = 2026-06-12T00:00:00+00:00
updated = 2026-07-28T00:00:00+00:00
draft = false
weight = 10
template = "docs/page.html"
[extra]
lead = "Three deployment models: Claude Code (stdio), container (HTTP), and Kubernetes."
toc = true
top = false
+++

## Claude Code (stdio)

Add to `.claude/settings.json`:

```json
{
  "mcpServers": {
    "google-workspace": {
      "command": "/path/to/mcp-google-workspace",
      "args": ["--policy", "/path/to/policy.json", "--eager-tools"]
    }
  }
}
```

Claude Code starts the binary as a child process, communicates over stdin/stdout, and shuts it down on exit. The `--eager-tools` flag loads all 53 helper tools at startup. Without it, only `gws_discover` and `gws_batch` are available initially.

To load MCP prompts (workflow recipes), add `--prompts-dir`:

```json
{
  "mcpServers": {
    "google-workspace": {
      "command": "/path/to/mcp-google-workspace",
      "args": ["--policy", "/path/to/policy.json", "--eager-tools", "--prompts-dir", "/path/to/prompts"]
    }
  }
}
```

## Container (HTTP)

```bash
podman run -p 3000:3000 \
  -v ./policy.json:/etc/mcp-google-workspace/policy.json:ro,Z \
  -v ./credentials.json:/etc/mcp-google-workspace/credentials.json:ro,Z \
  ghcr.io/fabiendupont/mcp-google-workspace:0.7.0
```

> On Fedora and RHEL with SELinux, the `:Z` flag is required for bind mounts.

The image is `FROM scratch` -- about 6 MB, no shell, no OS packages. Available for `linux/amd64` and `linux/arm64`.

To enable eager tools and prompts in the container, pass additional arguments:

```bash
podman run -p 3000:3000 \
  -v ./policy.json:/etc/mcp-google-workspace/policy.json:ro,Z \
  -v ./credentials.json:/etc/mcp-google-workspace/credentials.json:ro,Z \
  -v ./prompts:/etc/mcp-google-workspace/prompts:ro,Z \
  ghcr.io/fabiendupont/mcp-google-workspace:0.7.0 \
  --eager-tools --prompts-dir /etc/mcp-google-workspace/prompts
```

### Building locally

```bash
podman build -t mcp-google-workspace:local .
```

The Dockerfile uses UBI 10 as the builder with system Rust and `glibc-static` for static linking.

## Kubernetes

Manifests are in [`deploy/kubernetes/`](https://github.com/fabiendupont/mcp-google-workspace/tree/main/deploy/kubernetes).

```bash
kubectl create namespace mcp-google-workspace
kubectl create secret generic mcp-gws-credentials \
  --from-file=credentials.json=./your-credentials.json \
  -n mcp-google-workspace
kubectl apply -k deploy/kubernetes/
```

### What gets created

| Resource | Purpose |
|----------|---------|
| Namespace | `mcp-google-workspace` |
| ConfigMap | Policy JSON |
| Deployment | Server pod (non-root, read-only filesystem, drop all caps) |
| Service | ClusterIP on port 3000 |
| ServiceMonitor | Prometheus scraping at `/metrics` every 30s |

### Probes

| Probe | Path | Behavior |
|-------|------|----------|
| Liveness | `/livez` | Always 200. Failure means unresponsive. |
| Readiness | `/readyz` | 200 after Discovery Docs loaded, 503 during startup. |

### Multi-user

Each user gets their own Deployment, ConfigMap, and Secret. No shared state. See [Architecture](../../architecture/request-flow/) for the multi-user diagram.
