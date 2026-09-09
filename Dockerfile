# syntax=docker/dockerfile:1
#
# ai-harness control-plane image.
#
# The Rust build embeds the web bundle: `harness-server`'s build.rs runs
# `bun install && bun run build` in web/ and inlines web/dist into the binary —
# so the builder needs both cargo AND bun, and the runtime is a single static-ish
# binary that already serves the UI.
#
# The runtime also carries the agent CLIs (claude / codex / cursor / omp) + git + mise
# + a headless Chromium for the agents' browser tool,
# so `provider: claude|codex|cursor|pi` nodes and toolchain bootstrap work in-pod.
# Provider credentials are NOT baked in — they're entered in the UI, stored encrypted
# in Postgres, and materialized into $HOME (~/.claude, ~/.codex) or env (CURSOR_API_KEY)
# at run time.

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
# `jq` is used by the multi-repo idea-to-pr bash nodes (install-deps,
# verify-pr-base) to parse the `.pr-list` / `$HARNESS_REPOS` JSON — a plain
# `bash:` node hard-fails without it (same rationale as `gh` below).
# `libicu72` is the ICU runtime the .NET SDK needs for globalization — without
# it, a mise-provisioned `dotnet` FailFasts on first invocation ("Couldn't find
# a valid ICU package"), breaking the bc-idea-to-pr (Business Central / AL)
# compile steps.
# `file` is not asked for by any workflow — the agents reach for it themselves,
# to tell text from binary before reading something. `error: command not found:
# file` is the only missing-command failure in a month of run activity (10
# occurrences across 8 runs, in implement / validate / review / finalize
# nodes), against roughly a megabyte for the package and its libmagic
# dependency. Instructing agents not to use a standard utility is the losing
# side of that trade.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates curl file git bash xz-utils unzip jq libicu72 \
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
#
# This layer is cached on its text like any other, so the versions baked in
# here age the same way omp's did below. It is left unpinned because these two
# are the CLIs the system routes CAN update in place at runtime
# (`npm install --prefix`, see `system_routes.rs`), so a stale image is
# recoverable from the UI rather than only by a rebuild. Note `gpt-6-astra` on
# the `codex` provider needs Codex CLI >= 0.153.1 — if a node fails on that,
# updating the CLI from Settings is the fix.
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
    && chmod -R a+rX /opt/bun \
    && ln -sf /opt/bun/bin/bun /usr/local/bin/bun

# omp, in its own layer and at a pinned version.
#
# This used to be an unpinned `bun install -g` sharing the layer above. Docker
# keys a layer on its text, so once that text stopped changing every rebuild
# reused the cached layer and shipped whatever omp had been current the first
# time it was built — "unpinned" means "latest" only on a cache miss. That is
# how a cluster running workflows pinned to `openai-codex/gpt-6-astra` ended up
# with an omp that had never heard of the model: every `pi` review node failed
# with `Model "openai-codex/gpt-6-astra" not found` before making a request.
#
# The version therefore has to be stated, not inferred. Bumping this ARG
# changes the layer's text, which is what makes the new version actually get
# installed. Unlike the npm CLIs above, omp has NO in-app update path — it
# lives in /opt/bun, out of reach of the `npm install --prefix` the
# system routes use — so this line is the only place its version is decided.
#
# 18.1.12 added gpt-6-astra to omp's openai-codex catalog; keep this at or
# above that for as long as any workflow pins an Astra model.
ARG OMP_VERSION=18.1.15
RUN BUN_INSTALL=/opt/bun /opt/bun/bin/bun install -g "@oh-my-pi/pi-coding-agent@${OMP_VERSION}" \
    && chmod -R a+rX /opt/bun \
    && ln -sf /opt/bun/bin/omp /usr/local/bin/omp

# pi-web-access: adds web SEARCH + rich fetch (Exa MCP) to the omp agent on top
# of omp's built-in URL `fetch`. Installed into a fixed, world-readable dir —
# NOT the PV-backed $HOME, which a runtime volume mount would shadow — and loaded
# via `--plugin-dir` (the runner passes OMP_PLUGIN_DIRS → `--plugin-dir`).
RUN mkdir -p /opt/omp-plugins \
    && cd /opt/omp-plugins \
    && BUN_INSTALL=/opt/bun /opt/bun/bin/bun add pi-web-access \
    && chmod -R a+rX /opt/omp-plugins
# Cursor CLI (cursor-agent) — invoked as a subprocess by `provider: cursor` workflow
# nodes. The installer unpacks a multi-file payload to
# ~/.local/share/cursor-agent/versions/<ver>/ and symlinks ~/.local/bin/cursor-agent
# into it; the versioned executable resolves its bundled node/JS relative to its own
# real path. So we must relocate the WHOLE payload (not just the symlink) out of
# /root/.local (mode 700, unreadable by uid 1000): move the tree to /opt/cursor-agent,
# make it world-readable, and symlink the versioned executable onto PATH. The version
# dir is date-named, so pick the latest by sort rather than hard-coding it. Auth is
# materialized at run time via CURSOR_API_KEY — no login during build.
RUN curl https://cursor.com/install -fsS | bash \
    && mv /root/.local/share/cursor-agent /opt/cursor-agent \
    && chmod -R a+rX /opt/cursor-agent \
    && ln -sf "$(ls -d /opt/cursor-agent/versions/*/cursor-agent | sort | tail -1)" /usr/local/bin/cursor-agent \
    && rm -rf /root/.local /root/.cursor \
    && cursor-agent --version

# Chromium, for the agents' browser tool.
#
# omp ships a browser tool and the agents reach for it unprompted on frontend
# work; with no browser in the image it fails with "Shared browser daemon
# unavailable (broker start or Chromium launch failed)" — 6 occurrences across
# 4 runs in a week, in `implement-tasks` and `gpt-review-fix`. Unlike the
# missing-`file` case this is not only lost convenience: on a storefront project
# the deliverable IS the rendered page, and every review pass so far has had to
# sign off with "live rendering not verified" because nothing in the image could
# open it.
#
# This is by far the largest thing here — roughly 400 MB with its dependencies,
# against ~1 MB for `file`. It is worth it only because the projects this
# harness runs are mostly web front ends; an install that is all backend should
# consider dropping this layer.
#
# `fonts-liberation` is not optional cosmetics: without a font package Chromium
# renders every glyph as a box, so screenshots are worthless for the visual
# check that justifies having it at all.
#
# The two env vars below are the discovery paths browser tooling actually reads
# (chrome-launcher/lighthouse honor CHROME_PATH; puppeteer honors
# PUPPETEER_EXECUTABLE_PATH), set so a library that would otherwise try to
# download its own copy at run time finds this one instead. CHROMIUM_FLAGS is
# read by Debian's /usr/bin/chromium wrapper: containers get a 64 MB /dev/shm
# and no CAP_SYS_ADMIN, which are the two things that make an otherwise healthy
# Chromium fail to start.
RUN apt-get update \
    && apt-get install -y --no-install-recommends chromium fonts-liberation \
    && rm -rf /var/lib/apt/lists/* \
    && /usr/bin/chromium --version

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
    HARNESS_HTTP_ADDR=0.0.0.0:8080 \
    OMP_PLUGIN_DIRS=/opt/omp-plugins/node_modules/pi-web-access \
    CHROME_PATH=/usr/bin/chromium \
    PUPPETEER_EXECUTABLE_PATH=/usr/bin/chromium \
    CHROMIUM_FLAGS="--no-sandbox --disable-dev-shm-usage"
WORKDIR /home/harness
EXPOSE 8080

ENTRYPOINT ["harness"]
CMD ["serve"]
