# mcp-google-workspace

A Helm chart for the `mcp-google-workspace` MCP server: Drive, Gmail and Calendar
over the Model Context Protocol, with a startup policy that scopes every tool
call.

The chart deploys the same workload as the plain manifests in
[`deploy/kubernetes/`](../../deploy/kubernetes): one Deployment, one Service, the
policy ConfigMap, a NetworkPolicy, and optionally a ServiceMonitor and a
PodDisruptionBudget. CI compares the two renderings
([`scripts/chart-drift.py`](../../scripts/chart-drift.py)), so neither can drift
without a failing build.

## Requirements

- Kubernetes 1.25+ (the pod satisfies the `restricted` Pod Security profile).
- A released image **digest**. The image is never referenced by tag.
- A Google service-account JSON key in a Secret.
- A bearer token in a Secret (or, discouraged, inline).

The chart refuses to render without the digest, the token or the credentials
Secret, with an error naming the missing value.

## Install

```bash
# 1. Secrets (create them once)
kubectl create namespace mcp-google-workspace
kubectl -n mcp-google-workspace create secret generic mcp-gws-api-token \
  --from-literal=api-token="$(openssl rand -hex 32)"
kubectl -n mcp-google-workspace create secret generic mcp-gws-credentials \
  --from-file=credentials.json=./your-credentials.json

# 2. Digest of the release you are deploying
DIGEST="$(docker buildx imagetools inspect \
  ghcr.io/fabiendupont/mcp-google-workspace:<version> | awk '/^Digest:/{print $2}')"

# 3. Install
helm install mcp-google-workspace \
  oci://ghcr.io/fabiendupont/charts/mcp-google-workspace --version <version> \
  --namespace mcp-google-workspace \
  --set image.digest="$DIGEST" \
  --set auth.existingSecret=mcp-gws-api-token \
  --set credentials.existingSecret=mcp-gws-credentials \
  --set-file policy.raw=./gws-policy.json
```

`policy.raw` must be a JSON object; it is parsed during rendering, so malformed
JSON fails the install instead of the server. Alternatively pass a structured
`-f my-values.yaml` with `policy.document`, or point at a ConfigMap you manage
with `policy.existingConfigMap`.

The default `policy.document` is the fail-closed policy from
`deploy/kubernetes/configmap.yaml`: read-only, with a `<FOLDER_ID>` placeholder
that matches nothing. Replace it before expecting a Drive write to succeed:

```bash
kubectl -n mcp-google-workspace get cm mcp-google-workspace-policy \
  -o jsonpath='{.data.policy\.json}' > policy.json
# edit <FOLDER_ID>, then validate before applying
mcp-google-workspace check-policy policy.json
```

## Verify the chart artifact

The chart is published to `oci://ghcr.io/fabiendupont/charts` as part of the
release workflow on the version tag, and the same bytes (plus `SHA256SUMS`,
`.sig` and `.pem`) are attached to the GitHub release. Both are signed with
cosign keyless:

```bash
cosign verify ghcr.io/fabiendupont/charts/mcp-google-workspace:<version> \
  --certificate-identity-regexp '^https://github\.com/fabiendupont/mcp-google-workspace/\.github/workflows/(release|chart)\.yml@refs/tags/v[0-9].*$' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

## Values

| Key | Default | Description |
|---|---|---|
| `replicaCount` | `1` | Pods. The policy is per-process state; scale only for read-heavy use. |
| `image.repository` | `ghcr.io/fabiendupont/mcp-google-workspace` | Image repository. |
| `image.digest` | `""` (required) | `sha256:…` manifest digest. Tags are rejected. |
| `image.pullPolicy` | `""` | Empty means the kubelet default (`IfNotPresent` for digests). |
| `imagePullSecrets` | `[]` | Pull secrets for a private mirror. |
| `auth.existingSecret` | `""` | Secret holding the bearer token. |
| `auth.secretKey` | `api-token` | Key inside that Secret. |
| `auth.token` | `""` | Inline token, stored in the Helm release Secret. Discouraged. |
| `credentials.existingSecret` | `""` (required) | Secret with the Google service-account JSON. |
| `credentials.secretKey` | `credentials.json` | Key inside that Secret. |
| `policy.existingConfigMap` | `""` | ConfigMap with a `policy.json` key. Nothing is rendered. |
| `policy.configMapName` | `<fullname>-policy` | Name of the chart-managed ConfigMap. |
| `policy.raw` | `""` | JSON string (`--set-file`), parsed and validated at render time. |
| `policy.document` | fail-closed policy | Structured policy; maps merge, lists replace. |
| `config.rustLog` | `info` | `RUST_LOG`. |
| `config.extraAllowedHosts` | `[]` | Extra `Host` headers for rmcp, e.g. an ingress name. |
| `config.allowedHosts` | `""` | Replaces the generated `GWS_ALLOWED_HOSTS` entirely. |
| `config.clusterDomain` | `cluster.local` | Cluster DNS suffix for the generated hosts. |
| `config.extraEnv` | `[]` | Extra container env entries. |
| `audit.logPath` | `/dev/stdout` | `--audit-log` target; JSONL in the pod log by default. |
| `extraArgs` | `[]` | Extra container arguments. |
| `containerPort` | `3000` | Container, probe and Service target port. |
| `resources` | 50m/32Mi → 500m/128Mi | Requests and limits. |
| `podSecurityContext` | uid/gid 1001, fsGroup 1001 | Pod-level security context. |
| `securityContext` | read-only root, no caps | Container-level security context. |
| `service.type` / `service.port` | `ClusterIP` / `3000` | Service shape. |
| `metrics.annotations` | `true` | Prometheus scrape annotations on the pod. |
| `metrics.serviceMonitor.enabled` | `false` | Requires the Prometheus Operator CRDs. |
| `networkPolicy.enabled` | `true` | Ingress/egress default-deny plus explicit rules. |
| `networkPolicy.ingress.clientPeers` | `mcp-clients` + `mcp-client` | Peers allowed to reach the HTTP port. |
| `networkPolicy.ingress.monitoring` | `monitoring` namespace | Prometheus scrape rule. |
| `networkPolicy.egress.https.except` | link-local + RFC1918 | Destinations excluded from the 443 egress rule. |
| `podDisruptionBudget.enabled` | `false` | Only useful with more than one replica. |
| `serviceAccount.create` | `false` | No Kubernetes API access is needed. |
| `nodeSelector`, `tolerations`, `affinity`, `topologySpreadConstraints` | `{}` / `[]` | Scheduling. |
| `extraVolumes`, `extraVolumeMounts` | `[]` | For a file audit log or `--users-dir`. |

## Notes

- The `restricted` Pod Security labels live on the *namespace*, which Helm does
  not create by default. Label it yourself, or add the labels with
  `--create-namespace` plus a namespace manifest.
- A NetworkPolicy is enforced by the CNI, not by Kubernetes. On a CNI without
  NetworkPolicy support it has no effect.
- `kubectl port-forward` bypasses the NetworkPolicy and is subject to the same
  bearer token.
