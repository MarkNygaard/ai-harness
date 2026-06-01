# ai-harness — Architecture & Plan

> A Rust-native AI coding harness that runs **in Kubernetes**, is driven by
> **AI triggers or cron-polled Linear tasks**, executes **user-authored
> workflow DAGs** (Archon-style), and provisions project **toolchains from the
> UI** instead of a hand-built wrapper image.

---

## 1. Goal

Replace the author's current use of [Archon](https://github.com/coleam00/Archon)
with a new harness that fixes Archon's specific friction points for this user:

| Archon pain point | What we do instead |
|---|---|
| Designed to run on the same machine you code on | First-class **Kubernetes** deployment; the harness is a cluster service |
| You must hand-build a wrapper image to install `pnpm`, `cargo`, etc. | **Toolchain provisioning from the UI** — declare/install toolchains per project, no custom image rebuilds |
| Triggering is local/CLI/chat-centric | Trigger via **AI** *or* a **cron job that polls Linear** for tasks |
| The dashboard UI is functional but not loved | A new UI with **visual workflow-graph flows** |
| TypeScript/Bun | **Rust** (the user's preference + the majiayu-harness base is Rust) |

What we **keep** from Archon (the parts that work well):

- The **YAML workflow DAG** model: nodes with `depends_on`, per-node
  `provider`/`model`, `bash`/`script` nodes, reusable `command` nodes,
  `loop` nodes that converge on a `<promise>SIGNAL</promise>`, `$ARGUMENTS` /
  `$ARTIFACTS_DIR` / `$BASE_BRANCH` template variables.
- **Provider mixing** in one pipeline (Claude planning → Kimi/Pi implementation
  → Codex review → Claude sign-off) — this is the author's primary workflow.
- **Git worktree isolation** per run.

---

## 2. The two reference points

### 2.1 Archon (`C:\Users\Mark\Github\Archon`) — the model we port
TypeScript/Bun monorepo. Relevant packages:

- `packages/workflows` — the DAG engine. Rich node schema (see §6), topological
  layering (Kahn), parallel-within-layer execution, session threading
  (`context: fresh|shared`), loops with `until` / `until_bash` / `interactive`,
  `command` resolution from `.archon/commands/`, template-variable substitution.
- `packages/providers` — `IAgentProvider` trait; built-ins `claude`, `codex`;
  community `pi`. **Pi is an in-process JS SDK** (`@mariozechner/pi-coding-agent`),
  *not* a CLI or HTTP API — this matters for the Rust port (see §7.3).
- `packages/isolation` — git-worktree provider. **Confirmed: there is NO
  toolchain/dependency install step** — it only does worktree mechanics +
  `copyFiles`. This is exactly the gap we productize.
- `packages/server`, `packages/web` (React 19 + Radix + Tailwind + Zustand),
  `packages/git`, `packages/adapters` (Telegram/Slack/Discord/GitHub), Postgres.

### 2.2 majiayu-harness (`C:\Users\Mark\Github\majiayu-harness`) — the Rust base we seed from
MIT-licensed Rust workspace (13 crates), already provides ~70% of the plumbing:

- Multi-agent adapters (Claude CLI, Codex CLI, Anthropic API) + registry
- Postgres task/thread/turn lifecycle, **git worktree isolation**, multi-project
- HTTP + WebSocket + stdio + **MCP server**, ~42 JSON-RPC methods
- GitHub webhook automation, repo-backlog polling, PR-feedback sweeps
- React + Vite + Tailwind **web dashboard** (`web/`)
- OTLP observability, Starlark policy engine, signal-driven GC
- A `harness-workflow` crate — but it is a **fixed issue→PR runtime**, not a
  user-authored DAG. We replace its front-end with our DAG engine while keeping
  its durable runtime/worker/lease/reducer machinery.

**Decision (confirmed): seed the new `ai-harness` repo by copying the
majiayu-harness codebase**, then transform it. We are not starting from a blank
workspace and we are not vendoring it as a dependency — we own the fork.

---

## 3. Confirmed decisions

| Decision | Choice |
|---|---|
| Reuse strategy | **Copy majiayu-harness code as the seed**, then extend/refactor in-tree |
| Workflow format | **New DAG format** (Archon-inspired), with a **migration path** for existing Archon YAML |
| Agent backends at launch | **Claude** (CLI/API), **Kimi via Pi**, **Codex**, **Linear as trigger source** |
| K8s execution model | *Recommendation below* — author asked us to recommend (see §5) |

---

## 4. Target architecture

```
                        ┌──────────────────────────────────────────────┐
   Linear  ── cron ───▶ │                CONTROL PLANE                  │
   GitHub  ── webhook ▶ │  (long-lived Deployment, 1–N replicas)        │
   UI / AI ── HTTP ───▶ │                                               │
                        │  • REST/JSON-RPC API + WebSocket (live runs)  │
                        │  • Source pollers (Linear, GitHub backlog)    │
                        │  • Workflow DAG engine (scheduler/reducer)    │
                        │  • Toolchain manager (per-project specs)      │
                        │  • Postgres (durable run/queue/lease state)   │
                        │  • OTLP exporter                              │
                        └───────────────┬───────────────────────────────┘
                                        │ schedules K8s Job per workflow RUN
                                        ▼
                        ┌──────────────────────────────────────────────┐
                        │              RUNNER POD (ephemeral)           │
                        │  • git worktree for the run (ephemeral vol)   │
                        │  • toolchain bootstrap (init step)            │
                        │  • per-node warm cache (cargo/pnpm/uv store)  │
                        │  • executes the DAG's nodes:                  │
                        │      agent CLIs (claude / codex / pi)         │
                        │      bash / script nodes                      │
                        │  • streams node output → control plane (WS)   │
                        └──────────────────────────────────────────────┘
```

### Crate map (after transformation)

| Crate | Origin | Role |
|---|---|---|
| `harness-core` | majiayu (keep) | domain types, config, traits |
| `harness-protocol` | majiayu (keep) | JSON-RPC envelopes/codecs |
| `harness-server` | majiayu (adapt) | control-plane HTTP/WS/API |
| `harness-agents` | majiayu (extend) | Claude/Codex/API **+ new Pi adapter** |
| `harness-dag` | **new** | the Archon-style DAG model + executor |
| `harness-runtime` | majiayu `harness-workflow` (refactor) | durable queue, worker, lease, reducers |
| `harness-sources` | **new** | Linear poller, GitHub webhook, manual/AI triggers |
| `harness-toolchain` | **new** | per-project toolchain specs + bootstrap |
| `harness-k8s` | **new** | Job scheduling, runner pod lifecycle, log streaming |
| `harness-sandbox` | majiayu (keep) | sandbox spec/modes |
| `harness-exec` | majiayu (keep/fold) | ExecPlan model (may fold into `harness-dag`) |
| `harness-observe` | majiayu (keep) | events, OTLP, grading |
| `harness-rules` / `harness-skills` / `harness-gc` | majiayu (keep, optional) | policy/skills/GC (post-MVP) |
| `harness-cli` | majiayu (adapt) | `harness serve` / `exec` / migrate / etc. |
| `web/` | majiayu base + new graph UI | dashboard + **visual workflow flows** |

---

## 5. Kubernetes execution model — recommendation

The author asked us to recommend. Three viable shapes:

**A. One ephemeral Runner Pod (K8s Job) per *workflow run* — RECOMMENDED.**
The control plane is a long-lived Deployment. When a run starts, it creates a
K8s Job; that pod holds **one git worktree for the whole DAG** and executes all
the run's nodes (the DAG engine drives node order; the pod runs agent
subprocesses). Workspace is an ephemeral volume; a **per-node warm cache** (a
`hostPath` dir on whatever node the scheduler picks — cargo registry, pnpm store,
uv cache) is mounted to cut install time.

- ✅ **Free scheduling (what we want):** we set resource `requests` and let the
  Kubernetes scheduler place the Job on whichever node has room — no placement
  logic, no node pinning. The per-node `hostPath` cache means placement stays
  free *and* each node stays warm across runs (first run on a node is cold, then
  warm). This replaces the earlier "pin to the cache node" idea, which would have
  defeated free scheduling on a 3-node cluster.
- ✅ Clean per-run isolation; blast radius is one pod; horizontal scale is "more
  Jobs"; crashes don't take down the control plane.
- ✅ **Toolchain provisioning is natural**: an init step applies the project's
  declared toolchain spec before the first node runs (see §8).
- ✅ Worktree persists across all nodes of a run, so `context: shared`,
  `$ARTIFACTS_DIR`, and multi-node pipelines work exactly like Archon.
- ⚠️ Per-run startup latency (schedule + image pull + bootstrap). Mitigated by a
  prebuilt runner image with the agent CLIs baked in + per-node warm cache + node
  image pre-pull.
- ⚠️ **Bare-metal, no autoscaler:** with 3 fixed Talos nodes, if none has room
  the Job's pod stays `Pending` until capacity frees — it won't add a node. So
  cap concurrency (per-project `max_concurrent` + a global cap) and keep requests
  modest so runs bin-pack.
- ⚠️ Control plane needs RBAC to create/watch/delete Jobs in its namespace.

**B. Job per *node*.** Maximum isolation per node, but the worktree must be
persisted and re-hydrated between nodes (artifacts dir on a PVC), and per-node
pod scheduling latency dominates a 10-node pipeline. Rejected for MVP — too much
latency and state-shuffling for the author's pipelines.

**C. Long-lived server runs agents in-pod (majiayu today).** Simplest, fastest
per-task startup, but one noisy-neighbor pod, vertical scaling only, toolchains
installed once into the pod/PVC (closer to the Archon pain point we're fixing).
Good fallback / "dev mode", not the target.

**Recommendation: A (Job-per-run), with C available as a `local`/dev execution
backend** behind the same `Executor` trait so we can develop without a cluster.
Make the executor pluggable from day one:

```rust
#[async_trait]
trait RunExecutor {
    async fn start_run(&self, run: RunSpec) -> Result<RunHandle>;
    async fn stream(&self, h: &RunHandle) -> impl Stream<Item = NodeEvent>;
    async fn cancel(&self, h: &RunHandle) -> Result<()>;
}
// impls: LocalExecutor (subprocess + worktree), K8sJobExecutor (Job-per-run)
```

---

## 6. Workflow DAG format (new, Archon-inspired)

We design a **new** format (decision: "new DAG, migrate later") that keeps
Archon's ergonomics but is typed in Rust (serde) and tightened for k8s. Archon's
real node schema (reverse-engineered) is the reference superset:

**Node common fields:** `id`, `depends_on[]`, `when` (conditional),
`trigger_rule` (`all_success` | `one_success` | `none_failed_min_one_success` |
`all_done`), `retry`, `idle_timeout`, `hooks`.

**AI-node fields:** `provider`, `model`, `context` (`fresh` | `shared`),
`output_format` (JSON schema), `allowed_tools[]` / `denied_tools[]`, `mcp`,
`skills[]`, inline `agents{}`, `effort` (`low|medium|high|max`), `thinking`,
`maxBudgetUsd`, `systemPrompt`, `fallbackModel`, `betas[]`, `sandbox`.

**Node-type discriminators (mutually exclusive):**
- `command: <name>` — resolved from `.harness/commands/` (repo) then user/global
- `prompt: <text>` — inline prompt
- `bash: <script>` (+ `timeout`)
- `script: <text>` + `runtime: bun|uv` + `deps[]` (+ `timeout`)
- `loop: { prompt, until, max_iterations, fresh_context?, until_bash?, interactive?, gate_message? }`
- `approval: { message, capture_response, on_reject }` — human gate
- `cancel: <reason>` — terminate run

**Loop convergence** (`until`): detect `<anytag>SIGNAL</anytag>` (case-insensitive,
matched close tag), or SIGNAL at end-of-output / on its own line; optional
`until_bash` (exit 0 = done).

**Template variables:** `$ARGUMENTS`/`$USER_MESSAGE`, `$ARTIFACTS_DIR`,
`$BASE_BRANCH`, `$WORKFLOW_ID`, `$DOCS_DIR`, `$CONTEXT`/`$ISSUE_CONTEXT`,
`$LOOP_PREV_OUTPUT`, `$LOOP_USER_INPUT`, `$REJECTION_REASON`, `$1`–`$9`.
Substitution **fails loudly** on a referenced-but-missing variable.

**Scheduling:** topological layers (Kahn), sequential layers, parallel within a
layer. Sessions thread through sequential single-node layers; parallel layers are
always `fresh`. Durable state in Postgres (run, node outputs, leases) so a runner
pod crash is recoverable by the control plane — reusing majiayu's reducer/worker
machinery.

**Migration path:** ship an `archon-import` command that reads a `.archon`
workflow YAML and emits our format. Field mapping is near-1:1 (we deliberately
keep the same field names where possible), so the author's
`idea-to-pr-with-kimi-coding-and-codex.yaml` ports with minimal edits. Bundled
`command` markdown files (`archon-plan-setup`, `archon-implement-tasks`, …) get
copied into `harness` defaults.

---

## 7. Agent providers

A `Provider` trait mirroring majiayu's adapter pattern (already present for
Claude/Codex/API). Each provider is selected per node: `node.provider ??
workflow.provider ?? config.default`.

### 7.1 Claude — reuse majiayu (CLI + Anthropic API adapters). ✅ exists.
### 7.2 Codex — reuse majiayu (Codex CLI adapter). ✅ exists.
### 7.3 Pi / Kimi-for-Coding — **new adapter, design note.**
Archon invokes Pi as an **in-process JS SDK** (`@mariozechner/pi-coding-agent`).
That SDK is JavaScript-only — we cannot link it from Rust. Two options:

- **(Preferred) Pi CLI subprocess adapter** — invoke the `pi` CLI the same way
  majiayu's Claude/Codex CLI adapters shell out, parsing its streaming output.
  Auth from `~/.pi/agent/auth.json` / env. Fits the runner-pod model (the `pi`
  binary is baked into the runner image). Model ref format `<pi-provider>/<model>`
  (e.g. `kimi-coding/kimi-for-coding`), exactly as in the author's workflow.
- **(Fallback) Node sidecar** — a tiny Bun/Node service in the runner pod that
  wraps the Pi SDK and speaks a small JSON protocol to the Rust process. Only if
  the CLI lacks needed streaming/控制 features.

Decision pending verification of the `pi` CLI's streaming capabilities (Phase 3).

### 7.4 Concurrency: per-provider semaphores (Pi/Minimax has no SDK throttling) —
port Archon's semaphore cap into the adapter config.

---

## 8. Toolchain provisioning (the headline differentiator)

**Problem:** Archon's worktree isolation does nothing about toolchains — you
hand-build a wrapper image with `pnpm`/`cargo`/etc. We make toolchains a
**first-class, UI-managed, per-project spec** applied automatically before a run.

**Model:**
- A **toolchain spec** per project (stored in Postgres, editable from the UI and
  via `.harness/toolchains.toml` in the repo). Declares toolchains + versions,
  e.g. `node@20 + pnpm@9`, `rust@1.88 + cargo`, `uv`, `bun`, `go@1.23`, plus
  arbitrary `setup` commands (e.g. `pnpm install --frozen-lockfile`,
  `cargo fetch --locked`, `pnpm db:generate`).
- A **provisioner** that installs them at runner-pod startup, *before the first
  node*. Implementation uses a version manager (recommend **mise**
  (`jdx/mise`) — single tool, supports node/python/rust/go/bun/pnpm/uv and more)
  invoked from an init step. The author's existing lockfile-detection `bash`
  node (pnpm/cargo/npm/uv/poetry/go) becomes the built-in default `setup` recipe.
- **Warm caches** on a **per-node** `hostPath` dir (cargo registry/git, pnpm
  store, uv cache, mise install dir) — so the second run *on a given node* is
  fast, without pinning Jobs to a node (§5/§12.3). The provisioner points
  `CARGO_HOME`/`PNPM_STORE_DIR`/`UV_CACHE_DIR`/`MISE_DATA_DIR` at the mount.
- **UI affordance:** a "Toolchains" panel per project: pick toolchains+versions
  from a catalog, add custom setup commands, "Test provisioning" button that runs
  the bootstrap in a throwaway pod and streams logs. **No image rebuild required.**

**Why mise over baking everything into the image:** the image stays small and
stable; new toolchains/versions are data (the spec), not a CI/CD image change —
which is exactly the friction the author wants gone.

---

## 9. Sources & triggers

- **Linear (new):** a cron poller in the control plane queries Linear (GraphQL)
  on an interval for issues matching a saved filter (e.g. a label/state/assignee).
  Each match maps to a workflow + args and is enqueued (deduped by Linear issue
  id). Status/comments are **written back** to Linear as the run progresses and
  completes (link to the PR, post the final verdict). Cron schedule configurable
  per project from the UI.
- **AI / manual trigger:** `POST /runs` with `{workflow, args, project}` (from
  the UI or an AI agent via the MCP server). Reuse majiayu's task-submit surface.
- **GitHub (reuse majiayu):** webhook `@harness` mentions + PR-feedback sweeps,
  kept for the implement→PR→review→fix loop output side.

---

## 10. Web UI

Base: majiayu's React + Vite + Tailwind app — but **not** its task view, and
re-skinned to the author's house style (below).

### 10.0 Visual language & styling
Adopt the design system from the author's `home-ops-agent` web app (a Next.js +
**shadcn/ui** app) so ai-harness matches its look. The system is
framework-agnostic (CSS variables + Tailwind + shadcn), so it ports cleanly into
our Vite app:

- **Theme tokens:** copy the OKLCH token set from
  `home-ops-agent/web/src/app/globals.css` — neutral grayscale base, light+dark
  via CSS vars, `--radius: 0.625rem`, and the signature **orange accent**
  (`--accent-orange: oklch(0.6137 0.0737 67.86)` + `-light`/`-foreground`).
- **Fonts:** Geist (sans) + Geist Mono.
- **Components:** shadcn/ui primitives (cards, tooltip, dialog, badge, sidebar),
  thin scrollbars, dark mode default.

### Per-task view IS that task's executed workflow
Each task/run renders the **actual DAG it ran** as the primary view — not a
generic board (the fixed Kanban below is removed). Different tasks run different
workflows, so each task's screen reflects its own graph.

**Replace the fixed Kanban board.** majiayu's `Active.tsx` is a hardcoded board
with a fixed `COLUMNS` set (Pending → Implementing → Planning → Review → Feedback
→ Ready → Blocked) wired to its single `github_issue_pr` pipeline; tasks are
cards that move between those columns. ai-harness runs **arbitrary, per-task
DAGs**, so a fixed column set cannot represent them and is removed.

**Reference implementation:** `home-ops-agent/web/src/components/dashboard/
agent-flow.tsx` already renders a workflow as **React Flow + Dagre auto-layout**
(LR direction, no manual coordinates), with custom circle+icon nodes, an
orange-accent gradient ring for emphasis, animated accent edges for the active
path, labeled branch edges, and shadcn `Tooltip`s. We mirror this, driven by our
`RunReport` instead of a hardcoded pipeline.

**Replace the fixed Kanban board.** majiayu's `Active.tsx` is a hardcoded board
with a fixed `COLUMNS` set (Pending → Implementing → Planning → Review → Feedback
→ Ready → Blocked) wired to its single `github_issue_pr` pipeline; tasks are
cards that move between those columns. ai-harness runs **arbitrary, per-task
DAGs**, so a fixed column set cannot represent them and is removed.

**The per-task view IS that task's executed workflow.** Each task/run renders the
**actual DAG it ran** as the primary view — not a generic board. Different tasks
run different workflows, so each task's screen reflects its own graph.

- **Per-task workflow graph** (React Flow + Dagre, per §10.0): render *that run's*
  DAG with a **live overlay** — nodes color by state
  (pending/running/done/failed/skipped/cancelled), edges follow `depends_on`,
  loops show iteration count. Specifically (author's asks):
  - **Active step** — the currently-running node is emphasized with the
    orange-accent gradient ring + an animated incoming edge (the same treatment
    `agent-flow.tsx` uses for the active path), plus a subtle pulse.
  - **Elapsed time** — the running node shows a live "running 2m14s" badge
    (ticks client-side from the node's `started_at`); finished nodes show their
    final duration.
  - **Hover for details** — hovering a step opens a shadcn `Tooltip`/`HoverCard`
    with that step's detail: status, provider/model, tokens + cost, iteration
    count (for loops), start time + duration, and an output snippet. Click pins
    it open / expands the full streamed output.
  This is both the "nice visual flows" the author wants and the live task status.
  Backed by `RunReport` from `harness-dag` (per-node status/usage/iterations);
  the live timer needs per-node `started_at`/`ended_at` timestamps, which we add
  when persisting `run_nodes` (§11) and stream over WS.
- **Runs list** (replaces the board): a flat, filterable list/table of runs
  (workflow, project, status, timing, cost) — *not* fixed-stage columns. Click a
  run → its workflow graph. Live updates over WS.
- **Workflow editor** (post-MVP): build/edit DAGs visually, export to YAML.
- **Toolchains panel** (§8), **Linear sources panel** (§9).
- **Task overview** (§10.1): a per-run summary inspired by the Factory
  "mission" view — a **time waterfall** of the DAG (per-node start/end as a
  Gantt), and **token + cost breakdowns pivoted two ways: per workflow step and
  per AI model**, plus totals by type (input / output / cache-read / cache-write).

### 10.1 Token & cost accounting
A first-class requirement (the author wants a Factory-style task overview). Mechanics:

- Every agent adapter returns a `Usage { input, output, cache_read, cache_write,
  cost_usd }` per invocation; we capture it on the `run_nodes` row (a node may
  invoke its provider multiple times — e.g. loop iterations — so usage
  accumulates per node and we also keep per-invocation rows for drill-down).
- Because each node row carries `provider` + `model`, the UI rolls the same data
  up **by step** (group by node) and **by model** (group by model), and a run
  total by type. Loops show per-iteration token bars.
- **Fidelity caveat:** only Anthropic reports cache-read/write; Codex and
  (pending the Phase 3 spike) Pi may report only input/output. We store exactly
  what each provider returns and render `n/a` where unavailable — never synthesize
  a number. Totals/per-step/per-model work for all providers regardless.
- Cost is derived from a per-model price table (configurable) × token counts, so
  the same view shows **$ per step and $ per model**; ties into the per-node
  `maxBudgetUsd` / per-run ceiling guardrails (§12.5).

---

## 11. Persistence

Postgres (majiayu already requires it). Extend the schema with: `workflow_runs`,
`run_nodes` (per-node state/output/session + `started_at`/`ended_at` timestamps
for durations + the live "running 2m14s" badge and the §10.1 waterfall + token
usage: `input`, `output`, `cache_read`, `cache_write`, `cost_usd`, plus
`provider`/`model` for rollups),
`node_invocations` (per-invocation usage for loop/multi-call drill-down),
`projects` (+ toolchain spec), `linear_sources` (filter + cron + cursor),
`toolchain_specs`, and a `model_prices` table feeding cost derivation (§10.1).
Reuse majiayu's migration-on-connect mechanism and the durable queue/lease tables
from `harness-workflow`.

**In the target cluster, Postgres is already provided by CloudNativePG** (see
§14) — we provision a CNPG `Cluster` for ai-harness rather than bringing our own
Postgres container. The docker-compose Postgres stays for local dev only.

---

## 12. Open questions / risks

1. **Pi CLI capability** — does the `pi` CLI expose the streaming + tool-control
   the SDK does? Determines §7.3 CLI-vs-sidecar. *Verify early (Phase 3).*
2. **Linear filter semantics** — which exact Linear query (label? state? a saved
   view?) should trigger runs, and what write-back is wanted (status transition vs
   comment only)? *Need author input before Phase 5.*
3. **Cluster specifics** — *mostly resolved by inspecting `home-ops` (see §14).*
   Deploy via Flux; Postgres via CloudNativePG; ingress via Envoy Gateway +
   cert-manager; secrets via SOPS+age. **Storage constraint, resolved:** storage
   is `local-path` (node-local, RWO) with **no RWX/shared volume**, so a single
   warm-cache shared across nodes isn't possible. **Decision:** keep scheduling
   **free** (scheduler picks any node with room) and give each node its own
   `hostPath` warm cache — first run on a node is cold, then warm, no pinning.
   (Rejected: pinning Jobs to one cache node — defeats free scheduling and can
   fill that node; an RWX layer like Longhorn/NFS — extra storage we don't run.)
   *Talos caveat:* writable `hostPath` needs an allowed path (Talos permits
   certain `/var/...` mounts) — confirm the exact path in Phase 6.
4. **Secrets** — use the cluster's **SOPS+age** convention: provider tokens
   (Anthropic/OpenAI/Pi/Linear/GitHub) as SOPS-encrypted `Secret` manifests in
   Git, decrypted by Flux, mounted as env into the control plane and projected
   into runner pods. Redact in the OTLP/log pipeline (majiayu already redacts
   log paths). *Open: confirm one combined Secret vs per-provider.*
5. **Cost guardrails** — port Archon's `maxBudgetUsd` per node + a per-run ceiling.
6. **majiayu license/attribution** — MIT; preserve `LICENSE`/attribution in the
   seeded fork. *(Done in Phase 0.)*
7. **UI/API auth & exposure** — internal Envoy gateway vs external (Cloudflare
   tunnel). *Open: is ai-harness internal-only, or exposed externally?*

---

## 13. Non-goals (for now)

- Telegram/Slack/Discord adapters (Archon has them; not requested).
- Knowledge-base/RAG features (the *other* "Archon OS"; out of scope).
- Replacing GitHub as the PR target.
- Multi-tenant SaaS hardening — this is a single-operator cluster service.

---

## 14. Target cluster (`home-ops`)

The deployment target is the author's `home-ops` repo — a GitOps home cluster.
ai-harness must fit these conventions rather than invent its own:

| Area | What the cluster uses | Implication for ai-harness |
|---|---|---|
| OS / k8s | Talos Linux, k8s 1.35, 3× MS-01 (bare metal) | No cloud provider; bare-metal scheduling; 3 nodes |
| GitOps | **Flux** (`FluxInstance`); apps live in `kubernetes/apps/<namespace>/<app>/{app,cluster}` | Ship ai-harness as a Flux `Kustomization` + `HelmRelease` (or kustomize base) in that layout; **no manual `kubectl apply`**. Renovate keeps the image tag fresh. |
| CNI / ingress | Cilium; **Envoy Gateway** (internal + external gateways), Gateway API | Expose UI/API via an `HTTPRoute` on the **internal** gateway by default |
| TLS / DNS | cert-manager (Cloudflare DNS01), external-dns | TLS + DNS are declarative; just annotate the route |
| Secrets | **SOPS + age** (`.sops.yaml`, `age.key`) | Provider/Linear/GitHub tokens as SOPS-encrypted `Secret`s in Git (§12.4) |
| Database | **CloudNativePG** (Postgres 16 + pgvecto.rs); Valkey cache | Provision a CNPG `Cluster` for ai-harness; don't ship our own Postgres (§11) |
| Storage | **local-path-provisioner** (node-local, RWO) | No shared RWX volume → free scheduling + a per-node `hostPath` warm cache (§12.3) |
| Backup | Volsync → Cloudflare R2 | Back the Postgres PVC the same way |

**Execution model fit:** the Job-per-run model (§5) needs the control plane's
ServiceAccount to create/watch/delete Jobs (a namespaced `Role` + `RoleBinding`,
shipped via Flux). Runner Jobs are scheduled **freely** across the 3 Talos nodes
by the default scheduler (driven by resource `requests`); each node carries its
own `hostPath` warm cache, so placement stays unconstrained while successive runs
on the same node stay warm. With no autoscaler, a Job with no fitting node waits
`Pending` — so concurrency is capped rather than scaled out.

**Likely home:** a new `automation`-style namespace (the cluster already has an
`automation/` namespace) — to confirm with the author in Phase 6.
