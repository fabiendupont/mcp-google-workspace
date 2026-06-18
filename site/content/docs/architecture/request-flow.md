+++
title = "Request flow"
description = "How requests flow through the server"
date = 2026-06-12T00:00:00+00:00
updated = 2026-06-12T00:00:00+00:00
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
    POLICY --> EXEC
    EXEC --> GAPI
    DISPATCH --> TASKS
    DISPATCH --> TOOLS
    TOOLS --> CACHE
    CACHE --> DISC
{% end %}

## How a request flows

1. **Transport** — The client sends a JSON-RPC message over stdio or HTTP
2. **Protocol** — The message is parsed, validated, and assigned an error code category
3. **Metadata** — `_meta` is extracted for protocol version, client info, and W3C Trace Context
4. **Dispatch** — The method is routed to the appropriate handler (tools/call, tasks/get, etc.)
5. **Policy** — The policy engine checks service allow-list, method denylist, parameter constraints, and read-only mode
6. **Execute** — The Google API URL is built, OAuth token is obtained (cached), and the request is sent
7. **Response** — The Google API response is returned to the client as MCP tool result content

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
| `main.rs` | CLI args, telemetry init, transport selection |
| `server.rs` | JSON-RPC loop, dual-era dispatch, task chunk handling |
| `http.rs` | Axum HTTP server, SSE, rate limiter, probes, metrics |
| `execute.rs` | Google API execution, URL building, upload, download |
| `policy.rs` | JSON policy engine, generic constraint enforcement |
| `tools.rs` | Tool list from Discovery Docs, `gws_discover` handler |
| `tasks.rs` | Task lifecycle (working, completed, failed, cancelled) |
| `protocol.rs` | JSON-RPC types, error codes, request/response |
| `meta.rs` | `_meta` extraction, W3C Trace Context, era detection |
| `metrics.rs` | Prometheus counters, histograms, gauges |
| `auth.rs` | OAuth2 credential chain with token caching |
| `audit.rs` | Structured JSONL audit log writer |
