+++
title = "Deployment guide"
description = "Run locally, in containers, or on Kubernetes"
date = 2026-06-12T00:00:00+00:00
updated = 2026-11-18T00:00:00+00:00
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

Claude Code starts the binary as a child process, communicates over stdin/stdout, and shuts it down on exit. The `--eager-tools` flag loads every helper tool **the policy permits** at startup (helper tools for services absent from the policy are filtered out, exactly as in lazy mode). Without it, only `gws_discover` and `gws_batch` are available initially.

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
export GWS_API_TOKEN="$(openssl rand -hex 32)"

podman run -p 3000:3000 \
  -e GWS_API_TOKEN="$GWS_API_TOKEN" \
  -v ./policy.json:/etc/mcp-google-workspace/policy.json:ro,Z \
  -v ./credentials.json:/etc/mcp-google-workspace/credentials.json:ro,Z \
  ghcr.io/fabiendupont/mcp-google-workspace:0.9.1
```

> On Fedora and RHEL with SELinux, the `:Z` flag is required for bind mounts.

The image is `FROM scratch` -- about 6 MB, no shell, no OS packages. Available for `linux/amd64` and `linux/arm64`.

The container command binds to `0.0.0.0:3000`, so the server refuses to start
unless a bearer token is configured. `GWS_API_TOKEN` (or `--api-token`) provides
it; clients must send `Authorization: Bearer <token>` to `/mcp` and `/metrics`.
Passing `--insecure-no-auth` disables the requirement and should only be used
when the port is reachable exclusively by trusted clients (for example a local
podman network with no published port).

To enable eager tools and prompts in the container, pass additional arguments:

```bash
podman run -p 3000:3000 \
  -e GWS_API_TOKEN="$GWS_API_TOKEN" \
  -v ./policy.json:/etc/mcp-google-workspace/policy.json:ro,Z \
  -v ./credentials.json:/etc/mcp-google-workspace/credentials.json:ro,Z \
  -v ./prompts:/etc/mcp-google-workspace/prompts:ro,Z \
  ghcr.io/fabiendupont/mcp-google-workspace:0.9.1 \
  --eager-tools --prompts-dir /etc/mcp-google-workspace/prompts
```

### Building locally

```bash
podman build -t mcp-google-workspace:local .
```

The Dockerfile uses UBI 10 as the builder with system Rust and `glibc-static` for static linking.

## Kubernetes

Two options deploy the same workload: the Helm chart in
[`charts/mcp-google-workspace/`](https://github.com/fabiendupont/mcp-google-workspace/tree/main/charts/mcp-google-workspace)
(recommended) and the plain manifests in
[`deploy/kubernetes/`](https://github.com/fabiendupont/mcp-google-workspace/tree/main/deploy/kubernetes).
CI compares the two renderings, so they cannot drift apart.

### Helm chart

The chart is published as a cosign-signed OCI artifact with every release.

```bash
# Digest of the release you are deploying (never a tag)
DIGEST="$(docker buildx imagetools inspect \
  ghcr.io/fabiendupont/mcp-google-workspace:<version> | awk '/^Digest:/{print $2}')"

helm install mcp-google-workspace \
  oci://ghcr.io/fabiendupont/charts/mcp-google-workspace --version <version> \
  --namespace mcp-google-workspace --create-namespace \
  --set image.digest="$DIGEST" \
  --set auth.existingSecret=mcp-gws-api-token \
  --set credentials.existingSecret=mcp-gws-credentials \
  --set-file policy.raw=./gws-policy.json
```

The install fails at render time — naming the missing value — without an image
digest, a bearer token or a credentials Secret. `policy.raw` must be a
JSON object and is parsed during rendering; use `policy.document` for a
structured override or `policy.existingConfigMap` for a ConfigMap you manage.
The default policy is the fail-closed one from the plain manifests: read-only,
with a `<FOLDER_ID>` placeholder that matches nothing.

```bash
cosign verify ghcr.io/fabiendupont/charts/mcp-google-workspace:<version> \
  --certificate-identity-regexp '^https://github\.com/fabiendupont/mcp-google-workspace/\.github/workflows/(release|chart)\.yml@refs/tags/v[0-9].*$' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

See the chart
[README](https://github.com/fabiendupont/mcp-google-workspace/tree/main/charts/mcp-google-workspace)
for the values table. The `restricted` Pod Security labels belong on the
namespace, which Helm does not create.

### Plain manifests

The manifests in [`deploy/kubernetes/`](https://github.com/fabiendupont/mcp-google-workspace/tree/main/deploy/kubernetes)
apply without Helm.

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

kubectl apply -k deploy/kubernetes/
```

Before applying, review the manifests:

- **Image digest.** The Deployment line looks like
  `image: ghcr.io/fabiendupont/mcp-google-workspace@sha256:0000…`. The
  all-zero digest is a **placeholder and fails closed**: the kubelet cannot pull
  it, so the pod stays in `ImagePullBackOff` until it is replaced with the digest
  of a released image. Get it with
  `docker buildx imagetools inspect ghcr.io/fabiendupont/mcp-google-workspace:<version>`
  (or `crane digest ghcr.io/fabiendupont/mcp-google-workspace:<version>`) and
  substitute it. Pin the digest alone, or `:<version>@sha256:<digest>`: keeping a
  mutable tag such as `:latest` next to a digest is *not* a pin, because the tag
  can be re-pointed at different bytes.
- **The ConfigMap is the live startup policy.** It is mounted at
  `/etc/mcp-google-workspace/policy.json` and passed to the process with
  `--policy`, overriding the image command; it is not an example. It ships with
  `"read_only": true`. Replace the placeholder `<FOLDER_ID>` (and any calendar
  id) with real values — until then they match nothing, so Drive writes fail
  closed. Only the documented keys are schema: `folders`, `calendars`, and
  `paths` are **not** keys and are silently dropped (the parser does not reject
  unknown fields), which is how an earlier revision shipped with no scoping at
  all. Validate an extracted copy with `check-policy` before re-applying.
- **API token.** The Deployment reads key `api-token` from Secret
  `mcp-gws-api-token` and passes it as `GWS_API_TOKEN`. The pod binds to a
  non-loopback address, so it will not start without it.
- **`GWS_ALLOWED_HOSTS`.** Keep it in sync with the Service DNS names (and any
  ingress hostname) that clients use. rmcp rejects an unknown `Host` with `403`.
- **NetworkPolicy.** Ingress is default-deny with only the labelled client
  namespace/`podSelector` rules (plus the monitoring namespace for scraping).
  There is no node-CIDR rule: kubelet probes originate from the node and are
  always allowed under ingress isolation, so adding an `ipBlock` for them would
  also admit every pod on that CIDR. Egress is default-deny and limited to DNS
  (UDP and TCP/53) and TCP/443 with link-local `169.254.0.0/16` and the RFC1918
  ranges excluded, so the pod cannot reach in-cluster services, the node network,
  or the cloud metadata endpoint. The `except:` list is honoured by Calico,
  Cilium, and Antrea. A NetworkPolicy is enforced by the CNI, not by Kubernetes:
  on a CNI without NetworkPolicy support (for example plain flannel) the object
  is accepted but has **no effect**, so verify your CNI before relying on it.
- **Pod Security.** The namespace carries
  `pod-security.kubernetes.io/enforce: restricted` (plus `audit` and `warn`
  labels). The shipped pod already satisfies the profile.
- **Audit sink.** The Deployment passes `--audit-log /dev/stdout` by default, so
  JSONL records land in the pod log (`kubectl logs -n mcp-google-workspace
  deploy/mcp-google-workspace`) and need no writable volume under
  `readOnlyRootFilesystem: true`. To write a file instead, mount an `emptyDir` at
  `/var/log/mcp-gws` (with a `sizeLimit`) and pass that path — its contents are
  lost on pod restart. `--users-dir` likewise needs a writable per-user
  directory (an `emptyDir`), because a per-user audit-open failure is fatal.
- **Volume ownership.** The credentials Secret volume is mounted `defaultMode:
  0440` (root:fsGroup). The container runs as uid/gid 1001 with fsGroup 1001, and
  the Deployment's `runAsUser: 1001` overrides the image's `USER` directive; any
  writable volume you add for `--users-dir` must be writable by 1001 (an
  `emptyDir` is, via fsGroup).

### What gets created

| Resource | Purpose |
|----------|---------|
| Namespace | `mcp-google-workspace`, Pod Security `restricted` |
| ConfigMap | Policy JSON — the live startup policy |
| Secret `mcp-gws-api-token` | Bearer token for `/mcp` and `/metrics` |
| Secret `mcp-gws-credentials` | Google service-account credentials (mounted `0440`) |
| Deployment | Server pod (non-root, read-only filesystem, drop all caps, `--audit-log /dev/stdout`) |
| Service | ClusterIP on port 3000 |
| NetworkPolicy | Default-deny ingress from labelled clients and monitoring; egress limited to DNS and TCP/443 |
| ServiceMonitor | Prometheus scraping at `/metrics` every 30s, authenticated with the bearer token |

To verify the release artifacts before pinning the digest, see
[Verifying a release](../getting-started/installation/#verifying-a-release).

### Probes

| Probe | Path | Behavior |
|-------|------|----------|
| Liveness | `/livez` | Always 200. Failure means unresponsive. |
| Readiness | `/readyz` | 200 after Discovery Docs loaded, 503 during startup. |

### Multi-user

Run one process with `--users-dir` to serve several users from a single
Deployment. Each subdirectory holds that user's policy and audit log, and each
user gets their own MCP endpoint (`/mcp/alice`, `/mcp/bob`, ...).

Authentication in multi-user mode:

- A shared `--api-token` (or `GWS_API_TOKEN`) applies to every user endpoint and
  overrides per-policy tokens.
- Alternatively, each user's policy can set `server.api_token`; the middleware
  re-reads the policy, so rotated tokens take effect immediately on `SIGHUP`.
- The process refuses to start on a non-loopback bind unless every user has a
  token or a shared token is set. `--insecure-no-auth` is the explicit opt-out;
  a user without any token then has an open endpoint, so require a reverse proxy
  or network boundary.
- `/metrics` accepts the shared token or any user's configured token.
- Authenticated requests are rate-limited per user endpoint, keyed on the
  presented credential and using that user's `rate_limit_rpm`; the
  pre-authentication IP bucket is fixed (~60/min) and shared per client address.
  The outbound per-service limiter uses the lowest configured `rate_limit_rpm` as
  its fallback.

For stronger isolation, one Deployment per user with its own ConfigMap and
credentials Secret is still supported and avoids sharing a process. See
[Architecture](../../architecture/request-flow/) for the multi-user diagram.
