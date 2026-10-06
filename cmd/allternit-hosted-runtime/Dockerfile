# Allternit Hosted Runtime
# Runs a checksum-pinned Gizzi brain and the agent-daemon relay in a Fly Machine.
# Build context: repository root

FROM oven/bun:1.2-debian AS builder

ARG GIZZI_RELEASE=hosted-runtime-2026.07.16
ARG GIZZI_LINUX_X64_SHA256=f1d29bad0b3903d77261e7706ff80fd292fefece3ebeaa4bb7f08a51ad2fc694

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl nodejs npm \
    && rm -rf /var/lib/apt/lists/*

# Fetch the smoke-tested headless runtime artifact. Pinning both its release and
# checksum makes the hosted image reproducible without compiling desktop-only
# Gizzi sources inside the container build.
RUN curl -fsSL \
      "https://github.com/Allternit/gizzi-code/releases/download/${GIZZI_RELEASE}/gizzi-code-linux-x64-native" \
      -o /app/gizzi-code \
    && echo "${GIZZI_LINUX_X64_SHA256}  /app/gizzi-code" | sha256sum -c - \
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
# replaced allternit-mux. The pinned Gizzi above (2026.07.16) still runs its
# PTYs in-process (bun-pty) and never started mux; a Gizzi pinned from a
# release that has the pane-engine PTY uses this binary. Built from the same
# workspace, not downloaded: there's no pinned release artifact for it.
FROM rust:1.97-slim AS factory-builder

# The pane engine links libghostty-vt, built by Zig 0.15.2 (factory/pane/build.rs).
ARG ZIG_VERSION=0.15.2
ARG ZIG_X86_64_LINUX_SHA256=02aa270f183da276e5b5920b1dac44a63f1a49e55050ebde3aecc9eb82f93239
ARG ZIG_AARCH64_LINUX_SHA256=958ed7d1e00d0ea76590d27666efbf7a932281b3d7ba0c6b01b0ff26498f667f

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config ca-certificates curl xz-utils \
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
