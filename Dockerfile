# Base image pinned by digest (2026-11-18) so rebuilds resolve the same rootfs
# on every runner. The tag is intentionally omitted: a floating `latest` adds
# nothing once a digest is given, and dropping it means no mutable reference
# survives in this file. To refresh, resolve the current digest of
# registry.access.redhat.com/ubi10/ubi and replace the sha256 below.
FROM registry.access.redhat.com/ubi10/ubi@sha256:73ea54bfbfb586075bd3d45df6483a7f68875233c8cc7cebcf9b2bb42306c308 AS builder

# Build-only stage: gcc/make/glibc-static are needed for the static link, and
# rust/cargo come from the distro. None of this reaches the final image, which
# is `scratch` and inherits nothing.
RUN dnf install -y gcc make rust cargo glibc-static && \
    dnf clean all

WORKDIR /build

# Never copy the build context wholesale (no `COPY . .`): these are the only
# paths the build needs, so credentials, local policy, and test state cannot
# reach the image or the provenance record. Adding a path here means checking
# it against .dockerignore first.
COPY Cargo.toml Cargo.lock ./
COPY src/ src/

ENV RUSTFLAGS="-C target-feature=+crt-static"
RUN RUST_TARGET=$(rustc -vV | awk '/^host:/ {print $2}') && \
    cargo build --release --locked --target "$RUST_TARGET" && \
    cp "target/$RUST_TARGET/release/mcp-google-workspace" /build/mcp-google-workspace

FROM scratch

LABEL name="mcp-google-workspace" \
      summary="MCP server for Google Workspace APIs" \
      description="Model Context Protocol server with per-project safety policies for Drive, Gmail, Calendar, and other Google Workspace services." \
      io.k8s.display-name="MCP Google Workspace" \
      io.k8s.description="MCP server for Google Workspace APIs with per-project safety policies" \
      maintainer="Fabien Dupont"

# reqwest is built with rustls-tls-native-roots, so the trust store is read at
# run time; `scratch` has none, hence the copy from the build stage.
COPY --from=builder /etc/pki/tls/certs/ca-bundle.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /build/mcp-google-workspace /usr/local/bin/mcp-google-workspace
COPY policy.example.json /etc/mcp-google-workspace/policy.example.json

# Fixed non-root uid:gid. This matches runAsUser/fsGroup 1001 in
# deploy/kubernetes/deployment.yaml, so the image behaves the same under
# `docker run` and under Kubernetes, where the pod securityContext overrides
# it. The copied files are root-owned and world-readable (0755 binary, 0644
# policy), so uid 1001 needs no extra grants.
USER 1001:1001

EXPOSE 3000

# No HEALTHCHECK: the runtime image is `scratch` and ships no shell or HTTP
# client to probe with. Orchestrators should use the unauthenticated endpoints
# the server already exposes (src/http.rs): GET /livez and GET /healthz for
# liveness, GET /readyz for readiness, all on port 3000 — see
# deploy/kubernetes/deployment.yaml.
ENTRYPOINT ["mcp-google-workspace"]

# The default command exposes the HTTP transport on all interfaces. The server
# refuses to start that way without a bearer token: provide GWS_API_TOKEN at
# run time (`-e GWS_API_TOKEN=...`) or pass `--api-token`. For trusted,
# loopback-only or isolated networks, `--insecure-no-auth` is the explicit
# opt-out. Never bake a token into the image.
#
# The policy is deliberately NOT baked in: /etc/mcp-google-workspace/policy.json
# must be mounted (see deploy/kubernetes/deployment.yaml and configmap.yaml).
CMD ["--policy", "/etc/mcp-google-workspace/policy.json", "--http", "0.0.0.0:3000"]
