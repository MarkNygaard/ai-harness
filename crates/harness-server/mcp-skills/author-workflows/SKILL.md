---
name: author-workflows
description: |
  Write or change an ai-harness workflow DAG through this server's workflow_*
  tools: add and wire nodes, gate a step with `when:`, pass output between
  steps, give a workflow a report tab, and validate before saving. Use when
  someone asks to "write a workflow", "add a step", "gate this step", "why
  does my node skip" or "make the workflow do X". The full YAML reference is
  reference/authoring.md.
---

# Authoring harness workflows

A workflow is a DAG of `nodes` joined by `depends_on`, run in layers. Each
node has exactly one body: `prompt`, `bash`, `command`, `script`, `loop`,
`approval`, `cancel`, `linear` or `route`. Workflows are **global**: one saved
workflow is runnable by every project on this harness, and a saved workflow
with a built-in's name replaces that built-in everywhere.

**Read [reference/authoring.md](reference/authoring.md) before anything
non-trivial.** It is the full reference for every field, variable, condition
and trigger rule, and it is accurate to the engine this server runs. This
file is the short map.

## Before writing anything

1. `workflow_list`: does it exist already? A row with `overrides_bundled:
   true` means a saved workflow stands in front of a built-in.
2. `library_list`: install a published workflow (`library_install`) rather
   than author a near-duplicate.
3. `workflow_catalog`: the node kinds, providers and models, commands,
   trigger rules and MCP servers this harness actually has. Use names from
   it, not from memory.

## Build with the tools, not raw YAML

Every tool validates the whole DAG and saves atomically, so a bad change
fails without breaking the workflow:

- `workflow_create(name, description, provider, model)`
- `workflow_set_node(name, node)` adds or replaces one node by `id`. The node
  spec has the same fields as the YAML (`id`, one body, `depends_on`, `when`,
  `trigger_rule`, `output_format`, `timeout`, `model`, `context`, `mcp`, …).
- `workflow_connect(name, from, to)` adds an edge; `workflow_remove_node`
  removes a node and its edges.
- `workflow_set_ui` adds a left-nav entry and a findings/report tab.
- `workflow_get` returns the YAML; `workflow_validate` checks candidate YAML
  without saving; `workflow_save` saves whole YAML when that is simpler.

Every save keeps the version it replaces: `workflow_versions` and
`workflow_restore_version` undo an edit, and a deleted workflow sits in
`workflow_trash` for 14 days.

## The five mistakes that break workflows

1. **Skipped is not success.** A node after a `when:` branch needs
   `trigger_rule: none_failed_min_one_success` (or `one_success`); the
   default `all_success` skips it whenever a branch was skipped.
2. **`when:` reading prose.** Give the upstream node an `output_format` and
   test a field (`$classify.output.type == 'BUG'`). Never match free text.
3. **Bare `$BASE_BRANCH` in a run.** A run works in its own worktree with no
   local `main`; diff against `origin/$BASE_BRANCH`.
4. **`context: fresh` without a hand-off.** A fresh node remembers nothing.
   Pass state in `$node.output` or files under `$ARTIFACTS_DIR`.
5. **Deterministic work in a prompt.** Tests, builds and checks are `bash`
   nodes; only the reaction to them is an agent step.

Also: give a long agent step a `timeout` (milliseconds). Without one a hung
step runs until somebody cancels the run. `approval` nodes are parsed but not
wired yet: the run records them as skipped, so they do not pause anything.

## Example

```yaml
name: triage-and-fix
description: Classify the task, fix the bug or build the feature, then run the tests.
provider: claude
model: sonnet
nodes:
  - id: classify
    prompt: "Classify this task as BUG or FEATURE: $ARGUMENTS"
    model: haiku
    output_format:
      type: object
      properties: { type: { type: string, enum: [BUG, FEATURE] } }
      required: [type]
  - id: fix
    depends_on: [classify]
    when: "$classify.output.type == 'BUG'"
    prompt: "Reproduce and fix this bug, with a test that fails before the fix: $ARGUMENTS"
  - id: build
    depends_on: [classify]
    when: "$classify.output.type == 'FEATURE'"
    prompt: "Implement this feature, with tests: $ARGUMENTS"
  - id: check
    depends_on: [fix, build]
    trigger_rule: none_failed_min_one_success
    bash: "cargo test --workspace"
    timeout: 1800000
```

Commands (`command:`) are reusable prompt files; `workflow_catalog` lists the
ones this harness has.

## After saving

Try it on a real task with `run_trigger` (pass `real: false` for a dry run
that only echoes), then follow the run with `run_status` and
`run_node_output`. When a step skips or fails unexpectedly, the
`diagnose-runs` skill walks through reading the run.
