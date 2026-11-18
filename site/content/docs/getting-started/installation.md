+++
title = "Installation"
description = "Install from container or source"
date = 2026-06-12T00:00:00+00:00
updated = 2026-11-18T00:00:00+00:00
draft = false
weight = 10
template = "docs/page.html"
[extra]
lead = "Get the binary or container image."
toc = true
top = false
+++

## Container image (recommended)

Available on GitHub Container Registry for `linux/amd64` and `linux/arm64`:

```bash
podman pull ghcr.io/fabiendupont/mcp-google-workspace:0.9.2
```

The image is `FROM scratch` — only the static binary and CA certificates. About 6 MB.

Pin the reference. A version tag can be re-pointed; a digest cannot:

```bash
podman pull ghcr.io/fabiendupont/mcp-google-workspace:0.9.2@sha256:<digest>
```

Get the digest for a release with
`docker buildx imagetools inspect ghcr.io/fabiendupont/mcp-google-workspace:0.9.2`
or `crane digest ghcr.io/fabiendupont/mcp-google-workspace:0.9.2`. Never deploy
`:latest` — deployment integrity depends on the image reference being immutable.
Verify the artifact first: see
[Verifying a release](#verifying-a-release).

The image's default command starts the server in HTTP mode on `0.0.0.0:3000`, so
it requires a bearer token at startup (`GWS_API_TOKEN` or `--api-token`). See
the [deployment guide](../../deployment/guide/) for a complete `podman run`
invocation.

## From source

```bash
git clone https://github.com/fabiendupont/mcp-google-workspace.git
cd mcp-google-workspace
cargo build --release
```

The binary is at `target/release/mcp-google-workspace`.

## Verify

```bash
mcp-google-workspace --help
```

## Verifying a release

Releases are built from an **immutable commit SHA**, not from a tag name. The
workflow enforces an annotated `v*` tag and fails if the tag is lightweight or
has moved (the tagged commit must equal the built SHA). Assets include the static
binaries (`mcp-google-workspace-linux-amd64`,
`mcp-google-workspace-linux-arm64`), `SHA256SUMS`, the keyless cosign signature
(`SHA256SUMS.sig` + `SHA256SUMS.pem`), per-architecture CycloneDX SBOMs
(`sbom-linux-amd64`, `sbom-linux-arm64`), and the Helm chart
`mcp-google-workspace-<version>.tgz` with `SHA256SUMS.chart` plus its
`.sig`/`.pem`.

Download the files into one directory, then verify the checksums:

```bash
sha256sum -c SHA256SUMS
```

Verify that `SHA256SUMS` was signed by this repository's release workflow, on a
tag, by GitHub Actions:

```bash
cosign verify-blob SHA256SUMS \
  --signature SHA256SUMS.sig \
  --certificate SHA256SUMS.pem \
  --certificate-identity-regexp 'https://github.com/fabiendupont/mcp-google-workspace/.github/workflows/release.yml@refs/tags/v.*' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

Only a binary whose hash matches a verified `SHA256SUMS` line is the release you
reviewed.

The Helm chart is published both as a release asset and as an OCI artifact at
`oci://ghcr.io/fabiendupont/charts/mcp-google-workspace`. Verify the artifact
before installing it:

```bash
cosign verify ghcr.io/fabiendupont/charts/mcp-google-workspace:<version> \
  --certificate-identity-regexp '^https://github\.com/fabiendupont/mcp-google-workspace/\.github/workflows/(release|chart)\.yml@refs/tags/v[0-9].*$' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

Images are published as immutable per-version tags (`:<version>`) plus a manifest
digest. `:latest` exists only as a convenience alias, moved by a separate job that
asserts it points at the same digest — it is mutable, so pin `:<version>` or,
better, the digest. The publish job refuses to ship unscanned images: trivy runs
per architecture on the image digest with `severity: CRITICAL,HIGH`,
`ignore-unfixed`, and `exit-code 1`, and the SBOMs above are attached to the
release.

Get the digest for a version:

```bash
docker buildx imagetools inspect ghcr.io/fabiendupont/mcp-google-workspace:<version>
# or
crane digest ghcr.io/fabiendupont/mcp-google-workspace:<version>
```
