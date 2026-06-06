# Design: split `harness-server` to cut verify time

**Status:** **DEFERRED** (2026-06-06) — do **not** start unless the revisit
trigger below fires. **Owner:** build-perf.

## Decision & revisit trigger

The cheap win — the lighter `validate` pre-check (PR #77) — shipped first and
removed the single biggest cost (a full test compile+run dropped from `validate`,
~15–18 min). **Revisit this split only if, *after that deploy*, a typical
`idea-to-pr` run's verify gates (`validate` + `final-verify-loop`) are still
painfully slow** (say > ~25 min combined). If they're acceptable, leave
`harness-server` as-is — the split is a large, risky investment that isn't worth
it just for tidiness.

**Why deferred (a boundary probe, 2026-06-06):** the high-value target
(`task_executor`/`task_runner`, ~22k LOC) is **not a clean leaf** — it's a
central hub:

- imported by **~30 files** across `harness-server`, including **`http/state.rs`
  (`AppState`)** and the **active `workflow_runtime_worker/*`** — so it's shared
  by *both* the legacy and the live runs path (not dead code; can't delete);
- **mutually coupled with `task_db`** (`task_runner::store` → `TaskDb`, and
  `task_db` → `task_runner`), so they must move together;
- `task_executor` takes **`Arc<HarnessServer>`** (the server's own type) in core
  functions — a back-reference that would create a crate cycle.

So a clean extraction needs **decoupling first**, not a `git mv`. It's a
multi-PR, high-risk effort — see the staged approach under "If the trigger
fires" below.

## If the trigger fires — staged approach

Do **not** attempt the move in one PR. Sequence:

1. **Trait boundary (no code moves).** Define the slice of `HarnessServer` /
   `AppState` that `task_executor`/`task_runner` actually need as a trait in
   `harness-core`; switch their signatures from `Arc<HarnessServer>` to the
   trait. This is the real work and lands as its own reviewable PR with zero
   behaviour change.
2. **Extract the engine.** Once the back-reference is gone, move
   `task_runner` + `task_executor` + `task_db` (they move together) into a new
   `harness-task-engine` crate that depends only on `harness-core`. Update the
   ~30 importers.
3. **Then** the smaller isolated modules (`q_value_store`, `complexity_router`,
   `parallel_dispatch`, …) if still worthwhile.

## Also on the build-perf backlog

- **`cargo nextest`** (deferred from #77): drop-in faster test runner, but its
  per-test-process model breaks the `serial_test`-based Postgres-test isolation
  (concurrent `CREATE SCHEMA` → duplicate-key). Needs a `.config/nextest.toml`
  serial test-group covering the DB tests (`harness-core db::tests`,
  `harness-persist`, and any `#[serial_test::serial]` users) before it's safe.
  Smaller, contained — do this *before* the crate-split if perf is still a
  concern.

---

## Original analysis (retained)

## Problem

`harness-server` is **~102k LOC in a single crate** (~60% of the whole
workspace; ~80k of those LOC sit in files carrying `#[test]`/`#[cfg(test)]`).
Because the frequently-edited **active surface** (the runs/MCP/authoring HTTP API)
lives in the *same* crate as large, rarely-touched **legacy/engine** code, **any
edit recompiles the entire crate plus its inline test code** — and the
`idea-to-pr` pipeline pays that full recompile twice per run (`validate`, then
`final-verify-loop`). Measured: those two gates were ~19 min and ~22 min of a
111-min run, almost entirely compile.

Cargo caches *unchanged crates* and compiles crates in parallel. So the lever is
structural: **move the large/stable code into sibling crates** so editing the
active surface recompiles a small crate, not 102k LOC.

This is the durable fix. (Companion quick win: a lighter `validate` pre-check,
shipped separately. `cargo nextest` was attempted but deferred — its
per-test-process model breaks the `serial_test`-based isolation the Postgres
tests rely on, so it needs a `.config/nextest.toml` serial test-group first.)

## Principle

Split along **edit-frequency** and **dependency-leaf** lines:

- Code we edit often (runs API, MCP, authoring, dashboard) → stays in a small
  `harness-server`.
- Code that's large and stable (task engine, legacy task surface, runtime
  worker glue) → moves to sibling crates that recompile rarely and stay cached.

The hard constraint: an extracted crate **must not depend back on
`harness-server`** (no cycles). Where modules currently reach into the server's
`AppState`, the boundary must be expressed via **traits or shared types moved to
`harness-core`**, not a back-reference.

## Staged plan (one stage per PR; CI green throughout)

### Stage A — extract `harness-task-engine` (biggest bang, ~30k LOC)

Move the large, relatively stable task-execution machinery out of
`harness-server`:

- `task_runner/`, `task_executor/`
- `parallel_dispatch`, `complexity_router`
- `quality_trigger`, `periodic_retry`, `periodic_reviewer`
- `review_store`, `q_value_store`, `reconciliation`

These are large and rarely edited during feature/dogfood work, so dropping them
out of the active recompile path is the single biggest win. Their inline tests
move with them → test rebuilds become changed-crate-only and parallel.

**Boundary work (the real effort):** these modules currently couple to
`AppState`. Define the surface they need as a trait (e.g. `TaskEngineCtx`) or
move the shared data types into `harness-core`, so the new crate depends on
`harness-core` only — never on `harness-server`.

### Stage B — extract intake + runtime-worker glue

Move `intake/` (github_issues/feishu) and the `workflow_runtime_*` /
runtime-dispatch glue into their own crate (or fold the runtime glue into
`harness-workflow`'s `runtime`). Same no-cycle rule.

### Result

`harness-server` shrinks to: the axum router, `AppState` wiring, and the
**active runs/MCP/authoring/dashboard** surface — the code we actually edit.
Editing it recompiles a small crate; the engine/legacy crates stay cached.

## Risks & mitigations

- **`AppState` coupling / circular deps** — the main difficulty. Mitigate by
  introducing trait boundaries / moving shared types to `harness-core` *before*
  the move, and doing it one module-group per PR.
- **Test relocation** — inline `#[cfg(test)]` modules move with their code;
  shared test helpers may need to go to a small `*-test-support` module.
- **Churn** — large file moves; keep each PR to one crate extraction so review
  and bisection stay tractable.

## Not in scope

- Removing the legacy `/tasks/*` HTTP surface (a separate deprecation decision).
- Merging `task_executor` into `harness-runner` (a much larger rewrite; the
  split above gives most of the compile-time benefit without it).

## Expected payoff

Every future verify gate recompiles a fraction of today's unit. Combined with
the lighter `validate` pre-check (and `nextest` once its test isolation is
sorted), this should take the dominant bite out of the ~40 min/run currently
spent compiling+testing.
