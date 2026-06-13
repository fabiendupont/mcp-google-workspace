+++
title = "Common issues"
description = "Troubleshooting guide"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-12T00:00:00+00:00
draft = false
weight = 10
template = "docs/page.html"
[extra]
lead = "Authentication errors, policy errors, and container issues."
toc = true
top = false
+++

## Authentication errors

**"invalid_client"** — The client_secret is truncated. Use `gws auth export --unmasked`.

**"No credentials found"** — Check the [credential chain](../../security/credential-chain/).

**"permission to use project"** — Set `project_id` in policy to match your GCP project.

## Policy errors

**"Service not enabled"** — Add the service to `[[services]]` in your policy.

**"parents must be specified"** — Include `parents` in the request body for Drive writes.

## Container errors

**"Permission denied"** — On SELinux systems, use `:Z` flag on volume mounts.
