# ai-harness — Agent & Contributor Rules

This is the canonical guide for both humans and AI agents working in this repo.
`CLAUDE.md` points here.

## Project

`ai-harness` is a Rust-native orchestration layer for AI coding agents. It
constructs prompts and manages lifecycle — the agents (Claude Code, Codex,
Pi/Kimi) decide how to execute. It is **seeded from
[majiayu000/harness](https://github.com/majiayu000/harness) (MIT)** and is being
re-targeted to: run **in Kubernetes**, be triggered by **AI or cron-polled
Linear**, execute **user-authored workflow DAGs**, and **provision toolchains
from the UI**.

- Authoritative design: [docs/PLAN.md](docs/PLAN.md), [docs/PHASES.md](docs/PHASES.md).
- Authoring workflow DAGs (node types, `when:`/`$node.output`/`output_format`,
  `trigger_rule`, good-practices): [docs/authoring-workflows.md](docs/authoring-workflows.md).
- Inherited design specs in [docs/reference/](docs/reference/) are **background
  only** — they describe majiayu's design at seed time and may have drifted.
  Verify against the current code before relying on them.

## Language

All outputs MUST be in English: code comments, documentation, commit messages,
PR titles/descriptions, prompt templates (`prompts.rs`), CLI help, and error
messages.

## Build & verify

**Scope verification to what you changed — do NOT run the whole workspace after
every small edit.** This is a large Rust workspace; a full `--workspace` compile
is minutes. Match the verify to the change:

- **Web-only change** (only `web/**` touched): run **`cd web && bunx tsc --noEmit
  && bunx vitest run && bunx vite build`**. Do **NOT** run any `cargo` command —
  no Rust was touched, and compiling the workspace just to check a `.tsx` edit is
  pure waste.
- **Single-crate Rust change**: `cargo test -p <crate>` and `cargo clippy -p <crate>`
  for the crate(s) you edited (plus direct dependents if you changed a public API).
- **Cross-cutting Rust change** (enum variant, shared trait, workspace dep): the
  wider `cargo check --workspace` is justified — exhaustive match / API breakage
  needs it.

Always: `cargo fmt --all` (or `cd web && bunx prettier`) before committing.

**Two verify tiers — quick pre-check vs. the full gate:**

- **Quick pre-check** (an early/mid-pipeline sanity gate — NOT the full suite):
  `cargo fmt --all --check`, `RUSTFLAGS="-Dwarnings" cargo clippy --workspace
  --all-targets`, `cargo build --workspace`. Fast; catches format/lint/compile
  breakage without paying for the whole test run.
- **Full gate — run it once, at the end, not per-edit.** Before a PR is finalized
  (the workflow's final verify step), run:
  `RUSTFLAGS="-Dwarnings" cargo clippy --workspace --all-targets`, then
  `cargo test --workspace`. (No separate `cargo check` — `clippy --all-targets`
  already type-checks every target, so a preceding `check` is redundant work.)
  This is the single authoritative gate; the per-edit loop above stays scoped
  and fast.
- When adding an enum variant, grep ALL match sites and update them — CI uses
  exhaustive match checks.
- Dead code in `#[cfg(test)]` modules still trips `-D warnings` in CI — delete
  unused test helpers instead of `#[allow(dead_code)]`.
- Pre-commit hook (`.githooks/pre-commit`) runs fmt + clippy + test. After
  cloning, activate with: `git config core.hooksPath .githooks` (the workspace
  `build.rs` also auto-configures this).
- The web dashboard is bundled into `harness-server` at build time via `bun`
  (see `crates/harness-server/build.rs`). For Rust-only iteration, set
  `HARNESS_SKIP_WEB_BUILD=1` (a stub UI is embedded). Release builds require
  `bun` and a built `web/dist`. The `web/` app depends on `sdk/typescript` for
  shared API types.

## Architecture & glossary

The harness builds prompts and manages lifecycle; agents execute. New crates
being added per the plan: `harness-dag` (DAG model + executor), `harness-sources`
(Linear/GitHub triggers), `harness-toolchain`, `harness-k8s`. Existing terms:

| Term | Meaning | Location |
|---|---|---|
| **workflow runtime** | Orchestration layer that decides what happens next; event-sourced state machine with a command outbox. | `crates/harness-workflow/src/runtime/` |
| **agent runtime** (`CodeAgent` / `AgentAdapter`) | Agent abstraction; receives an `AgentRequest`, returns a stream or response. | `crates/harness-core/src/agent.rs`, `crates/harness-agents/src/` |
| **`RuntimeKind`** | Label the workflow layer attaches to an agent type (`CodexExec` / `CodexJsonrpc` / `ClaudeCode` / `AnthropicApi` / `RemoteHost`). | `crates/harness-workflow/src/runtime/model.rs` |
| **task** | Legacy execution unit; being migrated to flow through the workflow runtime. | `crates/harness-server/src/task_runner/` |
| **runtime host** | Process instance that executes runtime jobs. | `crates/harness-server/src/runtime_hosts.rs` |

There is no type literally named `AgentRuntime`. Prefer the precise names above.

- Avoid `Command::new("gh")` / `Command::new("git")` inside harness crates —
  git/GitHub interaction belongs in agent prompts. This is the orchestration
  philosophy inherited from the base; relax it deliberately, not by accident.

## Agent CLI specifics

- **Claude CLI** `-p` takes its prompt as the NEXT token: `claude -p <PROMPT> [flags]`.
  The prompt MUST immediately follow `-p`, or you get "Input must be provided".
  Both `claude.rs` (CodeAgent) and `claude_adapter.rs` (AgentAdapter) spawn the
  CLI — apply arg-construction changes to BOTH. Verify with
  `cargo test --package harness-agents`.
- **Pi/Kimi** adapter is not implemented yet (Phase 3 — see [docs/PLAN.md](docs/PLAN.md) §7.3).

## Server operation

- NEVER start `harness serve` from within a Claude Code / agent session — the
  `CLAUDECODE` and `CLAUDE_CODE_ENTRYPOINT` env vars propagate to spawned
  subprocesses and cause SIGTRAP. Start from a standalone terminal:
  `./target/release/harness serve --transport http --port 9800 --project-root <path>`.
- If already running inside an agent session, only stop/kill the server — let
  the user start it manually.

## Worktree usage

- NEVER use `isolation: "worktree"` for tasks that depend on unpushed local
  commits — worktrees check out from remote and miss local changes.
- Before using worktree isolation, check `git log origin/main..HEAD`; if there
  are unpushed commits touching the files you'll modify, work on `main` instead.
- Worktrees are only safe for truly independent tasks on un-modified code.

## Dependencies

- NEVER downgrade dependency versions unless explicitly requested.
- Prefer the standard library over new dependencies.
- Run `cargo audit` before adding security-sensitive crates.

## PR workflow

- Keep PRs focused; squash-merge.
- Do NOT modify the `Cargo.toml` version in feature/fix PRs — version bumps
  happen at release time (prevents conflicts across parallel PRs).
- The `CI Result` status check (see `.github/workflows/ci.yml`) must pass.
- CI uses path-based change detection — only affected crate tests run on PRs.
