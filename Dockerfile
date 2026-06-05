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
        ca-certificates curl git bash xz-utils unzip \
    && rm -rf /var/lib/apt/lists/*

# GitHub CLI. The idea-to-pr pipeline's finalize / verify-pr-base / verify-pr-title
# / review / summary steps all shell out to `gh` (pr create/edit/view/comment,
# labels). Agent nodes could improvise around a missing `gh`, but plain `bash:`
# nodes (e.g. verify-pr-base, `set -euo pipefail`) hard-fail with
# "gh: command not found". Install from the official apt repo so it lands on the
# default PATH (/usr/bin/gh) for both agent and bash nodes; auth is via the
# GH_TOKEN materialized into the process env at run time.
RUN mkdir -p -m 755 /etc/apt/keyrings \
    && curl -fsSL https://cli.github.com/packages/githubcli-archive-keyring.gpg \
        -o /etc/apt/keyrings/githubcli-archive-keyring.gpg \
    && chmod go+r /etc/apt/keyrings/githubcli-archive-keyring.gpg \
    && echo "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main" \
        > /etc/apt/sources.list.d/github-cli.list \
    && apt-get update \
    && apt-get install -y --no-install-recommends gh \
    && rm -rf /var/lib/apt/lists/*

# Node (Claude Code + Codex CLIs are npm packages).
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && npm install -g @anthropic-ai/claude-code @openai/codex \
    && npm cache clean --force \
    && rm -rf /var/lib/apt/lists/*

# Bun + omp (the Pi/Kimi CLI) and mise (toolchain provisioning). These MUST be
# installed into world-readable/executable locations — NOT under /root, which is
# mode 700 and unreadable by the non-root `harness` user (uid 1000). The old
# symlink-into-/root pattern left these binaries unexecutable at runtime, so mise
# could never provision a toolchain (cargo/pnpm/…). Install bun into /opt/bun via
# BUN_INSTALL (so its global packages, incl. omp, land there too) and move the
# mise binary into /usr/local/bin.
RUN curl -fsSL https://bun.sh/install | BUN_INSTALL=/opt/bun bash \
    && BUN_INSTALL=/opt/bun /opt/bun/bin/bun install -g @oh-my-pi/pi-coding-agent \
    && chmod -R a+rX /opt/bun \
    && ln -sf /opt/bun/bin/bun /usr/local/bin/bun \
    && ln -sf /opt/bun/bin/omp /usr/local/bin/omp
RUN curl -fsSL https://mise.run | sh \
    && mv /root/.local/bin/mise /usr/local/bin/mise \
    && chmod a+rx /usr/local/bin/mise \
    && rm -rf /root/.local

# Non-root user; $HOME holds the run-time-materialized agent credentials and is
# expected to be backed by a PersistentVolume so they survive restarts.
RUN useradd --create-home --uid 1000 --shell /bin/bash harness
# Activate mise in the harness user's login/interactive shells too. The runner
# itself uses `bash -c` with an injected shims PATH, but agents and any login
# shell (`bash -lc`) need this so mise-provisioned tools resolve there as well.
RUN echo 'eval "$(/usr/local/bin/mise activate bash)"' >> /home/harness/.bashrc \
    && chown harness:harness /home/harness/.bashrc

COPY --from=builder /usr/local/bin/harness /usr/local/bin/harness

USER harness
ENV HOME=/home/harness \
    PATH="/home/harness/.local/bin:/usr/local/bin:/usr/bin:/bin" \
    HARNESS_HTTP_ADDR=0.0.0.0:8080
WORKDIR /home/harness
EXPOSE 8080

ENTRYPOINT ["harness"]
CMD ["serve"]
