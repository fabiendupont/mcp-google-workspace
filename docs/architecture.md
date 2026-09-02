# Architecture

## Request Flow

```mermaid
graph TB
    subgraph Client
        CC[Claude Code / MCP Client]
    end

    subgraph "MCP Server"
        subgraph Transport
            STDIO[Stdio Transport]
            HTTP[HTTP Transport<br/>POST /mcp]
            SSE[SSE Stream<br/>GET /mcp]
        end

        PROTO[Protocol Layer<br/>JSON-RPC 2.0 parsing<br/>Error codes · Request ID tracking]
        META[Metadata Extraction<br/>_meta · W3C Trace Context<br/>Client era detection]
        DISPATCH[Request Dispatch<br/>initialize · server/discover<br/>tools/list · tools/call<br/>tasks/* · ping]

        subgraph "Tool Execution"
            POLICY[Policy Engine<br/>Service allow/deny<br/>Folder ACLs · Calendar ACLs<br/>Method denylists · Read-only]
            EXEC[Execute<br/>URL building · Scope selection<br/>Multipart upload · Resumable upload<br/>Binary download · Pagination]
            TASKS[Task Manager<br/>Upload/download sessions<br/>Lifecycle tracking<br/>Expiry cleanup]
        end

        subgraph Discovery
            CACHE[Discovery Cache<br/>Google Discovery Docs<br/>Fetched once, reused]
            TOOLS[Tool Builder<br/>Dynamic tool schemas<br/>from Discovery Docs]
        end

        subgraph Observability
            METRICS[Prometheus Metrics<br/>/metrics endpoint<br/>Request count · Latency · Errors]
            TRACE[OTel Tracing<br/>OTLP/HTTP export<br/>Span instrumentation]
            LOG[Structured Logging<br/>tracing macros<br/>stderr output]
        end

        subgraph Security
            AUTH[OAuth2 Chain<br/>Token · Credentials file<br/>Service account · ADC]
            RATE[Rate Limiter<br/>Per-client-IP<br/>Sliding window]
            ORIGIN[Origin Validation<br/>Configurable allowlist]
        end

        subgraph Probes
            HEALTH["/healthz"]
            READY["/readyz"]
            LIVE["/livez"]
        end
    end

    subgraph "Google Workspace"
        GAPI[Google APIs<br/>Drive · Gmail · Calendar<br/>Sheets · Docs · ...]
        DISC[Discovery Service<br/>API schemas]
    end

    CC -->|stdio or HTTP| Transport
    Transport --> PROTO
    PROTO --> META
    META --> DISPATCH
    DISPATCH --> POLICY
    POLICY --> EXEC
    EXEC --> AUTH
    AUTH --> GAPI
    DISPATCH --> TASKS
    DISPATCH --> TOOLS
    TOOLS --> CACHE
    CACHE --> DISC
    EXEC --> TRACE
    EXEC --> METRICS
    HTTP --> RATE
    HTTP --> ORIGIN
```

## Module Map

```
src/
├── main.rs              CLI arg parsing, templates, interactive wizard, policy checker
├── lib.rs               Library crate root, module declarations
├── handler.rs           rmcp ServerHandler: tools, prompts, resources, completions, tasks, elicitation, subscriptions, lazy discovery
├── server.rs            Tool dispatch, shared utilities, image gen, batch, tasks
├── http.rs              Hybrid Axum server: rmcp StreamableHttpService + health/metrics/webhooks
├── execute.rs           HTTP execution: URL rendering, params, pagination, resumable uploads, rate limiting
├── policy.rs            JSON policy engine: constraints, method denylists, read-only, recursive parent ancestry
├── tools.rs             Builds MCP tool list, lazy filtering by activated services, compact schema mode
├── meta.rs              Request metadata (W3C Trace Context)
├── auth.rs              OAuth2 chain: env var → credentials file → service account → ADC/gcloud
├── init.rs              Interactive setup wizard and policy templates
├── shared.rs            Shared utilities extracted to break circular dependency
├── drive_helpers.rs     Google Drive enrichment (9 tools)
├── gmail_helpers.rs     Gmail enrichment (10 tools)
├── sheets_helpers.rs    Google Sheets enrichment (14 tools)
├── helpers.rs           Google Docs enrichment (9 tools)
├── slides_helpers.rs    Google Slides enrichment (9 tools)
├── calendar_helpers.rs  Google Calendar enrichment (6 tools)
├── format.rs            Format transformers: Markdown/Plain → Docs batchUpdate, doc → Markdown
├── marp.rs              Marp Markdown to Google Slides conversion
├── image_gen.rs         Gemini image generation and Drive upload
├── cache.rs             LRU + TTL in-memory cache for Sheets values.get responses
├── rate_limit.rs        Per-service sliding-window rate limiter for Google API quotas
├── resources.rs         MCP resources: gws:// URI scheme, resource templates from Discovery Documents
├── completions.rs       MCP completions: autocomplete for resource URIs and prompt arguments
├── elicitation.rs       MCP elicitation: structured user input
├── subscriptions.rs     MCP subscriptions: Google Drive watch channels, webhook notifications
├── prompts.rs           MCP prompts: load external Markdown files, argument substitution
├── tasks.rs             Task lifecycle for resumable uploads and chunked downloads
├── metrics.rs           Prometheus counters, histograms, gauges
└── audit.rs             Structured JSONL audit log writer
```

## Multi-User Deployment

Each user gets their own server instance with their own credentials and policy:

```mermaid
graph LR
    subgraph "Kubernetes Cluster"
        subgraph "User A"
            PA[Policy A<br/>ConfigMap]
            CA[Credentials A<br/>Secret]
            DA[mcp-gws Pod A]
            SA[Service A]
        end

        subgraph "User B"
            PB[Policy B<br/>ConfigMap]
            CB[Credentials B<br/>Secret]
            DB[mcp-gws Pod B]
            SB[Service B]
        end
    end

    UA[User A's Agent] --> SA --> DA
    UB[User B's Agent] --> SB --> DB
    DA --> GA[Google APIs<br/>User A's data]
    DB --> GB[Google APIs<br/>User B's data]
    PA --> DA
    CA --> DA
    PB --> DB
    CB --> DB
```

This gives per-user isolation: different policies, credentials, rate limits, and no shared state.
