+++
title = "MCP Google Workspace"

[extra]
lead = 'Give AI agents <b>controlled access</b> to Google Workspace APIs. A JSON policy file scopes what each agent can access — folder-level Drive ACLs, per-calendar permissions, method denylists, and read-only mode.'
url = "/docs/getting-started/quickstart/"
url_button = "Get started"
repo_version = "v0.9.1"
repo_license = "Open-source Apache-2.0 License."
repo_url = "https://github.com/fabiendupont/mcp-google-workspace"

[[extra.menu.main]]
name = "Docs"
section = "docs"
url = "/docs/getting-started/quickstart/"
weight = 10

[[extra.list]]
title = "Purpose-built helpers"
content = "Drive, Docs, Sheets, Slides, Gmail, Calendar, and Tasks each have dedicated helper tools. Lazy discovery keeps the tool list small until the model needs a service."

[[extra.list]]
title = "Policy-as-code"
content = "A single JSON file controls everything: which services, which folders, which calendars, which methods."

[[extra.list]]
title = "Secure by default"
content = "Bearer auth with boot-latched enforcement, origin checks, two-tier rate limits (pre-auth IP and per-credential budgets), and request-body caps on the HTTP transport. Policy denials are always audited as 403. Prometheus metrics, OpenTelemetry traces, Kubernetes probes, and a multi-arch container."
+++
