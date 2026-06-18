+++
title = "Deployment guide"
description = "Run locally, in containers, or on Kubernetes"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-12T00:00:00+00:00
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
      "args": ["--policy", "/path/to/policy.json"]
    }
  }
}
```

Claude Code starts the binary as a child process, communicates over stdin/stdout, and shuts it down on exit.

## Container (HTTP)

```bash
podman run -p 3000:3000 \
  -v ./policy.json:/etc/mcp-google-workspace/policy.json:ro,Z \
  -v ./credentials.json:/etc/mcp-google-workspace/credentials.json:ro,Z \
  ghcr.io/fabiendupont/mcp-google-workspace:0.1.0
```

> On Fedora and RHEL with SELinux, the `:Z` flag is required for bind mounts.

The image is `FROM scratch` — about 6 MB, no shell, no OS packages. Available for `linux/amd64` and `linux/arm64`.

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
| ConfigMap | Policy TOML |
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
