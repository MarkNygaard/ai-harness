---
name: diagnose-runs
description: |
  Find out what happened in an ai-harness workflow run and why: which step
  failed, was skipped or was cancelled, what it printed, and what to change.
  Use when someone gives you a run id (e.g. `task-1794194165697`), asks why a
  run failed, stalled or "did nothing", why a Linear issue was not updated, or
  what the agents keep tripping over across runs. Uses this server's run_*
  tools; read-only.
---

# Diagnosing a harness run

A run is one execution of a workflow DAG. Its id is `<workflow>-<unix ms>`, so
`security-audit-1795651221487` is a `security-audit` run started at
1795651221487 ms (2026-11-26 00:00:21 UTC). Read-only tools are enough for
everything below. Never start, cancel or rerun anything while diagnosing unless
the person asks.

## 1. Find the run

Call `run_status` with the id.

- **"not found"** usually means the run belongs to a different harness. Every
  harness keeps its own runs; if more than one harness server is connected,
  try each before concluding the run does not exist. `run_list` shows what this
  one holds.
- No id? `run_list` (newest first) and match on `title`, `project`,
  `workflow_name` or the time.

## 2. Read the shape before the output

`run_status` returns the run's `status`, its `graph` (each node's `depends_on`,
`when`, `artifact`) and a row per node with `status`, `note`, `started_at` /
`ended_at`, `iterations`, `error_count`, `provider` and `model`. Work out:

- **Where it stopped.** The first node, in order, that is `failed` or
  `cancelled`. Everything after it is usually a consequence, not a cause.
- **Skipped nodes.** A `note` of "`when` condition evaluated false" is the
  engine working as designed. Find the upstream output the condition reads
  (`$layout.output.run_backend == 'true'`) in that node's `output` and check
  the value. A skip only matters when the value is wrong.
- **Timing.** A step that ran for hours, or for a second, is evidence. Compare
  it with the step's `timeout` in `workflow_get`; a step with no `timeout` can
  run until the run is cancelled.
- **`error_count` on a successful node** counts failed tool calls the agent
  worked around. It is not a failure, but it is a reason to read the step.

## 3. Read the step that matters

`run_node_output` with the run id and the node id returns that step's output
tail (`tail_lines`, default 200; `0` for all) and `total_lines`.
`include_artifact: true` adds the file the step wrote (a plan, a report).
Read one step at a time; `run_status` already holds every output and is a lot
to read for one question.

What the common endings mean:

| What you see | What it means |
|---|---|
| `failed` with output | Read the end of the output; the error is usually in the last lines. |
| `failed`, `bash` node | The script exited non-zero; its stderr is in the output. |
| `cancelled`, no output, `iterations: 0` | The agent never returned a result. The run was cancelled from outside the step: by a person, by the server reaping a run whose lease went stale (a restart or a lost pod), or after the step hung. The run record does not say which; the server log at `ended_at` does. |
| `cancelled` by a `cancel` node | The workflow refused on purpose; the cancel node's text says why. |
| `skipped`, other note | Its dependencies did not satisfy its `trigger_rule` (an upstream step failed or was skipped). |

## 4. Look wider when one run is not the story

- `run_activity_errors` (optionally `project`, `days`) groups identical agent
  failures across runs. A failure with a high `runs` count is a property of
  the project (a missing generated file, an absent credential, a command that
  is not where the agent looked). That belongs in the project's CLAUDE.md,
  not in a rerun.
- `run_findings` shows which report items a person marked (issued, ignored,
  passed, failed).
- `issue_prs` lists every run for a Linear issue and the pull request each
  opened, with live CI state; `pr_status` and `pr_diff` read one PR.
- `run_linear_claim` explains a Linear issue or agent session that is not
  being updated: no row means the run was never linked; a null
  `agent_session_id` means there is no session to post into.
- `workflow_get` shows the workflow as it is now. It may have changed since
  the run.

## 5. Report

Lead with the answer: what happened, in which step, and when. Then the
evidence (the status, the note, the last lines that matter), and keep what
you observed separate from what you suspect. End with the smallest next step:
a rerun with different input, a `timeout` on a step, a rule for the project's
CLAUDE.md, or the server log line to look for. Quote ids, node ids and times
exactly so the person can find them.
