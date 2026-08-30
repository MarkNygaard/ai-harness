# Linear epics

An **epic** is a Linear issue with sub-issues. Delegate it to the harness and it
builds the sub-issues one at a time, each on a shared branch, each reviewed
against its own acceptance criteria before the next begins. The default branch
sees the feature once, finished, as a single pull request.

This page is about **setting that up**. For writing the workflows themselves see
[authoring-workflows.md](authoring-workflows.md); for connecting a Linear
account see [linear-connect.md](linear-connect.md).

## When an epic is the wrong shape

Most work is not an epic. `idea-to-pr` already explores, plans and decomposes
inside one run — its plan step routinely produces ten numbered tasks with exact
file paths, executed by a cheap model. Splitting the same work across sub-issues
multiplies the cost rather than the quality: **each piece is a full pipeline**
(explore, plan, implement, simplify, three review passes, verify) **plus a
supervisor review**.

An epic earns that when one of two things is true:

1. **The work exceeds one run's coherence** — explore, plan, implement and review
   share a single context, and past some size the plan stops being specific
   enough to execute cheaply.
2. **Later pieces depend on earlier ones being *right*, not merely done** — the
   supervisor grades each piece with fresh context before the next starts, where
   a single run's review sees the finished diff and a wrong early step has
   already propagated.

The test: *if you cannot say what would break if two pieces were built in the
other order, it does not need to be an epic.*

Independent work is a poor fit even when it is large. Several documentation
pages, or unrelated fixes across a repo, gain nothing — the branch exists so
piece N+1 starts from a tree containing 1..N, and independent work pays that
sequential cost for no benefit. File those as separate issues and let them run
in parallel.

Good epic: a frontend where component B imports component A. Poor epic: a docs
set, or three unrelated bug fixes.

## The relay

Nothing in the harness "runs an epic". Bindings hand an issue from one column to
the next, and the epic is what falls out of them being wired in a ring:

```
Todo             --> idea-to-pr         builds the piece, opens a PR
   |  Ready (epic piece)
Ready for merge  --> merge-pr           merges it into epic/<EPIC>
   |  Ready
Done             --> linear-epic-supervise
                                        reviews it, releases it,
                                        moves the next piece to Todo
   |
Todo             --> the next piece
```

Three couplings have to line up, and **nothing enforces them**. A binding whose
ready status no other binding claims from means the run succeeds, the move
succeeds, and the work is parked forever with no error anywhere. Use
`linear_check` (below) rather than discovering it by an epic stopping overnight.

## What each part does

**`idea-to-pr`** builds one piece. When the piece belongs to an epic, the poller
bases the run on `epic/<EPIC>` instead of the binding's base branch, so piece
N+1 starts from a tree that already contains 1..N.

**`merge-pr`** merges the piece's PR into the epic branch. **The supervisor does
not merge** — a common misreading. It reviews what has already landed.

**`linear-epic-supervise`** does three different jobs depending on what it is
handed:

| handed | it does |
|---|---|
| an epic (sub-issues, no parent) | creates `epic/<EPIC>`, starts the first piece |
| a merged piece | grades it; advances, or files a corrective |
| the last piece, passing | opens `epic/<EPIC>` → default branch as one PR |

It **releases** each piece when it is finished with it — clears the delegate —
because a merged piece rests in the column the supervisor triggers from, and
nothing else would stop the poller offering it again on the next tick.

Nothing merges the final epic PR. It is the whole feature, and the one review
that was never a piece.

## Setting it up

**1. Three bindings on the team.** On the project's Linear dialog:

| claims from | workflow | Ready | Ready (epic piece) |
|---|---|---|---|
| Todo | `idea-to-pr` | wherever standalone issues should stop | Ready for merge |
| Ready for merge | `merge-pr` | Done | — |
| Done | `linear-epic-supervise` | — | — |

**`Ready (epic piece)` is the field that makes epics and ordinary work
coexist.** A team that will not merge without a human review sets `Ready` to
something like *Functional testing*, so standalone issues stop there. A piece of
an epic is not finished work heading for the default branch — it is heading for
the epic's own branch, where the supervisor grades it and the whole feature is
reviewed once. Leave it empty and both behave the same, which is how every
binding behaved before the field existed.

**2. Nothing else.** The column a piece starts in is derived from the claims the
poller already records. `EPIC_READY_STATE` remains as a project env var for a
board that wants pieces somewhere else, but it is an override, not setup.

Optional, all project env vars: `EPIC_BLOCKED_STATE` (where a stalled epic
goes), `EPIC_REVIEW_STATE` (where the finished epic goes when its PR opens),
`EPIC_CORRECTIVE_LABEL`.

**3. Check it.** Over MCP: `linear_check({ project })`. It walks the relay and
reports what breaks, in column names rather than state ids.

## Writing the epic

**Create sub-issues in reverse build order.** Linear gives newer issues the
*lower* `sortOrder`, and the supervisor builds in ascending order — so create the
last piece first and the board reads correctly without dragging anything.

**Delegate every sub-issue and the epic**, then put the epic in the build
column. The harness cannot delegate an issue to itself, so a piece nobody
delegated is never picked up no matter which column it reaches.

**Give each piece its own acceptance criteria.** The supervisor grades a piece
against the criteria in its own description and nothing else — not against what
it would have built, and not against work the criteria assign to a later piece.
Vague criteria produce a pass on work that does not hold up; criteria that
overlap the next piece produce a corrective for something not yet due.

## Bounds

Deliberate, and not configurable:

- **2 corrective rounds per piece.** Three failures is a specification problem,
  and a fourth attempt spends money to prove it again.
- **15 correctives per epic**, as a runaway backstop.

On either limit the epic stops and says so on the epic's own comments, which are
its only memory — there is no epic state stored anywhere. The board is the
ledger: the pieces, their columns, and a `Corrects: AIH-12` line in each
corrective's body, which is what counts rounds.

## When an epic stops

Run `linear_check` first; most stalls are a column nothing polls. Otherwise:

- **A piece sits delegated in a column with no binding** — that column is the
  end of the relay. Check `Ready` and `Ready (epic piece)`.
- **A piece sits undelegated** — the supervisor released it (finished) or it was
  never delegated. Delegate it to restart.
- **The epic sits in the build column and nothing happens** — it needs
  sub-issues; an issue without them is built, not supervised.
