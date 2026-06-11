FROM --platform=$BUILDPLATFORM docker.io/library/rust:1.96-bookworm AS builder

ARG TARGETPLATFORM

RUN case "$TARGETPLATFORM" in \
      "linux/amd64") echo "x86_64-unknown-linux-musl" > /tmp/rust-target ;; \
      "linux/arm64") echo "aarch64-unknown-linux-musl" > /tmp/rust-target ;; \
      *) echo "Unsupported platform: $TARGETPLATFORM" && exit 1 ;; \
    esac && \
    rustup target add "$(cat /tmp/rust-target)" && \
    apt-get update && \
    apt-get install -y musl-tools gcc-aarch64-linux-gnu && \
    rm -rf /var/lib/apt/lists/*

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=aarch64-linux-gnu-gcc

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/

RUN cargo build --release --target "$(cat /tmp/rust-target)" && \
    cp "target/$(cat /tmp/rust-target)/release/mcp-google-workspace" /build/mcp-google-workspace

FROM registry.access.redhat.com/ubi9-micro:latest

LABEL name="mcp-google-workspace" \
      summary="MCP server for Google Workspace APIs" \
      description="Model Context Protocol server with per-project safety policies for Drive, Gmail, Calendar, and other Google Workspace services." \
      io.k8s.display-name="MCP Google Workspace" \
      io.k8s.description="MCP server for Google Workspace APIs with per-project safety policies" \
      maintainer="Fabien Dupont"

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/pki/tls/certs/ca-bundle.crt
COPY --from=builder /build/mcp-google-workspace /usr/local/bin/mcp-google-workspace
COPY policy.example.toml /etc/mcp-google-workspace/policy.example.toml

ENV SSL_CERT_FILE=/etc/pki/tls/certs/ca-bundle.crt

USER 1001

EXPOSE 3000

ENTRYPOINT ["mcp-google-workspace"]
CMD ["--policy", "/etc/mcp-google-workspace/policy.toml", "--http", "0.0.0.0:3000"]
