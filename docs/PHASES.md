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

## Phase 2 — Control-plane API + live UI graph — ✅ done

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
- [x] **DAG topology in the stream + store** (prereq for real edges): added
  `NodeMeta {id, depends_on}` to `harness-dag`, carried on `RunEvent::RunStarted`
  (live graph) and `RunReport`/`RunDetail` (historical), persisted as a `graph`
  jsonb column on `harness_workflow_runs` (idempotent `ADD COLUMN IF NOT EXISTS`).
- [x] **Re-skin to the `home-ops-agent` design system** (PLAN §10.0): OKLCH
  neutral+orange palette + Geist/Geist Mono (`@fontsource-variable`) in
  `globals.css`, with the legacy `--bg/--ink/--line/--rust` tokens **aliased** onto
  the new palette so the whole app re-skins with near-zero per-component churn.
  Stayed on Tailwind 3 (no risky v4 migration); added `cn()` + lightweight
  `card`/`badge`/`button` primitives.
- [x] **Removed the fixed Kanban as the primary view:** `/` is now the Runs
  experience; the legacy task dashboard (incl. `Active.tsx`) is parked at `/tasks`.
- [x] Web: **per-run workflow graph** (`@xyflow/react` + `@dagrejs/dagre`,
  `components/runflow/`) — renders *that run's* actual DAG (edges from
  `depends_on`) with: **active step** = colored ring + spinner + animated incoming
  edge; **elapsed-time** badge (client-ticked from `started_at` via `useNow`);
  **hover → details** card (status/provider/model/tokens/iterations/duration +
  output snippet). Driven by `useRunView` (live SSE accumulator → persisted
  `RunDetail` once finished). 15 unit tests (reducer, layout, format, aggregation).
- [x] per-node `started_at`/`ended_at` already on `RunReport`/`run_nodes`; surfaced
  in the graph + overview.
- [x] Web: **Runs list** — flat list (workflow/status/steps/time), click → graph;
  inline **submit-a-run** form (`POST /runs`). Live updates via the run SSE.
- [x] **Token task overview:** per-step table (in/out/total + duration + status)
  and **per-model** aggregation. *Follow-up:* `$` cost from a `model_prices`
  table (tokens only for now; flagged).

**Exit:** ✅ submit the sample workflow from the browser, watch nodes go
pending→running→done in the graph, and see token totals per step and per model.
*(Cost-in-dollars and HTTP integration tests remain as follow-ups.)*

---

## Phase 3 — Pi / Kimi provider — ✅ done (core)

**Goal:** the author's provider mix (Claude + Kimi/Pi + Codex) works in one DAG.

- [x] **Spike done** (PLAN §7.3): the Pi family ships a headless CLI (`omp -p
  --mode json` / RPC) with JSONL events, token usage incl. cache, Kimi model
  selection, and `--resume` session ids → **CLI subprocess, no sidecar**;
  standardize on the **`omp`** fork.
- [x] **Pi adapter** (`harness-runner/src/pi.rs`): a session-aware `PromptAgent`
  (not a `CodeAgent` — that boundary is session-less) that runs `omp -p --mode
  json --model <provider/model>` (+ `--resume <id>`). A pure, unit-tested
  `parse_omp_stream` reads the JSONL event stream — `session` header `id`
  (threads `context: shared`), assistant `message_end`/`agent_end` text, and
  `agent_end.telemetry.usage` (full token fidelity incl. cache, camelCase keys).
  Auth via `MOONSHOT_API_KEY` (inherited); binary/timeout via `OMP_CLI`/
  `OMP_TIMEOUT_SECS`. Model: bare names prefixed `kimi-code/`.
- [x] **Provider dispatch** (`harness-runner/src/dispatch.rs`): `DispatchAgent`
  routes `provider: pi|omp|kimi` → `PiAgent`, everything else → the existing
  `CodeAgentRunner` (claude/codex/anthropic-api). Wired into both `execute_run`
  (CLI) and the server's `POST /runs`. Claude + Codex still select per-node.
- [x] `examples/multi-provider.yaml`: a Claude + Pi/Kimi + Codex DAG; runs
  end-to-end (echo verified; `--real` invokes the CLIs).
- [ ] *(deferred to Phase 6 runner image, PLAN §7.5):* bake `omp` + language
  servers; enable adopt-list extensions (hashline edit, LSP-on-write, native
  search, `omp commit`, `conflict://`, `pr://`, `AGENTS.md` ingestion). Also
  deferred: per-provider concurrency semaphore; `[agents.pi]` config block
  (today it's env-driven).

**Exit:** ✅ a workflow with a Claude node, a Pi/Kimi node, and a Codex node all
run, each on its declared provider/model (`examples/multi-provider.yaml`).
*(Runner-image baking of `omp` + extensions lands with Phase 6.)*

---

## Phase 4 — Bundled default pipeline (kimi + codex) — ✅ done

**Goal:** ai-harness **ships the author's `idea-to-pr`
pipeline as a built-in default**, runnable by name, and uses it as the standard
workflow for new runs — the way Archon ships `.archon/workflows/defaults/`.
(Reframed from a generic "archon-import": the formats already align, so we bundle
*this* pipeline + its commands rather than build a translator.)

- [x] **Bundled from `MarkNygaard/niles`** (the author's battle-tested version): the
  workflow YAML + its 5 referenced command markdowns, **de-prefixed** (`plan-setup`,
  `confirm-plan`, `implement-tasks`, `validate`, `finalize-pr`), compiled into the
  binary under `harness-runner/defaults/{workflows,commands}` via `include_str!`
  (`harness-runner/src/defaults.rs`). (niles has no `commands/` dir — it uses
  Archon's stock command library; sourced those from the local Archon defaults.)
- [x] **Engine gap closed:** `LoopConfig` gained `provider`/`model`; the loop runner
  prefers loop-level over node/workflow (the kimi self-review + final-verify loops
  declare them *inside* `loop:`). New dag test covers it.
- [x] **Name-based resolution + project override:** `resolve_workflow_source` takes
  a path **or** a bare name → project `.harness/workflows/<name>.yaml` → bundled
  default; `LocalRunner::resolve_command` falls back to bundled commands after the
  project dirs. Wired into `execute_run`, the `harness run` CLI (workflow now
  optional), and the server's `POST /runs`.
- [x] **Made it the default:** `DEFAULT_WORKFLOW` is used when no workflow is named
  (CLI omitted / empty request); the UI submit form defaults to it.
- [ ] *Deferred:* per-node `maxBudgetUsd` + per-run cost ceiling; a live real PR run
  (`gh`/creds/the `/simplify` pi extension) → Phase 6.

**Exit:** ✅ `harness run idea-to-pr` (or no arg)
resolves the bundled workflow + commands and executes every node, each on its
declared provider/model — echo-verified end-to-end (the prompt + the `plan-setup`/
`confirm-plan` *command* nodes resolve from the bundle). The real PR-opening run
lands with the Phase 6 runner image (`gh`/creds/toolchains).

---

## Phase 4.5 — Visual workflow editor (React Flow builder) — ✅ done

**Goal:** build/edit workflows in the UI from tested building blocks — set
provider/model per step in a properties drawer — instead of hand-writing YAML.
Built on the **free MIT `@xyflow/react`** core (no React Flow Pro). A dedicated
builder screen (palette + canvas + drawer), distinct from the read-only run-graph
but sharing node components + Dagre.

- [x] **Shared authoring core** (`harness-runner/src/authoring.rs`, PR #13):
  `validate_workflow` (parse + cycle check → structural errors), `list`/`get`/
  `save` (project shadows bundled; save refuses invalid + unsafe names), and a
  `catalog` (node kinds, provider/model hints, commands). Exposed at
  `/api/authoring/*`. The editor **and** the Phase 4.6 MCP server share it.
- [x] **Builder layout** (`web/src/routes/editor/WorkflowEditor.tsx`, PR #14):
  left palette, center canvas, right properties drawer; "Tidy" re-runs Dagre.
  Routes `/editor` + `/editor/:name`; "Editor" in the sidebar nav.
- [x] **Palette = real node kinds** (Agent step / Command / Shell / Loop / Script),
  click or drag onto the canvas; 1:1 with the DAG. Per-node configure + delete.
  **No run controls** — execution stays on the Runs page.
- [x] **Branch/join = pure topology:** connect edges (= `depends_on`); the joining
  node's `trigger_rule` is set in the drawer. No special node types.
- [x] **Properties drawer:** id, kind (switchable), body (prompt / bash / command
  w/ catalog autocomplete / script+runtime / loop), provider+model (catalog
  hints), context, trigger_rule, timeout. Renaming a node rewrites its edges.
- [x] **Round-trip on the DAG model** via js-yaml (`lib/workflow-yaml.ts`): canvas
  ⇄ `EditorWorkflow` ⇄ YAML; live validation against `/api/authoring/validate`;
  **Save** writes to `.harness/workflows/<name>.yaml`. Loads bundled + project
  (saving a bundled one creates a project copy). 16 unit tests across the two PRs.
- [ ] *Follow-ups:* mid-edge **`+` insert** (v1 = palette-add + drag-connect +
  delete); persisted node positions (re-laid out via Dagre on load today); HTTP
  integration tests for `/api/authoring/*` (Linux/CI-only fixture); a web CI job
  (web is currently verified locally — tsc + 159 vitest + build — not in CI).

**Exit:** ✅ build a workflow from scratch in the editor — palette → connect →
configure provider/model per step → live-validate → save to `.harness/workflows`
— then run it from the Runs page, without touching a YAML file.

---

## Phase 4.6 — MCP workflow-authoring server — ✅ done (core)

**Goal:** let people build/edit workflows *with their AI* (Claude, etc.) over MCP
— no UI required. A **second front-end to the Phase 4.5 authoring core**, added to
the existing MCP server (`harness-cli/src/cmd/mcp_server.rs`) so both behave
identically.

- [x] **Catalog tool** `workflow_catalog` — node kinds, provider/model hints,
  commands (bundled + project), context modes, trigger rules: the legal building
  blocks, matching the editor's palette.
- [x] **Read tools** `workflow_list`, `workflow_get <name>` (bundled + project) —
  learn from / edit the existing default.
- [x] **`workflow_validate <yaml>`** → structured `parse_workflow` + cycle errors
  (unknown dep, body exclusivity, …). The build→validate→fix loop. The *tool*
  succeeds even when the workflow is invalid (the result reports it), so an AI can
  iterate.
- [x] **`workflow_save <name> <yaml>`** → validate-then-save to project
  `.harness/workflows/` (refuses invalid + unsafe names).
- [x] MCP tests (tool list + validate good/bad + catalog).
- [ ] *Deferred:* `dry_run` (echo topology preview) and `start_run` — they need run
  plumbing into the stdio server; folded into the Phase 5 `run/start` trigger work.

**Exit:** ✅ from an MCP-connected assistant, read the catalog + default pipeline,
describe a change in natural language, and have the AI edit → `workflow_validate`
→ `workflow_save` it. (Triggering the run over MCP lands with Phase 5.)

---

## Phase 5.0 — Projects (repo-scoped runs) — 🔨 in progress

**Goal:** runs are scoped to a **project** (a git repo), not one global
`project_root`. Register Ticket0 / ai-harness / niles, trigger runs into the
right repo, and see them all in one mixed feed. Prerequisite for Linear (sources
are per-project). Decisions (with the author): **persistent PVC clone** per
project, **one global GitHub token**, **projects-only first pass** (manual
triggers; Linear layered on after).

- [x] **Registry** (`harness-persist::ProjectStore` + `harness_projects` table):
  `name` (slug + checkout dir) · `git_url` · `base_branch` · `default_workflow`.
  Idempotent `CREATE TABLE IF NOT EXISTS`; CRUD + a DB round-trip test.
- [x] **API** `/api/projects` (list/register/get/delete). Register clones `git_url`
  into `projects_dir/<name>` (sibling of `project_root`, overridable via
  `HARNESS_PROJECTS_DIR`); re-register `git fetch`es. **`base_branch` is optional —
  auto-detected from the repo's `origin/HEAD`** after clone (so a repo whose default
  is `develop` is picked up without asking), falling back to `main`. A clone failure
  still saves the row and returns a `warning` (bad URL / missing token).
- [x] **Global GitHub credential**: `github` provider in the encrypted credential
  store (`token`) → `GH_TOKEN`/`GITHUB_TOKEN` at run time + a transient git
  credential helper for private clone/fetch (token never written to repo config).
- [x] **Project-scoped runs**: `POST /api/runs` **requires** `project` (400
  otherwise — there is no global-root fallback; every run lives in a project). The
  run fetches the checkout and cuts an **isolated worktree** off
  `origin/<base_branch>` (concurrent same-project runs don't collide); a setup
  failure fails the run visibly. `project` persisted on the run row (column already
  existed) and surfaced in the list/detail. A project's `default_workflow` fills an
  empty `workflow`; its stored (auto-detected) `base_branch` is the default.
- [x] **UI**: Projects page (register form + list with remove) + sidebar nav; a
  **project selector** on the run-trigger form (pre-fills the default workflow);
  the GitHub-token field on the Credentials page; project badge on run rows.
- [ ] *Follow-ups:* resolve project workflows from the project's own
  `.harness/workflows` (today: global + bundled); per-project credential scope if
  ever needed (global for now); surface clone/worktree status in the UI; HTTP
  integration tests for `/api/projects` (CI fixture).

**Exit:** register two projects from the UI, trigger a run into each, and watch
both appear in one Runs feed — each operating on its own repo's worktree.

---

## Phase 5 — Linear source + cron triggers — ⬜ not started

**Goal:** a cron job polls Linear and triggers workflows; status flows back.
Linear sources are configured **per project** (Phase 5.0) — a saved filter maps
to a project + workflow.

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
- [ ] **Credentials & secrets — fully UI-managed, no cluster access required.**
  The author must be able to connect every provider **from the UI** — *not* by
  editing Helm values, hand-writing SOPS secrets, or `kubectl exec`-ing in to run
  `claude login` / `codex login` by hand. Pieces:
  - **Auth broker** (control plane): per provider, spawns the *official* CLI login
    so usage attributes to the **normal subscription** (we run Anthropic's `claude`
    / OpenAI's `codex` binaries — the allowed path — never a hand-rolled OAuth
    client or a third-party token bucket). Captures the resulting credential
    **files** (`~/.claude/.credentials.json`, `~/.codex/auth.json`) — which carry
    refresh tokens, so they self-renew — not a bare long-lived token.
  - **Browser-authorize UX:** UI shows a "Connect Claude" / "Connect Codex" button
    → the official consent page opens → click Authorize. Likely lands as
    *authorize-URL + paste-the-returned-code* (those CLI logins use a `localhost`
    callback built for a local CLI+browser; a server deployment can't catch that
    redirect). ⚠️ **Verify per CLI** whether `claude login` / `codex login` expose
    a headless/manual-code variant; if one is localhost-only, fall back to
    local-login-then-import for that provider. Kimi stays a simple API key / `omp`
    login.
  - **Runtime store, not GitOps:** the control plane keeps these in an
    **encrypted, app-managed store** (encrypted Postgres rows, or a k8s `Secret`
    the control plane writes/rotates via the API) — added/rotated live from the UI
    with **no redeploy and no Flux/SOPS commit**. Per-project or global scope.
  - **Pod materialization:** the runner bootstrap writes the creds into
    `~/.claude` / `~/.codex` (+ `MOONSHOT_API_KEY`) before node 1; **credential
    health** indicator in the UI (expiry / last-refreshed / revoked).
- [ ] **Flux deployment** (matches `home-ops`): `kubernetes/apps/<ns>/ai-harness/
  {app,cluster}` with a `HelmRelease`/kustomize base, a **CloudNativePG** `Cluster`
  (not our own Postgres), **Envoy Gateway** `HTTPRoute` + cert-manager TLS. Note:
  **SOPS+age** Secrets cover only *bootstrap/infra* secrets (DB creds, the app's
  own signing keys) — **agent/provider credentials are the UI-managed runtime
  store above**, deliberately *not* GitOps-managed so connecting a provider never
  needs a cluster login or a commit.

**Exit:** from a clean cluster, register a project, **connect Claude + Codex +
Kimi entirely from the UI** (browser-authorize, no `kubectl`/SOPS), set toolchains,
trigger the flagship workflow from Linear, and watch it provision toolchains +
run to a PR — entirely in-cluster, no hand-built wrapper image, no hand-managed
secrets.

> Cluster is now mapped (PLAN §14). Remaining author inputs: target namespace,
> internal-vs-external exposure, and the warm-cache storage choice (recommend
> per-node PVC + node affinity).

---

## Phase 7 — Hardening & polish — ⬜ not started

**Goal:** production-ready for a single-operator cluster.

- [ ] Secret redaction in logs/OTLP; retry/idempotency on runner crashes (lease
  recovery via the reducer machinery).
- [ ] **Per-node tool/capability restrictions (workflow fidelity).** Today *every*
  agent node runs with full tools (edit/bash/git/gh, `DangerFullAccess`); a node's
  role is enforced only by its prompt. The first real run proved this is
  insufficient: the `plan-setup` step **implemented the change, committed, pushed,
  and opened the PR** even though its prompt explicitly said "This step does NOT
  implement anything" / "does NOT create a PR" — a capable model given a tiny,
  fully-specified task just did it. Fix: a node-level `tools`/`capabilities` field
  in `harness-dag` (e.g. `read-only`, `no-vcs`, `implement`, `pr`) that the runner
  **enforces** — read-only nodes physically cannot edit/commit/push/PR. Prompt
  hardening (done for plan-setup/confirm-plan) is a stopgap, not the fix.
- [ ] **Rich task overview** (Factory-style, PLAN §10.1): time waterfall /
  milestone Gantt of the DAG, token panels by type/step/model, per-run cost
  dashboards, loop per-iteration token bars.
- [ ] Re-enable optional majiayu features if wanted (policy rules, skills, GC).
- [ ] **Remove the legacy majiayu *task* subsystem from the server.** The UI for
  it (Kanban/Tasks, Overview, Worktrees, old shell) was deleted in the shadcn
  migration (PR #24), but the Rust side is still present and dormant: the `/tasks`
  routes + `task_routes`/`task_mutation_routes`/`task_query_routes`/
  `task_submission_routes`, the `task_executor` + `task_runner`, the durable
  `workflow_runtime`/runtime-hosts machinery, and the related `/api/dashboard`,
  `/api/overview`, `/api/worktrees`, `/api/workflows/runtime/*`, `/api/runtime-hosts/*`
  routes + their handlers/stores. None of it is used by the `/api/runs` (harness-dag)
  path. Removing it is a large, careful pass (many modules + tests) — do it as its
  own PR, keeping `/api/runs`, `/api/authoring`, `/api/credentials`, the SPA serving,
  and the GitHub/webhook intake we still want. Also drop the now-orphaned
  `crate::dashboard`/`crate::overview` HTML routes + the `/overview`,`/worktrees`
  server GET routes.
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
