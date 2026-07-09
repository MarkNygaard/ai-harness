---
name: ai-harness
description: >-
  Trigger and monitor ai-harness runs on the cluster over MCP — turn a task into
  a reviewed PR without leaving this project. Use when the user wants to "send
  this to ai-harness / the harness", "trigger idea-to-pr", "run a GEO audit on
  the cluster", "kick off a harness run", "check a harness run's status", or
  A/B-compare two models on the same task. Also covers **authoring custom
  workflows** over MCP (the workflow_* tools — see authoring-workflows.md) when
  the user wants to "create/edit a harness workflow", "add a node", or "branch a
  workflow". Covers connecting the harness MCP-over-HTTP endpoint and the
  run_trigger / run_trigger_pair / run_status / run_list / workflow_* tools.
---

# Drive ai-harness from this project (over MCP)

**ai-harness** is a self-hosted orchestration cluster for AI coding agents. You
give it a task (title + description) and a registered **project** (a git repo it
has cloned); it runs a **workflow** — by default `idea-to-pr`: plan → implement →
multi-pass review → build gate — in an isolated worktree and opens a **reviewed
pull request**. Other bundled workflows include `geo-audit` (audit a project's
live site for AI-search readiness) and `merge-pr` / `revise-pr`. You interact
with it purely through its MCP tools: trigger a run, then poll its status.

This skill covers two things: **running** the harness (trigger + monitor, below)
and **authoring** your own workflows over MCP. If the user wants to *build or
edit a workflow* rather than run one, read **[authoring-workflows.md](authoring-workflows.md)**
(bundled alongside this file) — the `workflow_*` tools, the node model, `when:` /
`trigger_rule` / `output_format`, and a worked build.

## Step 0 — Is the harness MCP connected?

Check whether `mcp__harness__*` tools are available (e.g. `mcp__harness__run_list`).

- **Available** → skip to Discover.
- **Not available** → the MCP server isn't wired into this project. Add it to the
  project's `.mcp.json` (MCP-over-HTTP — no local binary):

  ```json
  {
    "mcpServers": {
      "harness": {
        "type": "http",
        "url": "https://<your-harness-cluster>/mcp",
        "headers": { "Authorization": "Bearer <your-token>" }
      }
    }
  }
  ```

  The **URL and token are personal** (they grant control of your cluster). Keep
  `.mcp.json` **out of version control** — add it to `.gitignore`. Ask the user
  for the cluster URL + token if you don't have them; do not invent them. After
  it's added, the user reloads so the `mcp__harness__*` tools appear.

## Discover what's runnable

- `mcp__harness__run_list({ limit })` — recent runs (most recent first). Use it
  to see which **projects** exist and how prior runs look (there's no separate
  "list projects" call — read project names off recent runs, or ask the user).
- `mcp__harness__workflow_list({})` — the workflows you can run (bundled
  defaults + global custom workflows). Workflows are global, so this takes no
  project.

Confirm the exact **project name** before triggering — it must be a project the
cluster has registered, not a local folder name.

## Trigger a run

`mcp__harness__run_trigger` — starts a run and returns a `run_id`.

- Required: **`project`**, **`description`** (the task spec — what to build/do;
  fed to the workflow as `$ARGUMENTS`).
- Optional: `workflow` (empty = the project's default, usually `idea-to-pr`),
  `title` (human label), `base_branch` (empty = project default), `real`
  (default `true`; `false` = echo/dry-run for a wiring check).

Examples:

```jsonc
// Fix/build something → a PR via the default workflow.
mcp__harness__run_trigger({
  project: "ticket0",
  title: "Add rate limiting to the public API",
  description: "Add a token-bucket rate limiter (60 req/min/IP) to the public REST API, with tests and a config flag. Return 429 with a Retry-After header."
})

// Audit a project's live site (workflow reads the project's external URL).
mcp__harness__run_trigger({
  project: "ticket0",
  workflow: "geo-audit",
  title: "GEO Audit · ticket0",
  description: "Audit the live site for GEO / AI-search readiness."
})
```

A run is **asynchronous** — `run_trigger` returns immediately with the `run_id`;
the work happens on the cluster.

## Monitor + report

- `mcp__harness__run_status({ run_id })` — one run's status + per-node detail.
  Poll until the status is terminal (`completed` / `failed` / `cancelled`).
- Report the outcome to the user: the PR URL (from the finalize/summary node
  output) and the verdict, or the failure and which node failed.
- `mcp__harness__run_findings({ run_id })` — for a workflow with a report tab,
  the per-finding marks a human set: each `finding_key` + `action` (`built` /
  `issued` / `ignored` / `checked` / `passed` / `failed`). Use it to read back,
  e.g., which manual test scenarios a person passed vs failed.

Don't fabricate a `run_id` or a result — only report what `run_status` returns.

## A/B compare two models (optional)

`mcp__harness__run_trigger_pair` runs the **same task twice**, differing only in
which model the chosen steps use:

- `swap_from` `{provider, model}` — the steps under test (their current model).
- `variant_a` — arm A's model (set `variant_a == swap_from` to make A the
  baseline).
- `variant_b` — arm B's model (the challenger).
- plus `project` + `description` (+ optional `workflow`, `title`).

Returns a `pair_id` and both `run_id`s; compare them in the harness dashboard.

## Guardrails

- The harness **opens a PR** — treat it like any agent-authored PR: review before
  merge.
- Trigger only against a **registered project**; confirm the name first.
- The MCP endpoint controls a real cluster — never commit the URL/token.
