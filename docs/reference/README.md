# Reference docs (from majiayu-harness)

These are design specs and notes **inherited from
[majiayu000/harness](https://github.com/majiayu000/harness)** (MIT), retained
because they document subsystems `ai-harness` reuses — the workflow runtime,
storage layer, agent/codex adapters, multi-project model, backlog polling, and
project cache.

They describe majiayu's design as of the seed copy and are **not** authoritative
for `ai-harness`. The authoritative plan lives in [../PLAN.md](../PLAN.md) and
[../PHASES.md](../PHASES.md). Treat anything here as background that may have
drifted; verify against the current code before relying on it.

majiayu's project-history material (postmortems, GC reports, audits, issue
write-ups, bug/todo notes, validation evidence) was removed during the Phase 0
seed cleanup — it pertained to that project's history, not ours.
