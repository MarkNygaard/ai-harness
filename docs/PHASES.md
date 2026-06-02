# ai-harness — Phased Roadmap

Step-by-step build plan **and live checklist**. Each phase has a **goal**,
checkboxed **work** items, a **Status**, and an **exit criterion** (a concrete,
demoable thing that proves the phase is done). We tick items off as they land and
add carried-over tasks under **Reminders**. Phases are ordered so the system is
runnable end-to-end as early as possible, with the big differentiators
(toolchains, Linear, k8s) layered on after a working local core.

Legend: `[x]` done · `[~]` in progress / partial · `[ ]` not started.
Phase status: ✅ done · 🔨 in progress · ⬜ not started.

See [PLAN.md](PLAN.md) for the architecture these phases implement.

---

## Phase 0 — Seed & green build — 🔨 in progress

**Goal:** `ai-harness` repo builds from a copy of majiayu-harness.

- [x] Copy the majiayu-harness workspace into `ai-harness` (MIT `LICENSE` +
  attribution preserved).
- [x] Identity reset: `repository` URL → `MarkNygaard/ai-harness`, version
  `0.6.34 → 0.1.0`, fresh README/CHANGELOG, removed stray cruft.
- [x] Keep `harness-*` crate names + `harness` binary (decided — no rename).
- [x] Moderate cleanup: archived reusable specs to `docs/reference/`, deleted
  majiayu project-history docs + `sdk/python`; restored `sdk/typescript` (web
  depends on it); stripped crates.io release automation; pruned majiayu bot
  workflows (kept generic CI).
- [x] Rewrote `AGENTS.md` (canonical) + `CLAUDE.md` (pointer) for ai-harness.
- [x] `cargo build --workspace` green; `cargo check`/`clippy`/`test` green.
- [ ] Bring up Postgres via compose; `harness serve` boots; `/health` responds.
- [ ] First commit (deferred by user until we're further along).

**Exit:** clean build + booting server from the new repo, committed.
*(Build ✅; serve/health + commit still open.)*

---

## Phase 1 — DAG model & local executor — 🔨 in progress

**Goal:** run a multi-node workflow locally (no k8s, no Linear yet).

- [x] New crate `harness-dag`: serde node-schema types (PLAN §6), YAML loader,
  Kahn topological layering, cycle detection, template-variable substitution
  (fail-loud on missing vars).
- [x] **Loop signal detection** (`signal.rs`): tag-wrapped `<x>SIGNAL</x>`,
  end-of-output, and own-line forms; restrictive to avoid false positives.
- [x] **Executor core** (`exec.rs`): `NodeRunner` trait + `run_workflow` driver —
  sequential/parallel layers, `context: fresh|shared` session threading,
  `trigger_rule` evaluation, `$VAR` substitution before dispatch, loop iteration
  + `$LOOP_PREV_OUTPUT` + convergence, cancel handling, and token-`Usage`
  aggregation into a serde `RunReport`. Execution is a trait seam so the same
  driver runs locally, in k8s, or against a mock. **29 tests + doc-test green.**
- [~] `LocalRunner` (`harness-runner`) impl of `NodeRunner`: real `bash`
  subprocess (workspace cwd, timeout via `kill_on_drop`, exit-code → success),
  `.harness/commands/` resolution + path-traversal guard + `$VAR` substitution,
  and `prompt`/loop dispatch via a session-aware `PromptAgent` seam.
- [x] **Real agent backend**: `CodeAgentRunner` (`PromptAgent`) wraps majiayu's
  `AgentRegistry` — resolves `provider` → registered `CodeAgent` (Claude/Codex/
  Anthropic API), runs `CodeAgent::execute`, maps output + `TokenUsage`. 13 tests
  in `harness-runner`. **Session gap addressed as fresh-per-node** (documented):
  `CodeAgent` has no session resume, so `context: shared` doesn't thread through
  it yet — closing it needs a trait extension (tracked in Reminders).
- [x] **Config-built registry + real-agent run:** `build_agent_registry(config)`
  registers Claude/Codex/(Anthropic API) from `HarnessConfig`; `harness-run --real
  [--sandbox <mode>]` now drives the workflow with `CodeAgentRunner` instead of
  `EchoAgent`. (Echo remains the default for dependency-free demos.)
- [x] **`script` runtimes** (`bun` for TS/JS via temp file; `uv run --with <dep>`
  for Python) and **loop `until_bash`** (exit 0 = converge, run through the
  runner's Bash path — no new trait method). Tested (bun script + until_bash loop).
- [x] **Git worktree per run** (`harness-runner::Worktree`): `harness-run
  --worktree` runs in an isolated `git worktree add -b` checkout (HEAD), removed
  on drop. The one deliberate `git`-subprocess exception (run infrastructure,
  documented), mirroring majiayu's `WorkspaceManager`.
- [x] **`--config <file>`**: `harness-run --config <toml>` loads `HarnessConfig`
  (toml + `rebase_relative_paths`) for the real-agent registry, vs the default.
- [x] **Folded into the main CLI as `harness run`**: extracted the run lifecycle
  into a shared `harness_runner::execute_run`/`RunOptions` (used by both the
  standalone `harness-run` bin and the new `harness run` subcommand);
  `harness run <wf.yaml> [--real --worktree --database-url …]`. Verified
  end-to-end (echo + persisted to local Postgres).
- [~] Run-level wiring: `EchoAgent` (built-in dev `PromptAgent`) + a
  `harness-run <workflow.yaml>` binary that builds the `VarContext`
  (`$ARTIFACTS_DIR`/`$BASE_BRANCH`/…), drives `run_workflow` via `LocalRunner`,
  and prints a per-node `RunReport` summary. Demo: `examples/hello.yaml` runs
  end-to-end (parallel layer + join). **Remaining:** real git worktree per run,
  and fold into the main `harness` CLI as `harness run` (once real agents land).
- [x] **Persist runs** — new `harness-persist` crate (`RunStore`): idempotent
  Postgres schema (`harness_workflow_runs` + `harness_run_nodes` with status,
  provider/model, token usage, iterations, converged, note, `started_at`/
  `ended_at`); records a `RunReport` in one transaction. Wired into `harness-run`
  via `--database-url`/`$HARNESS_DATABASE_URL`. Depends only on `harness-dag`+`sqlx`
  so the server can reuse it. (`node_invocations` loop-drill-down deferred; SQL
  validated on the CI Postgres job — Docker wasn't running locally.)

**Exit:** a 3-node sample workflow (bash → claude prompt → loop) runs to
completion locally and its run/node state is queryable.
*(Orchestration ✅ via mock; real `LocalExecutor` + persistence still open.)*

---

## Phase 2 — Control-plane API + live UI graph — 🔨 in progress

**Goal:** submit a run from the UI and watch it execute as a graph.

- [x] **Control-plane API** (`harness-server/src/http/runs_routes.rs`, attached via
  an axum `Extension` to avoid entangling `AppState`): `POST /runs` (submit →
  background `run_workflow_streaming` via `LocalRunner` + echo/real agent),
  `GET /runs` + `GET /runs/{id}` (from `harness-persist`), and **`GET
  /runs/{id}/stream`** = **SSE** of live `RunEvent`s (matches the existing
  `/tasks/{id}/stream` pattern; futures-channel→broadcast bridge). `harness-persist`
  gained `list_runs`/`get_run` (+ `RunSummary`/`RunDetail`, DB-tested).
  *Follow-ups:* runs persist on completion (404 until done — live via SSE); add a
  `running` status + insert-on-start; HTTP integration tests (server fixture, CI).
- [ ] **Re-skin to the `home-ops-agent` design system** (PLAN §10.0): port its
  OKLCH theme tokens (`globals.css`), orange accent, Geist/Geist Mono, shadcn/ui
  primitives into our Vite app. Reference its
  `web/src/components/dashboard/agent-flow.tsx` (React Flow + Dagre) as the graph
  template.
- [ ] **Remove the fixed Kanban** (`Active.tsx` + hardcoded `COLUMNS`/`TaskCard`,
  tied to the single `github_issue_pr` pipeline).
- [ ] Web: **per-task workflow graph** (React Flow + Dagre) as the primary task
  view — render *that run's* actual DAG with a live overlay: node state colors,
  edges from `depends_on`, loop iteration counts. Author's asks (PLAN §10):
  **active step** = orange gradient ring + animated incoming edge + pulse;
  **elapsed-time badge** ("running 2m14s", client-ticked from `started_at`);
  **hover → tooltip** with status/provider/model/tokens/cost/duration + output
  snippet; click pins/expands. Backed by `RunReport` + per-node timestamps.
- [ ] Add per-node `started_at`/`ended_at` to the executor/`RunReport` +
  `run_nodes` (needed for the elapsed-time badge and the §10.1 waterfall).
- [ ] Web: **Runs list** — flat, filterable list (workflow/project/status/timing),
  *not* fixed-stage columns; click a run → its graph. Live updates over WS.
- [ ] **Token capture:** adapters return `Usage` per invocation; persist on
  `run_nodes` (+ `node_invocations`). Basic **task overview**: token totals + a
  **per-step** and **per-model** breakdown, derived `$` from a `model_prices`
  table. (Rich Gantt/task visuals land in P7.)

**Exit:** submit the Phase 1 sample workflow from the browser, watch nodes go
pending→running→done in the graph, and see correct token + cost totals broken
down per step and per model.

---

## Phase 3 — Pi / Kimi provider — ⬜ not started

**Goal:** the author's provider mix (Claude + Kimi/Pi + Codex) works in one DAG.

- [x] **Spike done** (PLAN §7.3): the Pi family ships a headless CLI (`omp -p
  --mode json` / RPC) with JSONL events, token usage incl. cache, Kimi model
  selection, and `--resume` session ids → **CLI subprocess, no sidecar**;
  standardize on the **`omp`** fork.
- [ ] Implement the Pi adapter (a session-aware `PromptAgent`): `omp -p --mode
  json --model kimi-code/<model>`, parse `agent_end` + `SessionStats`, thread
  `context: shared` via captured `SessionHeader.id` + `--resume`; per-provider
  concurrency semaphore.
- [ ] **Runner image / Pi config (PLAN §7.5):** bake `omp` + language servers;
  enable the adopt-list extensions (hashline edit, LSP-on-write, native search,
  `omp commit`, `conflict://`, `pr://` reads, `AGENTS.md` ingestion); skip
  ACP/browser/DAP/eval for headless. Decide `.omp` vs `.harness` config home.
- [ ] Confirm Codex + Claude adapters still select per-node.

**Exit:** a workflow with a Claude node, a Pi/Kimi node, and a Codex node all run
and stream correctly, each on its declared provider/model.

---

## Phase 4 — Archon import + author's pipeline parity — ⬜ not started

**Goal:** the real `idea-to-pr-with-kimi-coding-and-codex` workflow runs.

- [ ] `harness archon-import <path>`: map Archon YAML → our format; copy bundled
  `command` markdown (`archon-plan-setup`, `archon-implement-tasks`,
  `archon-validate`, `archon-finalize-pr`, …) into `harness` defaults.
- [ ] Port `$BASE_BRANCH` auto-detection, the lockfile-detection install bash
  node (becomes the default toolchain `setup` recipe in Phase 6), PR
  finalize/verify nodes, the kimi self-review loop, codex pass, sonnet sign-off.
- [ ] Per-node `maxBudgetUsd` + per-run cost ceiling.

**Exit:** the author's flagship workflow runs locally end-to-end and opens a PR
with the same shape it produces in Archon today.

---

## Phase 5 — Linear source + cron triggers — ⬜ not started

**Goal:** a cron job polls Linear and triggers workflows; status flows back.

- [ ] `harness-sources`: Linear GraphQL client; saved filter → workflow+args
  mapping; interval poller in the control plane; dedupe by Linear issue id
  (cursor in `linear_sources`).
- [ ] Write-back: comment/transition Linear issue on run start / PR open / verdict.
- [ ] UI: **Linear sources panel** (configure filter, schedule, target workflow).
- [ ] Manual/AI trigger via MCP server method (`run/start`).

**Exit:** create/label a Linear issue → within one poll interval a run starts,
and the issue gets a comment linking the run + resulting PR.

> ⚠️ Needs author input first: exact Linear filter semantics + desired write-back
> (status transition vs comment). See PLAN §12.2.

---

## Phase 6 — Kubernetes execution + toolchain provisioning — ⬜ not started

**Goal:** runs execute in ephemeral runner pods; toolchains are UI-managed.

- [ ] `harness-k8s`: `K8sJobExecutor` — Job-per-run with **free scheduling**
  (resource `requests`, no node pinning) + a **per-node `hostPath` warm cache**
  (local-path is node-local, no RWX — see PLAN §5/§12.3/§14), ephemeral workspace
  volume, log/event streaming back to the control plane; RBAC (ServiceAccount +
  namespaced Role to manage Jobs). Cap concurrency (no autoscaler → `Pending`).
- [ ] Prebuilt **runner image**: git + agent CLIs (`claude`, `codex`, `pi`) + `mise`.
- [ ] `harness-toolchain`: per-project toolchain spec (Postgres + `.harness/
  toolchains.toml`); bootstrap step runs `mise` + `setup` commands before node 1;
  caches via the per-node `hostPath` mount (`CARGO_HOME`/`PNPM_STORE_DIR`/…).
- [ ] UI: **Toolchains panel** (catalog pick + custom setup + "Test provisioning"
  streams logs from a throwaway pod). **No image rebuild to add a toolchain.**
- [ ] **Flux deployment** (matches `home-ops`): `kubernetes/apps/<ns>/ai-harness/
  {app,cluster}` with a `HelmRelease`/kustomize base, a **CloudNativePG** `Cluster`
  (not our own Postgres), **Envoy Gateway** `HTTPRoute` + cert-manager TLS, and
  **SOPS+age** Secrets for provider/Linear/GitHub tokens.

**Exit:** from a clean cluster, register a project, set its toolchains in the UI,
trigger the flagship workflow from Linear, and watch it provision toolchains +
run to a PR — entirely in-cluster, no hand-built wrapper image.

> Cluster is now mapped (PLAN §14). Remaining author inputs: target namespace,
> internal-vs-external exposure, and the warm-cache storage choice (recommend
> per-node PVC + node affinity).

---

## Phase 7 — Hardening & polish — ⬜ not started

**Goal:** production-ready for a single-operator cluster.

- [ ] Secret redaction in logs/OTLP; retry/idempotency on runner crashes (lease
  recovery via the reducer machinery).
- [ ] **Rich task overview** (Factory-style, PLAN §10.1): time waterfall /
  milestone Gantt of the DAG, token panels by type/step/model, per-run cost
  dashboards, loop per-iteration token bars.
- [ ] Visual **workflow editor** (build/edit DAGs in the UI, export YAML).
- [ ] Re-enable optional majiayu features if wanted (policy rules, skills, GC).
- [ ] Docs: operator runbook, workflow-authoring guide, Archon-migration guide.

**Exit:** documented, observable, recoverable; the author's daily workflow lives
on ai-harness instead of Archon.

---

## Reminders / carried-over tasks

- **Branch `feat/seed-dag-engine` → PR #1** open against `main`; **CI fully green**
  (clippy `-D warnings`, check, test-with-Postgres, web, disabled-modules). Ready
  to squash-merge on the author's go.
- **Inherited-warnings cleanup: not needed** — the harness-sandbox/harness-agents
  warnings were Windows-only `cfg(unix)` artifacts; Linux CI is clean.
- **Verify `harness serve` boots + `/health`** against local Postgres (Phase 0
  exit item still open).
- **Web build is stub-only in dev** here (bun not producing a real bundle); the
  full UI rebuild happens in Phase 2. `sdk/typescript` retained because `web/`
  imports its types.
- **Open inputs needed before** Phase 5 (Linear filter/write-back semantics).
  Phase 6 cluster is now mapped from `home-ops` (PLAN §14); only namespace,
  internal/external exposure, and warm-cache storage choice remain.
- **Target cluster = `home-ops`** (Talos + Flux + Envoy Gateway + CloudNativePG +
  SOPS/age + local-path). ai-harness deploys as a Flux app; uses CNPG for
  Postgres. Storage is node-local (no RWX) → **free scheduling + per-node
  `hostPath` warm cache** (no pinning); cap concurrency (no autoscaler).
- **Pi adapter = `omp` CLI subprocess** (resolved; PLAN §7.3). `omp -p --mode
  json --model kimi-code/<model>`, usage incl. cache, `--resume` sessions.
- **omp extensions triage (PLAN §7.5):** adopt tool-level wins (hashline edit,
  LSP-on-write, native search, `omp commit`, `conflict://`, `pr://`, `AGENTS.md`).
  **`swarm-extension` = our `harness-dag`** — reference/validation, NOT adopted
  (we own orchestration in Rust). Skip ACP/browser/DAP/eval headless. Memory
  (Hindsight) needs PVC persistence; resolve `.omp` vs `.harness` config home.
- **Session-resume gap:** the DAG driver threads sessions for `context: shared`,
  but majiayu's `CodeAgent` (Claude/Codex via `CodeAgentRunner`) has **no session
  id** — currently fresh-per-node. **Pi via `omp --resume` closes it for Pi
  nodes**; the same CLI-`--resume` trick is the likely fix for Claude/Codex.
- **LocalRunner gaps to close:** `script` (bun/uv) and `until_bash` are stubbed
  (script returns an explicit "unsupported" error for now).
- **UI design source = `home-ops-agent`** (`C:\Users\Mark\Github\home-ops-agent`):
  shadcn/ui + Tailwind v4 OKLCH theme, orange accent, Geist fonts; its
  `web/src/components/dashboard/agent-flow.tsx` (React Flow + Dagre) is the
  per-task-graph template. Its hardcoded agent pipelines (PR-review modes, alert
  triage) are reference for graph shape. Adopt in Phase 2.
- **Per-node timestamps**: add `started_at`/`ended_at` to the executor/RunReport
  + `run_nodes` for the live elapsed-time badge and the §10.1 waterfall.

---

## Dependency order (at a glance)

```
P0 seed ─▶ P1 DAG+local ─▶ P2 API+UI graph ─▶ P3 Pi ─▶ P4 Archon parity
                                                              │
                                       ┌──────────────────────┤
                                       ▼                      ▼
                                P5 Linear+cron          P6 k8s+toolchains
                                       └──────────┬───────────┘
                                                  ▼
                                            P7 hardening
```

P5 and P6 are independent after P4 and can be built in parallel. P6 is the
biggest single phase; P3 carries the main external risk (Pi CLI capability) so
its spike happens early.
