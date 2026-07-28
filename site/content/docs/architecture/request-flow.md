+++
title = "Request flow"
description = "How requests flow through the server"
date = 2026-06-12T00:00:00+00:00
updated = 2026-07-28T00:00:00+00:00
draft = false
weight = 10
template = "docs/page.html"
[extra]
lead = "From MCP client to Google API and back."
toc = true
top = false
+++

## Overview

{% mermaid() %}
graph TB
    subgraph Client
        CC[Claude Code / MCP Client]
    end

    subgraph "MCP Server"
        subgraph Transport
            STDIO[Stdio Transport]
            HTTP[HTTP Transport - POST /mcp]
            SSE[SSE Stream - GET /mcp]
        end

        PROTO[Protocol Layer - JSON-RPC 2.0]
        META[Metadata - _meta and Trace Context]
        DISPATCH[Request Dispatch]

        subgraph "Tool Execution"
            POLICY[Policy Engine]
            HELPERS[Helper Dispatch]
            EXEC[Execute - URL building and API calls]
            TASKS[Task Manager]
        end

        subgraph Discovery
            CACHE[Discovery Cache]
            TOOLS[Tool Builder]
        end
    end

    subgraph "Google Workspace"
        GAPI[Google APIs]
        DISC[Discovery Service]
    end

    CC -->|stdio or HTTP| Transport
    Transport --> PROTO
    PROTO --> META
    META --> DISPATCH
    DISPATCH --> POLICY
    POLICY --> HELPERS
    HELPERS --> EXEC
    EXEC --> GAPI
    DISPATCH --> TASKS
    DISPATCH --> TOOLS
    TOOLS --> CACHE
    CACHE --> DISC
{% end %}

## How a request flows

1. **Transport** -- The client sends a JSON-RPC message over stdio or HTTP
2. **Protocol** -- The message is parsed, validated, and assigned an error code category
3. **Metadata** -- `_meta` is extracted for protocol version, client info, and W3C Trace Context
4. **Dispatch** -- The method is routed to the appropriate handler (tools/call, tasks/get, etc.)
5. **Policy** -- The policy engine checks service allow-list, method denylist, parameter constraints, and read-only mode
6. **Helper dispatch** -- Services with helpers (Drive, Docs, Sheets, Slides, Gmail) route to purpose-built tool functions instead of the generic executor
7. **Execute** -- The Google API URL is built, OAuth token is obtained (cached), and the request is sent
8. **Response** -- The Google API response is returned to the client as MCP tool result content

## Multi-user deployment

Each user gets their own server instance with isolated credentials and policy:

{% mermaid() %}
graph LR
    subgraph "User A"
        PA[Policy A] --> DA[Pod A]
        CA[Credentials A] --> DA
    end
    subgraph "User B"
        PB[Policy B] --> DB[Pod B]
        CB[Credentials B] --> DB
    end
    DA --> G[Google APIs]
    DB --> G
{% end %}

## Module map

| Module | Purpose |
|--------|---------|
| `main.rs` | CLI args, templates, interactive wizard, policy checker |
| `handler.rs` | rmcp ServerHandler: tools, prompts, resources, completions, tasks, elicitation, subscriptions |
| `server.rs` | Tool dispatch business logic, request explanation |
| `tools.rs` | Tool list builder, lazy filtering, compact schema mode |
| `execute.rs` | Google API execution, URL building, pagination, resumable uploads, rate limiting |
| `helpers.rs` | Google Docs enrichment (9 tools) |
| `sheets_helpers.rs` | Google Sheets enrichment (14 tools) |
| `drive_helpers.rs` | Google Drive enrichment (9 tools) |
| `slides_helpers.rs` | Google Slides enrichment (9 tools) |
| `gmail_helpers.rs` | Gmail enrichment (10 tools) |
| `format.rs` | Markdown/Plain to Docs batchUpdate, doc to Markdown reverse converter |
| `cache.rs` | LRU + TTL in-memory cache for Sheets reads |
| `rate_limit.rs` | Per-service sliding-window rate limiter |
| `policy.rs` | JSON policy engine, constraint enforcement |
| `http.rs` | Axum HTTP server, SSE, probes, metrics, webhooks |
| `resources.rs` | MCP resources: gws:// URI scheme, resource templates |
| `completions.rs` | MCP completions: autocomplete for resource URIs and prompt arguments |
| `elicitation.rs` | MCP elicitation: structured user input |
| `subscriptions.rs` | MCP subscriptions: Drive watch channels, webhooks |
| `prompts.rs` | MCP prompts: external Markdown files, argument substitution |
| `tasks.rs` | Task lifecycle for resumable uploads and chunked downloads |
| `metrics.rs` | Prometheus counters, histograms, gauges |
| `meta.rs` | Request metadata, W3C Trace Context |
| `auth.rs` | OAuth2 credential chain with token caching |
| `audit.rs` | Structured JSONL audit log writer |
| `image_gen.rs` | Gemini image generation and Drive upload |
| `marp.rs` | Marp Markdown to Google Slides conversion |
