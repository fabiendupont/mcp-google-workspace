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
├── main.rs         CLI args, telemetry init, transport selection
├── server.rs       JSON-RPC loop (stdio), dual-era dispatch, task chunk handling
├── http.rs         Axum HTTP server, SSE, rate limiter, probes, metrics endpoint
├── protocol.rs     JSON-RPC types, error codes, request/response construction
├── meta.rs         _meta extraction, W3C Trace Context, client era detection
├── execute.rs      Google API execution, URL building, multipart/resumable upload, download
├── policy.rs       TOML policy engine, folder/calendar/method enforcement
├── tools.rs        Tool list generation from Discovery Docs, gws_discover handler
├── tasks.rs        Task lifecycle (working → completed/failed/cancelled)
├── metrics.rs      Prometheus counters, histograms, gauges
├── auth.rs         OAuth2 credential chain (token, file, service account, ADC)
└── resolve.rs      Drive folder path → ID resolution
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
