# Design: Linear-epic orchestrator

**Status:** proposal (no implementation yet) · **Author:** ai-harness · **Reviewers:** _tbd_

A **Linear epic** describes a feature; its **sub-issues** are the individual
pieces. This proposes an orchestrator that decomposes an epic into ordered
sub-issues, builds them one at a time through the harness, has a strong model
(Opus at `max` effort) **supervise** each result against the sub-issue's intent,
and — when a piece isn't built as expected — files a corrective sub-issue and
keeps going, until the whole feature is done.

## Goal / non-goals

**Goal:** one epic in → a sequence of reviewed PRs out, coordinated over
hours/days, with a supervisor gating progression and self-correcting.

**Non-goals (v1):** parallel sub-issues (v1 is strictly sequential, one at a
time); replacing human review (the supervisor *gates*, humans can still review
the PRs); a real-time UI for the epic (it's observable in Linear + the runs list).

## Core insight: the Linear board is the state machine

We do **not** need a new long-lived engine. The existing **Linear poller**
([`linear_poller.rs`](../../crates/harness-server/src/http/linear_poller.rs))
already is a durable, in-production event loop that:

- polls every 30 s (no webhooks needed);
- when an issue enters a **bound column + label**, **claims** it (moves it to an
  in-progress column) and **fires the bound workflow** as a run;
- on run completion (observed by DB polling), **reports back**: transitions the
  issue (→ review / ready), comments, and retries/marks-failed per the binding's
  `max_attempts`;
- **chains workflows across columns** — claims are keyed per `(issue, workflow)`,
  so an issue already flows `idea-to-pr` (on "AI Eligible") → review → `merge-pr`
  (on a "Ready to merge" column) today.

So the epic loop is just **more columns + labels + bindings + two new
workflows**, with Linear as the source of truth. This matches how the user
already thinks (epics / sub-issues / board columns) and avoids duplicating epic
state inside the harness.

### Considered and rejected: the v2 workflow-runtime

The event-sourced runtime (`crates/harness-workflow/src/runtime/`, `repo_backlog`,
`StartChildWorkflow`) *could* host a durable parent that dispatches child runs and
reacts to completions. **Rejected for v1** because it is **not wired into
production** (test-only; nothing spawns its worker loop; child→parent completion
propagation is hard-scoped to one child type), so it would mean activating an
entire dormant tier — far more work — and it would duplicate state Linear already
holds. Revisit only if board-as-state-machine hits real limits (e.g. parallel
sub-issues with complex join semantics).

## Board model

Columns (Linear workflow states) and labels for an epic-enabled team:

| Column / label | Meaning | Bound workflow |
|---|---|---|
| `Epic: plan` (label on the epic) | An epic ready to decompose | **`linear-epic-plan`** (new) |
| `Queued` | A sub-issue not yet started | — |
| `AI Eligible` (label) | The *current* sub-issue to build | `idea-to-pr` (existing) |
| `In review` | idea-to-pr opened a PR | — (poller sets this) |
| `Ready to merge` | PR ready | `merge-pr` (existing) |
| `Built` | PR merged; awaits supervision | **`linear-epic-supervise`** (new) |
| `Done` | Supervisor passed it | — |
| `Blocked / needs human` | Attempt cap hit, or supervisor gave up | — |

Only **one** sub-issue carries "AI Eligible" at a time (v1 sequential). The
orchestrator advances the label; the poller does the rest.

## Lifecycle

**Happy path**

1. Human labels an epic `Epic: plan`. Poller fires `linear-epic-plan`.
2. `linear-epic-plan` reads the epic body, produces an **ordered task list**,
   **creates one sub-issue per task** (parent = the epic) in `Queued`, and moves
   the **first** to `AI Eligible`. Comments the plan on the epic.
3. Poller fires `idea-to-pr` on sub-issue #1 → PR → `In review`.
4. `merge-pr` (on `Ready to merge`) merges → poller moves #1 to `Built`.
5. Poller fires `linear-epic-supervise` on #1 (Opus, `effort: max`): checks out
   the merged code, evaluates it against #1's title/description (acceptance
   criteria). Verdict:
   - **Pass** → move #1 to `Done`; move the **next** `Queued` sub-issue to
     `AI Eligible` (loop to step 3).
   - **Fail** → move #1 to `Done` (its PR merged) **and create a corrective
     sub-issue** under the epic describing the gap, placed **before** the next
     task, marked `AI Eligible`.
6. When no `Queued`/eligible sub-issues remain, the epic is complete → label the
   epic `Done`, comment a summary (links to every sub-issue + PR).

**Failure handling** — each sub-issue inherits the binding's `max_attempts`
retry/`failed_label` behavior (already in the poller). A sub-issue that exhausts
attempts goes to `Blocked / needs human` and the epic **pauses** (no further
advancement) so a human can step in.

## New capabilities to build

Everything above is reuse **except** these four pieces:

### 1. Linear client: create sub-issues + read children
- `create_issue` ([`linear.rs:463`](../../crates/harness-sources/src/linear.rs#L463))
  gains an optional `parent_id` (`issueCreate(input:{ …, parentId })`).
- A new read for an epic's sub-issues + their state/label/order (the current
  client only reads `id/title/labels`; no `parent`/`children`/`relations`). The
  supervisor needs this to find "the next `Queued` sub-issue" and to know the
  epic's children when summarizing.

### 2. Let a workflow write to Linear (the main new capability)
Today only the poller performs Linear mutations (in Rust). The two new workflows
must **create sub-issues and move labels**. Options:

- **(A, recommended) A harness-internal Linear command surface.** Bundled
  `command`/bash nodes (or a small authenticated internal endpoint the run can
  call) that invoke the harness's `LinearClient` using the **stored Linear
  credential** for the project. Pros: uses existing creds, works headless, one
  trusted path, testable. Cons: new command/endpoint surface.
- **(B) The agent uses the user's Linear MCP connector** (`mcp__linear__*`).
  Pros: no new harness code. Cons: **may be absent in headless/cron runs**
  (interactively-authenticated connectors aren't guaranteed in poller-triggered
  runs), and it's the user's personal connector, not the project's service creds.

→ Go with **(A)**. Scope the command surface tightly: `create_sub_issue`,
`set_label`, `move_state` — enough for the two workflows, nothing more.

### 3. `linear-epic-plan` workflow (decompose)
A single-run DAG: read epic → plan (Opus, `effort: max`, `output_format` an
ordered task list with acceptance criteria per task) → a `command` step that
creates the sub-issues (via #2) and marks #1 eligible. Reuses the existing
plan-quality prompt patterns.

### 4. `linear-epic-supervise` workflow (the brain)
Fires when a sub-issue reaches `Built`. Opus `effort: max`. Checks out the
merged result, evaluates against the sub-issue's acceptance criteria (produce a
structured verdict — reuse the `report`/verdict machinery so the outcome is
inspectable via a report tab + `run_findings`). Then a `command` step that
advances the next sub-issue **or** creates a corrective sub-issue. The verdict is
the gate.

## Key decisions (for reviewers)

1. **"Merged" vs "PR opened".** The poller's "ready/completed" means the PR
   *opened*, not merged. The supervisor should review **merged** code, so the
   flow routes through `merge-pr` first (as drawn). Alternative: supervise the
   open PR branch and merge *after* a pass — changes ordering. **Recommend:
   merge then supervise** (simpler, and the merged main is the real artifact);
   the corrective-sub-issue mechanism handles anything the supervisor later finds.
2. **Ordering / dependencies between sub-issues.** v1: strict sequential order
   (the plan's order). Later: allow the plan to declare a dependency graph and
   run independent sub-issues in parallel (needs concurrency > 1 + join logic).
3. **Concurrency.** v1: one sub-issue at a time per epic (`max_concurrent_runs:
   1` on the binding). Keeps the supervisor's "did the previous piece land
   correctly" gate meaningful.
4. **Cost.** Opus `max` supervising every sub-issue is expensive. Mitigations:
   scope the supervisor to the sub-issue's diff, cap corrective rounds per
   sub-issue, and make the supervisor model/effort configurable on the binding.
5. **Human gates.** Optional: a `Needs approval` column before the epic starts
   dispatching, and the existing `Blocked / needs human` stop. The epic should
   never silently loop forever — a per-epic corrective-round cap.
6. **Epic-done detection.** "No `Queued`/eligible sub-issues remain" requires
   the read-children capability (#1). Until then the epic can't reliably know
   it's finished.

## Reused vs new

| Piece | Status |
|---|---|
| Poll → claim → run → report-back → retry | ✅ exists (poller) |
| Cross-column workflow chaining per `(issue, workflow)` | ✅ exists |
| `idea-to-pr`, `merge-pr` | ✅ exist |
| Structured verdict / report tab / `run_findings` | ✅ exists (recent work) |
| `create_issue` with `parent_id` | ⚒ new (small) |
| Read an epic's sub-issues / relations | ⚒ new (small) |
| Workflow → Linear write surface | ⚒ new (**main plumbing**) |
| `linear-epic-plan` workflow | ⚒ new |
| `linear-epic-supervise` workflow | ⚒ new |
| Board columns/labels + 3 bindings (plan / supervise / existing build+merge) | ⚒ config |

## Phasing

- **Phase 1 — decomposition + kickoff.** Linear client `parent_id` + read-children;
  the workflow→Linear write surface; `linear-epic-plan`. Result: label an epic →
  it fans out ordered sub-issues and the existing poller builds #1. (No supervisor
  yet; advancement is manual.)
- **Phase 2 — the supervisor loop.** `linear-epic-supervise` + its binding:
  review merged code, advance the next sub-issue or file a corrective one. This
  closes the loop.
- **Phase 3 — polish.** Epic-done detection + summary, per-epic corrective-round
  cap, configurable supervisor model/effort, optional human approval gate.
- **Later.** Parallel sub-issues (dependency graph + concurrency), and — only if
  needed — migrating the coordinator onto the v2 runtime.

## Open questions

- Which Linear team(s)/columns will host this? (bindings are per project+workflow)
- Merge-then-supervise vs supervise-then-merge (decision #1)?
- Corrective-round cap per sub-issue and per epic?
- Should the supervisor's structured verdict auto-create the corrective sub-issue,
  or propose it for one-click human confirmation first?
