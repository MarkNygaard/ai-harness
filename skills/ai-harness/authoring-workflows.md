# Authoring ai-harness workflows over MCP

Reference for the `ai-harness` skill. Read this when the user wants to **build or
edit a workflow** on the cluster (not just trigger an existing one). Connecting
the MCP endpoint is covered in the main `SKILL.md`; the same `mcp__harness__*`
tools are used here.

An **ai-harness workflow** is a DAG of `nodes` joined by `depends_on` edges. The
cluster runs the nodes in topological layers (same layer → parallel) inside an
isolated git worktree. The bundled `idea-to-pr` (plan → implement → review →
gated PR) is the default; this is for building your **own** workflows or variants
— entirely through MCP tools, so you never hand-write or commit YAML and every
change is validated atomically.

Custom workflows are **global**: saved to `.harness/workflows/<name>.yaml` on the
cluster and runnable by every project. The authoring tools take **no `project`
argument** — there is one global set of workflows, not a per-project one. (The
run tools — `run_trigger` etc. — still take a `project`, since a *run* operates
on a specific registered repo.)

## The authoring loop

Build a workflow with structured, node-level calls — each validates the whole
DAG and saves atomically, so a bad change fails loudly instead of corrupting the
file:

| Tool | Use |
|---|---|
| `workflow_catalog()` | Discover the legal node kinds, provider/model hints, available commands, and trigger rules. Start here. |
| `workflow_create({name, description?, provider?, model?})` | New empty workflow. Errors if one by that name exists. |
| `workflow_set_node({name, node})` | Add or replace one node by `id` (JSON spec — see below). Re-validates the whole DAG. |
| `workflow_set_ui({name, ui})` | Set/clear the workflow's `ui:` — a left-nav entry + a report tab (see [`ui:`](#ui--nav-entry--report-tab)). Pass `null` to clear. |
| `workflow_connect({name, from, to})` | Add an edge: `to` now depends on `from`. Catches unknown ids and cycles. |
| `workflow_remove_node({name, id})` | Delete a node and strip it from every dependent's `depends_on`. |
| `workflow_validate({yaml})` | Parse + check candidate YAML without saving. |
| `workflow_get` / `workflow_list` | Read a workflow's YAML / list what's available (bundled + custom). |
| `workflow_save({name, yaml})` | Validate then save raw YAML in one shot (escape hatch if you'd rather assemble YAML yourself). |

**Typical build:** `workflow_catalog` → `workflow_create` → one
`workflow_set_node` per step (each returns the node summaries — your
build→validate→fix loop) → `workflow_connect` for any extra edges. To edit an
existing **custom** workflow, `workflow_get` it then `workflow_set_node` /
`workflow_connect` / `workflow_remove_node`. Bundled defaults like `idea-to-pr`
have no file — to customize one, `workflow_get` its YAML and `workflow_save` it
under a new name.

The `node` spec passed to `workflow_set_node` is JSON whose fields map **1:1** to
the YAML fields below.

## The node model

```yaml
name: my-workflow             # required
description: what it does      # write it for routing — imperative, mention triggers
provider: pi                   # optional workflow-level default (per-node overrides)
model: kimi-code/kimi-for-coding
nodes:
  - id: first
    bash: "echo hi"
  - id: second
    depends_on: [first]
    prompt: "use $first.output"
```

Every node has an `id` and **exactly one body**. Optional fields:

| Field | Meaning |
|---|---|
| `depends_on` | Node ids that must reach a terminal state before this node is considered. |
| `when` | Conditional gate, evaluated **after** `trigger_rule` (see Conditions). |
| `trigger_rule` | How the node reacts to its deps' terminal states (see below). |
| `provider` / `model` | Override the workflow default (AI bodies only: `prompt`/`command`/`loop`). |
| `context` | `shared` (default — inherit the prior sequential node's session) or `fresh` (new session). |
| `timeout` | Milliseconds, for `bash`/`script` bodies. |
| `output_format` | JSON schema shaping an AI body's output so `$node.output.field` / `when:` can read it. |
| `category` | Free-form label for grouping in the UI. |

### Node bodies (pick one)

**`prompt`** — inline AI prompt:
```yaml
- id: classify
  prompt: "Classify this issue as BUG or FEATURE: $ARGUMENTS"
  model: haiku
```

**`bash`** — shell, no AI; stdout captured as the node's output:
```yaml
- id: changed
  bash: "git diff --name-only origin/$BASE_BRANCH...HEAD"
  timeout: 15000
```

**`command`** — resolve a reusable markdown prompt by name (discover names via
`workflow_catalog`):
```yaml
- id: implement
  command: implement-tasks
```

**`script`** — TS/JS via `bun` or Python via `uv`; stdout captured:
```yaml
- id: transform
  script: |
    console.log(JSON.stringify({ ok: true }))
  runtime: bun            # 'bun' or 'uv' — REQUIRED
  deps: ["pandas>=2"]     # uv only: each passed as `uv run --with`
  timeout: 30000
```

**`loop`** — re-run a prompt until it converges:
```yaml
- id: review
  loop:
    prompt: "Review and fix. When clean emit: <promise>REVIEW_CLEAN</promise>"
    until: REVIEW_CLEAN          # signal string; emitting it ends the loop
    max_iterations: 5            # hard cap — size it to the worst case
    fresh_context: true          # optional: each iteration a new session
    until_bash: "cargo test"     # optional: exit 0 also ends the loop
    provider: pi                 # optional loop-body provider/model override
    model: kimi-code/kimi-for-coding
```
The previous iteration's output is `$LOOP_PREV_OUTPUT`. A loop that hits
`max_iterations` **without** converging still counts as success — pair it with a
`bash` gate if you need convergence enforced.

**`cancel`** — terminate the run with a reason (usually `when:`-gated):
```yaml
- id: abort
  depends_on: [classify]
  when: "$classify.output.safe != 'true'"
  cancel: "refusing to proceed: input flagged unsafe"
```

**`approval`** — human gate. **Partially implemented:** the executor records an
approval node as *skipped* ("not yet supported"). Don't rely on it to pause a run
today.

## trigger_rule — run vs. skip

After a node's deps finish, `trigger_rule` decides whether it runs. A dependency
that never ran (skipped) counts as **Skipped**, *not* success.

| Rule | Runs when |
|---|---|
| `all_success` (default) | every dependency succeeded |
| `one_success` | at least one dependency succeeded |
| `none_failed_min_one_success` | no dependency failed **and** at least one succeeded |
| `all_done` | every dependency reached any terminal state (cleanup / "run regardless") |

**The #1 gotcha:** after a `when:`-gated branch, the merge node sees a *skipped*
dependency, so the default `all_success` would skip the merge too. Use
`none_failed_min_one_success`:

```yaml
- id: fix-bug
  depends_on: [classify]
  when: "$classify.output.type == 'BUG'"
  command: fix-bug
- id: plan-feature
  depends_on: [classify]
  when: "$classify.output.type == 'FEATURE'"
  command: plan-feature
- id: ship
  depends_on: [fix-bug, plan-feature]
  trigger_rule: none_failed_min_one_success   # exactly one branch ran
  command: open-pr
```

## Variables

Substituted in `prompt`, `bash`, `script`, `when`, and resolved `command` text:

| Variable | Value |
|---|---|
| `$ARGUMENTS` / `$USER_MESSAGE` | the task input (description) |
| `$ARTIFACTS_DIR` | per-run scratch dir for passing state (esp. to `context: fresh` nodes) |
| `$BASE_BRANCH` | the run's base branch |
| `$WORKFLOW_ID` | unique run id |
| `$1`..`$9` | positional command args |
| `$nodeId.output` | an upstream node's output (`.field[.field…]` for JSON) |

Referencing a **recognized** harness variable that is unset is a hard error
(fail-loud). Unknown `$tokens` (e.g. shell `$HOME`) pass through verbatim, so
bash bodies are safe.

> **Worktree gotcha:** a per-run worktree has no local `main` — only the
> remote-tracking `origin/main`. Always diff against **`origin/$BASE_BRANCH`**
> (`git diff origin/$BASE_BRANCH...HEAD`); a bare `$BASE_BRANCH` fails to resolve
> and silently widens the diff to the whole repo.

### `$node.output` — passing state between nodes

```yaml
- id: plan
  prompt: "write a plan"
- id: build
  depends_on: [plan]
  prompt: "implement this plan:\n\n$plan.output"
```

- `$id.output.field` does best-effort JSON navigation; a missing node, bad JSON,
  or absent field resolves to the **empty string** (lenient — an upstream node
  may have been skipped).
- Every `$id.output` must point to a **declared** node or the workflow fails to
  parse.
- For reliable field access, make the upstream node emit JSON via `output_format`.
- `context: fresh` nodes remember nothing — `$node.output` and `$ARTIFACTS_DIR`
  files are the *only* way state reaches them.

## Conditions (`when:`) and output_format

`when:` gates a node, evaluated after `trigger_rule`. Comparisons are string
equality after resolution; a lone value is truthy unless it resolves to `""`,
`false`, `0`, or `null`. A malformed `when:` fails the node (and is caught at
parse time).

Always pair a `when:` that reads an AI node with an `output_format` on that node
so you read a **stable field**, never free-form prose:

```yaml
- id: classify
  prompt: "Is this a bug or a feature?"
  output_format:
    type: object
    properties: { type: { type: string, enum: [BUG, FEATURE] } }
    required: [type]
- id: fix
  depends_on: [classify]
  when: "$classify.output.type == 'BUG'"
  command: fix-bug
```

`output_format` appends a directive telling the agent to respond with only
conforming JSON; it shapes the output, it isn't hard-validated. Use it on any
node a downstream `when:` reads, and on verdict/gate nodes.

## A worked build (MCP calls)

```jsonc
// 1. See what's legal (node kinds, providers, commands).
mcp__harness__workflow_catalog({})

// 2. New empty workflow.
mcp__harness__workflow_create({
  name: "triage-fix",
  description: "Classify an issue, then fix bugs or plan features, then open a PR.",
  provider: "pi", model: "kimi-code/kimi-for-coding"
})

// 3. One node per step (each call re-validates the DAG).
mcp__harness__workflow_set_node({ name: "triage-fix", node: {
  id: "classify", prompt: "Classify $ARGUMENTS as BUG or FEATURE.", model: "haiku",
  output_format: { type: "object", properties: { type: { type: "string", enum: ["BUG","FEATURE"] } }, required: ["type"] }
}})
mcp__harness__workflow_set_node({ name: "triage-fix", node: {
  id: "fix", depends_on: ["classify"], when: "$classify.output.type == 'BUG'", command: "fix-bug"
}})
mcp__harness__workflow_set_node({ name: "triage-fix", node: {
  id: "plan", depends_on: ["classify"], when: "$classify.output.type == 'FEATURE'", command: "plan-feature"
}})
mcp__harness__workflow_set_node({ name: "triage-fix", node: {
  id: "ship", depends_on: ["fix","plan"], trigger_rule: "none_failed_min_one_success", command: "open-pr"
}})

// 4. (Each set_node already validated + saved. Read it back to confirm.)
mcp__harness__workflow_get({ name: "triage-fix" })
```

Then run it — the **run** tools take a `project` (the workflow is global, but a
run targets a registered repo), see the main `SKILL.md`:
`run_trigger({ project: "ticket0", workflow: "triage-fix", description: "<task>" })`.

## `ui:` — nav entry + report tab

A workflow can opt into two UI surfaces, so a report-style workflow you author
over MCP shows up like the built-ins — no front-end changes. Set it with
`workflow_set_ui` (or include a top-level `ui:` block in `workflow_save` YAML):

```js
mcp__harness__workflow_set_ui({
  name: "security-audit",
  ui: {
    nav:    { label: "Security Audit", icon: "shield" },
    report: { label: "Findings", verdict_node: "deep-review", scored: false }
  }
})
```

| Field | Meaning |
|---|---|
| `nav.label` | Left-nav entry; appears once the workflow has ≥1 run, links to a page listing its runs. |
| `nav.icon` | Icon key: `shield`, `world-search`, `zoom-code`, `search`, `report`, `checklist` (falls back to a default). |
| `report.label` | Tab label on the run detail page (e.g. `Findings`). |
| `report.verdict_node` | Node whose JSON output is the verdict; if omitted the UI scans nodes. Must name a real node. |
| `report.scored` | `true` → show a score + rating; `false` (default) → findings list only. |

The report renders a node's JSON output shaped as
`{ summary?, score?, rating?, findings: [{ title?, severity?, category?, detail?, fix?, location? }] }`
— produce it with `output_format` on the verdict node. Each finding gets
"Build this" (fires `idea-to-pr`), "Create issue" (Linear, when configured),
and "Ignore" — persisted per run. Pass `ui: null` to clear it.

## Good practices

1. **Deterministic work → `bash`/`script`, not a prompt.** "Run the tests" is a
   `bash` node; only the *reaction* to a failure is an AI node.
2. **`output_format` on every node a downstream `when:` reads.**
3. **`none_failed_min_one_success` after conditional branches** (skipped ≠ success).
4. **Cheap models for glue, strong models for substance.** `haiku` to
   classify/route/format; reserve `sonnet`/`opus`/Kimi for code and analysis.
5. **`context: fresh` + artifacts for state.** Design the artifact chain first —
   each node's `$ARTIFACTS_DIR` file is the spec for the next.
6. **Gate side-effecting nodes on a verdict.** A verify node emits `{passed}`
   (`output_format`); the side-effecting node is
   `when: "$verify.output.passed == 'true'"`, with a `cancel` on the negation.
7. **Scope to the change** — diff `origin/$BASE_BRANCH...HEAD`, act only on those
   files, run the narrowest verify chain that covers them.
8. **Write the `description` for routing** — imperative; mention triggers and
   what it does *not* do.

### Anti-patterns

- ❌ Asking AI to run a deterministic check ("run the tests and tell me if they passed").
- ❌ Pattern-matching free-form AI output in `when:` (no `output_format`).
- ❌ A `context: fresh` node whose prompt assumes "the bug we discussed".
- ❌ Bare `$BASE_BRANCH` in a worktree diff.
- ❌ Tiny `max_iterations` on an open-ended loop.
- ❌ Relying on `approval` nodes to pause a run today (not yet wired).

## Guardrails

- Confirm the **project** name first — it must be one the cluster has registered.
- A custom workflow is **global** (every project can run it); name it clearly.
- Every `workflow_set_node` / `workflow_save` validates the DAG; if a call
  returns an error, fix that node before continuing — don't proceed on a failed
  validation.
