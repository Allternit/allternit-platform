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
# allternit-mux: the PTY multiplexer daemon gizzi-code auto-spawns on demand
# (cmd/gizzi-code/src/runtime/integrations/pty/index.ts). Previously absent from
# this image, which silently killed terminal/PTY features in the hosted runtime
# (see docs/Audits_and_Research/BYOC_DESKTOP_CLOUD_IOS_ARCHITECTURE_AUDIT.md,
# tangled-edge finding #1). Built from the same workspace, not downloaded, since
# there's no pinned release artifact for it the way there is for gizzi-code.
FROM rust:1.97-slim AS mux-builder

WORKDIR /app

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config \
    && rm -rf /var/lib/apt/lists/*

COPY . .

RUN cargo build --release -p allternit-mux

# ─────────────────────────────────────────────────────────────────────────────
FROM oven/bun:1.2-debian

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY --from=builder /app/gizzi-code ./cmd/gizzi-code/dist/gizzi-code
COPY --from=builder /app/cmd/agent-daemon/dist/index.js ./cmd/agent-daemon/dist/index.js
COPY --from=mux-builder /app/target/release/allternit-mux ./cmd/gizzi-code/dist/allternit-mux

COPY cmd/allternit-hosted-runtime/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh \
    && chmod +x /app/cmd/gizzi-code/dist/gizzi-code \
    && chmod +x /app/cmd/gizzi-code/dist/allternit-mux

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
