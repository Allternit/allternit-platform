# Self-hosted GitHub Actions runner for the allternit-api deploy workflow's
# compile job (Allternit/allternit-platform, label allternit-api-builder).
# Runs as an unprivileged user with no host secrets; the deploy job (SSH key,
# Tailscale) stays on GitHub-hosted runners.
FROM ubuntu:24.04
ARG RUNNER_VERSION=2.338.2
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      build-essential clang pkg-config protobuf-compiler perl make cmake curl ca-certificates git jq unzip \
      libicu74 libkrb5-3 zlib1g libssl3 \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -m -u 1500 runner
USER runner
WORKDIR /home/runner
RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.94.1 \
 && /home/runner/.cargo/bin/rustup component add rustfmt clippy rust-src
ENV PATH=/home/runner/.cargo/bin:$PATH
RUN mkdir actions-runner && cd actions-runner \
 && curl -fsSL -o r.tgz https://github.com/actions/runner/releases/download/v${RUNNER_VERSION}/actions-runner-linux-x64-${RUNNER_VERSION}.tar.gz \
 && tar xzf r.tgz && rm r.tgz
COPY --chown=runner entrypoint.sh /home/runner/entrypoint.sh
ENTRYPOINT ["/bin/bash", "/home/runner/entrypoint.sh"]
