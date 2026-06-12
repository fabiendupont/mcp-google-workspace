FROM registry.access.redhat.com/ubi10/ubi:latest AS builder

RUN dnf install -y gcc make rust cargo glibc-static && \
    dnf clean all

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ src/

ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN RUST_TARGET=$(rustc -vV | awk '/^host:/ {print $2}') && \
    cargo build --release --target "$RUST_TARGET" && \
    cp "target/$RUST_TARGET/release/mcp-google-workspace" /build/mcp-google-workspace

FROM scratch

LABEL name="mcp-google-workspace" \
      summary="MCP server for Google Workspace APIs" \
      description="Model Context Protocol server with per-project safety policies for Drive, Gmail, Calendar, and other Google Workspace services." \
      io.k8s.display-name="MCP Google Workspace" \
      io.k8s.description="MCP server for Google Workspace APIs with per-project safety policies" \
      maintainer="Fabien Dupont"

COPY --from=builder /etc/pki/tls/certs/ca-bundle.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /build/mcp-google-workspace /usr/local/bin/mcp-google-workspace
COPY policy.example.toml /etc/mcp-google-workspace/policy.example.toml

USER 65534

EXPOSE 3000

ENTRYPOINT ["mcp-google-workspace"]
CMD ["--policy", "/etc/mcp-google-workspace/policy.toml", "--http", "0.0.0.0:3000"]
