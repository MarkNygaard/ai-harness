# Design: split `harness-server` to cut verify time

**Status:** proposed (Stage A ready to implement). **Owner:** build-perf.

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
