# Registry & Directory Submissions

This repository ships the manifests needed to list the server on the major
MCP registries and directories. This file tracks what goes where and the
status of each submission.

## Manifests in this repository

| File | Consumed by | Notes |
|---|---|---|
| `server.json` (root) | Official MCP Registry, GitHub MCP Registry | Standard `server.json` manifest (2025-12-11 schema). Reverse-DNS name, OCI package pointing at the ghcr.io multi-arch image, stdio transport. Update `version` and `packages[].version` on each release. |
| `glama.json` (root) | Glama | Declares maintainers; Glama scans the repo for the rest. |
| `registry/docker-mcp-catalog.yaml` | Docker MCP Catalog | Payload for a PR to `docker/mcp-registry` (`servers/mcp-google-workspace/metadata.yaml`). Requires mirroring the ghcr.io image to a Docker Hub namespace (command in the file header). |

## Submission checklist

- [ ] **Official MCP Registry** — PR `server.json` to
      `modelcontextprotocol/registry` following its contributing guide.
- [ ] **GitHub MCP Registry** — PR to `github/mcp-registry`
      (`servers/fabiendupont/mcp-google-workspace.json`), same manifest
      content as root `server.json`.
- [ ] **Glama** — `glama.json` is in place; submit the repo URL at
      https://glama.ai/register.
- [ ] **Docker MCP Catalog** — mirror image to Docker Hub (see payload
      header), then PR `registry/docker-mcp-catalog.yaml` to
      `docker/mcp-registry`.
- [ ] **PulseMCP** — submit https://www.pulsemcp.com/submit
- [ ] **mcp.so** — submit https://mcp.so/submit (listings reviewed)
- [ ] **MCPServers.org** — submit via their free queue
- [ ] **awesome-mcp-servers** — PR to
      `punkpeye/awesome-mcp-servers`
- [ ] **crates.io** — `cargo publish` (name `mcp-google-workspace`); the
      `server.json` packages list can then gain a `cargo`-installable
      entry or a GitHub-releases binary entry.
- [ ] **Smithery** — evaluate: their paid plans and proxy/CLI ended
      October 2026; decide whether the listing is still worth it.

## Positioning (use for directory descriptions)

Safety-gated Google Workspace access for AI agents: curated tools for
Gmail, Drive, Calendar, Docs, Sheets, Slides, and Tasks, plus
auto-generated tools for every other Google API via Discovery Documents —
behind a declarative policy engine (per-folder/per-recipient constraints,
method denylists, read-only modes), optional human approval for writes
(MCP elicitation), JSONL audit logs, and single-process multi-user HTTP
deployment with per-user credentials and bearer authentication.
