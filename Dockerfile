# syntax=docker/dockerfile:1
#
# ai-harness control-plane image.
#
# The Rust build embeds the web bundle: `harness-server`'s build.rs runs
# `bun install && bun run build` in web/ and inlines web/dist into the binary —
# so the builder needs both cargo AND bun, and the runtime is a single static-ish
# binary that already serves the UI.
#
# The runtime also carries the agent CLIs (claude / codex / omp) + git + mise, so
# `provider: claude|codex|pi` nodes and toolchain bootstrap work in-pod. Provider
# credentials are NOT baked in — they're entered in the UI, stored encrypted in
# Postgres, and materialized into $HOME (~/.claude, ~/.codex) at run time.

# ── Builder: cargo + bun → the `harness` binary with the UI embedded ──────────
FROM rust:1-bookworm AS builder

# bun, for build.rs's web bundling (web/ + sdk/typescript).
RUN curl -fsSL https://bun.sh/install | bash \
    && ln -sf /root/.bun/bin/bun /usr/local/bin/bun

WORKDIR /src
COPY . .

# Release build of the CLI (`harness serve`). This transitively builds
# harness-server, whose build.rs bundles the web UI via bun.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p harness-cli \
    && cp /src/target/release/harness /usr/local/bin/harness

# ── Runtime: the binary + agent CLIs + git + mise ────────────────────────────
FROM debian:bookworm-slim AS runtime

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl git bash xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Node (Claude Code + Codex CLIs are npm packages).
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && npm install -g @anthropic-ai/claude-code @openai/codex \
    && npm cache clean --force \
    && rm -rf /var/lib/apt/lists/*

# Bun + omp (the Pi/Kimi CLI), and mise (toolchain provisioning).
RUN curl -fsSL https://bun.sh/install | bash \
    && ln -sf /root/.bun/bin/bun /usr/local/bin/bun \
    && bun install -g @oh-my-pi/pi-coding-agent \
    && curl -fsSL https://mise.run | sh \
    && ln -sf /root/.local/bin/mise /usr/local/bin/mise

# Non-root user; $HOME holds the run-time-materialized agent credentials and is
# expected to be backed by a PersistentVolume so they survive restarts.
RUN useradd --create-home --uid 1000 --shell /bin/bash harness
# Make the root-installed global bun bin available to the harness user too.
RUN ln -sf /root/.bun/bin/omp /usr/local/bin/omp || true

COPY --from=builder /usr/local/bin/harness /usr/local/bin/harness

USER harness
ENV HOME=/home/harness \
    PATH="/home/harness/.local/bin:/usr/local/bin:/usr/bin:/bin" \
    HARNESS_HTTP_ADDR=0.0.0.0:8080
WORKDIR /home/harness
EXPOSE 8080

ENTRYPOINT ["harness"]
CMD ["serve"]
