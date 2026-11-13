# Allternit Hosted Runtime
# Runs a checksum-pinned Gizzi brain and the agent-daemon relay in a Fly Machine.
# Build context: repository root

FROM oven/bun:1.2-debian AS builder

# A gizzi-code/v* release of Allternit/allternit-platform. 2.2.4+ names the
# calling session on connector tool calls (`_meta["allternit/session"]`), which
# hosted agents' phone tools and approvals need. The checksum is the release's
# linux-x64 tarball (its checksums.txt), not the binary inside it.
ARG GIZZI_VERSION=2.9.0
ARG GIZZI_LINUX_X64_SHA256=5aa323e4c5804a2d1747e6b2b29a7d6da0ce2fb4082c6ad8dc1f1da7ae8d7a3d

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl nodejs npm \
    && rm -rf /var/lib/apt/lists/*

# Fetch the smoke-tested headless runtime artifact. Pinning both its release and
# checksum makes the hosted image reproducible without compiling desktop-only
# Gizzi sources inside the container build.
RUN curl -fsSL \
      "https://github.com/Allternit/allternit-platform/releases/download/gizzi-code%2Fv${GIZZI_VERSION}/gizzi-code-v${GIZZI_VERSION}-linux-x64.tar.gz" \
      -o /tmp/gizzi.tar.gz \
    && echo "${GIZZI_LINUX_X64_SHA256}  /tmp/gizzi.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/gizzi.tar.gz -C /app gizzi-code \
    && rm /tmp/gizzi.tar.gz \
    && chmod +x /app/gizzi-code

# Install and bundle only the relay daemon. The compiled Gizzi artifact already
# contains its own runtime and dependencies.
COPY package.json pnpm-lock.yaml ./
COPY cmd/allternit-hosted-runtime/pnpm-workspace.yaml ./pnpm-workspace.yaml
COPY patches ./patches
COPY cmd/agent-daemon/package.json ./cmd/agent-daemon/

RUN npm install -g pnpm@10.28.0 \
    && pnpm install --filter @allternit/agent-daemon... --frozen-lockfile

COPY cmd/agent-daemon/src ./cmd/agent-daemon/src

RUN cd cmd/agent-daemon \
    && bun build src/index.ts --target=bun --outfile=dist/index.js

# ─────────────────────────────────────────────────────────────────────────────
# allternit-factory: the Allternit Factory engine. Its pane engine owns Gizzi's
# PTYs (cmd/gizzi-code/src/runtime/integrations/pty/index.ts finds it next to
# gizzi-code and starts it with `allternit-factory pane tty ensure`); it
# replaced allternit-mux. The pinned Gizzi above has the pane-engine PTY and
# uses this binary. Built from the same workspace rather than taken from the
# release tarball, so it always matches this checkout's pane engine.
FROM rust:1.97-slim AS factory-builder

# The pane engine links libghostty-vt, built by Zig 0.15.2 (factory/pane/build.rs).
ARG ZIG_VERSION=0.15.2
ARG ZIG_X86_64_LINUX_SHA256=02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239
ARG ZIG_AARCH64_LINUX_SHA256=958ed7d1e00d0ea76590d27666efbf7a932281b3d7ba0c6b01b0ff26498f667f

WORKDIR /app

# libssl-dev: openssl-sys. protobuf-compiler: the engine's build.rs compiles
# proto/verification.proto with tonic-build, which needs protoc.
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev protobuf-compiler ca-certificates curl xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN arch="$(uname -m)" \
    && case "$arch" in \
         x86_64) sha="${ZIG_X86_64_LINUX_SHA256}" ;; \
         aarch64) sha="${ZIG_AARCH64_LINUX_SHA256}" ;; \
         *) echo "no pinned Zig for $arch" >&2; exit 1 ;; \
       esac \
    && curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-${arch}-linux-${ZIG_VERSION}.tar.xz" -o /tmp/zig.tar.xz \
    && echo "${sha}  /tmp/zig.tar.xz" | sha256sum -c - \
    && mkdir -p /opt/zig \
    && tar -xJf /tmp/zig.tar.xz -C /opt/zig --strip-components=1 \
    && rm /tmp/zig.tar.xz \
    && ln -s /opt/zig/zig /usr/local/bin/zig \
    && zig version

COPY . .

RUN cargo build --release -p allternit-factory

# ─────────────────────────────────────────────────────────────────────────────
FROM oven/bun:1.2-debian

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/gizzi-code ./cmd/gizzi-code/dist/gizzi-code
COPY --from=builder /app/cmd/agent-daemon/dist/index.js ./cmd/agent-daemon/dist/index.js
COPY --from=factory-builder /app/target/release/allternit-factory ./cmd/gizzi-code/dist/allternit-factory

COPY cmd/allternit-hosted-runtime/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh \
    && chmod +x /app/cmd/gizzi-code/dist/gizzi-code \
    && chmod +x /app/cmd/gizzi-code/dist/allternit-factory

ENV ALLTERNIT_GATEWAY_URL=http://127.0.0.1:8013
ENV ALLTERNIT_RUNTIME_IDENTITY_PATH=/data/runtime-identity.json
ENV XDG_DATA_HOME=/data/.local/share
ENV XDG_CONFIG_HOME=/data/.config
ENV XDG_STATE_HOME=/data/.local/state
ENV XDG_CACHE_HOME=/data/.cache
ENV GIZZI_CONFIG_DIR=/data/.config/gizzi-code
ENV GIZZI_DISABLE_AUTOUPDATE=1
# Without this, the sidecar auto-pulls and runs its embedded Ollama model on
# every hosted-runtime Fly Machine by default, competing with real workloads
# for CPU/memory on a box with no guaranteed GPU headroom for it (see
# docs/Audits_and_Research/BYOC_DESKTOP_CLOUD_IOS_ARCHITECTURE_AUDIT.md,
# tangled-edge finding #4).
ENV ALLTERNIT_SIDECAR_DISABLED=1
ENV NODE_ENV=production
ENV PORT=8013

VOLUME ["/data"]

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
