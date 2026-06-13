+++
title = "Installation"
description = "Install from container or source"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-12T00:00:00+00:00
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
podman pull ghcr.io/fabiendupont/mcp-google-workspace:0.1.0
```

The image is `FROM scratch` — only the static binary and CA certificates. About 6 MB.

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
