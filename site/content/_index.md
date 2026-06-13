+++
title = "MCP Google Workspace"

[extra]
lead = 'Give AI agents <b>controlled access</b> to Google Workspace APIs. A TOML policy file scopes what each agent can access — folder-level Drive ACLs, per-calendar permissions, method denylists, and read-only mode.'
url = "/docs/getting-started/quickstart/"
url_button = "Get started"
repo_version = "v0.1.0"
repo_license = "Open-source Apache-2.0 License."
repo_url = "https://github.com/fabiendupont/mcp-google-workspace"

[[extra.menu.main]]
name = "Docs"
section = "docs"
url = "/docs/getting-started/quickstart/"
weight = 10

[[extra.list]]
title = "Discovery-driven"
content = "Fetches Google Discovery Documents at runtime. No hardcoded API list. New endpoints appear automatically."

[[extra.list]]
title = "Policy-as-code"
content = "A single TOML file controls everything: which services, which folders, which calendars, which methods."

[[extra.list]]
title = "Production-ready"
content = "Prometheus metrics, OpenTelemetry traces, Kubernetes probes, multi-arch container, rate limiting."
+++
