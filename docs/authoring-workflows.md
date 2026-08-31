# Authoring workflows

How to write an ai-harness workflow DAG. This is the canonical reference for the
YAML format, the variable/condition machinery, and the good-practices that keep
a workflow correct once it meets a real repo. It is accurate to the current
engine (`crates/harness-dag`) — where a feature is partial, it says so.

Bundled workflows live in `crates/harness-runner/defaults/workflows/*.yaml`;
custom workflows are **global** — they live in the server's
`.harness/workflows/*.yaml` (authored via the editor/MCP) and a custom file
shadows a bundled default of the same name. There is no per-project workflow
storage. Validate any candidate with `harness_dag::parse_workflow` (the visual
editor and MCP authoring server both go through `harness-runner`'s
`validate_workflow`).

## Two ways to author

**Prefer the MCP tools** (the `harness mcp-server` exposes them) — build a
workflow with structured, node-level calls instead of hand-writing YAML. Each
call validates and saves atomically, so a bad change fails without corrupting
the file:

| Tool | Use |
|---|---|
| `workflow_catalog` | discover legal node kinds, providers/models, commands, trigger rules |
| `workflow_create` | new empty workflow (`name`, optional `description`/`provider`/`model`) |
| `workflow_set_node` | add/replace a node by id from a JSON spec (`{id, <body>, depends_on, when, category, …}`) |
| `workflow_connect` | add a dependency edge (`to` depends on `from`) |
| `workflow_remove_node` | delete a node + strip it from dependents |
| `workflow_get` / `workflow_list` / `workflow_validate` | read / list / check |

A typical build: `workflow_catalog` → `workflow_create` → a `workflow_set_node`
per step (each returns the resulting node summaries — the build→validate→fix
loop) → `workflow_connect` for any extra edges. You never write YAML, and every
step is checked.

**Authoring (and running) against the hosted cluster.** There are two transports
to the same cluster. Workflows are **global**, so the `workflow_*` authoring
tools take no `project` argument; only the **run** tools take a `project` (a run
operates on a *registered cluster project*).

**(a) Hosted HTTP endpoint — no local binary (preferred).** The server exposes a
single `POST /mcp` (JSON-RPC over HTTPS). Point your client straight at it; the
bearer token is the cluster API token. This transport also exposes **run
control** — `run_trigger`, `run_status`, `run_list` — so you can kick off and
monitor a run (e.g. `idea-to-pr`) from the editor, not just author:

```json
{
  "mcpServers": {
    "harness-cluster": {
      "type": "http",
      "url": "https://harness.mnygaard.io/mcp",
      "headers": { "Authorization": "Bearer ${HARNESS_TOKEN}" }
    }
  }
}
```

**(b) Local stdio proxy.** Run `harness mcp-server` locally and set
`HARNESS_REMOTE_URL` (+ `HARNESS_TOKEN`) in the client's `.mcp.json` `env`; the
binary proxies the `workflow_*` tools to the cluster's global
`/api/authoring/*` API. (Authoring only — no run control.)

```json
{
  "mcpServers": {
    "harness-cluster": {
      "command": "harness",
      "args": ["mcp-server"],
      "env": {
        "HARNESS_REMOTE_URL": "https://harness.mnygaard.io",
        "HARNESS_TOKEN": "<your token>"
      }
    }
  }
}
```

**The rest of this doc is the format + design reference** — the node fields a
`workflow_set_node` JSON spec accepts map 1:1 to the YAML below, and the
good-practices / `trigger_rule` / `when:` / `output_format` rules apply
identically whichever way you author. (Raw YAML via `workflow_save` / editing
the file directly is still supported — it's the same model.)

## Workflow shape

```yaml
name: my-workflow            # required
description: what it does     # optional — written for routing (see below)
provider: pi                  # optional workflow-level default (per-node overrides)
model: kimi-code/kimi-for-coding
ui:                           # optional — opt into UI surfaces (see below)
  nav: { label: "Security Audit", icon: shield }
  report: { label: "Findings", verdict_node: deep-review, scored: false }
nodes:
  - id: first
    bash: "echo hi"
  - id: second
    depends_on: [first]
    prompt: "use $first.output"
```

A workflow is a list of **nodes** connected by `depends_on` edges. The scheduler
computes topological layers; nodes in the same layer run in parallel.

### `ui:` — nav entry + report tab

A workflow can opt into two UI surfaces. Both are optional; omit `ui:` entirely
for a plain workflow.

| Field | Meaning |
|---|---|
| `nav.label` | Left-nav entry label. The entry appears once the workflow has ≥1 run and links to a page listing its runs. |
| `nav.icon` | Icon key from the curated allow-list (`shield`, `world-search`, `zoom-code`, `search`, `report`, `checklist`); falls back to a default. |
| `report.label` | Tab label on the run detail page (e.g. `Findings`, `Review`). |
| `report.verdict_node` | Node whose JSON output is the **verdict**; if omitted the UI scans nodes for one matching the shape. Must reference a real node. |
| `report.scored` | `true` → show a score + rating + per-dimension bars + score-history sparkline (GEO-style); `false` (default) → findings list only. |
| `report.actions` | Opt-in per-finding buttons; **default `[]` → a clean, read-only list**. Any of `build` (fire idea-to-pr), `issue` (file into Linear), `ignore`. A bug/finding report uses `[build, issue, ignore]`. |
| `report.status` | Per-item status control: `none` (default), `check` (a "tested" checkbox), or `pass_fail` (Passed / Failed) — for manual test checklists. |

The report renders a node's JSON output shaped as
`{ summary?, score?, rating?, categories?[], findings: [{ title?, severity?, category?, detail?, fix?, location?, effort? }] }`
— produce it with `output_format` on the verdict node. This is how any
workflow (including MCP-authored ones) gets a report tab + nav entry without
front-end changes.

The per-item marks a user sets (`build`/`issue`/`ignore` or `checked`/`passed`/
`failed`) persist per run and are readable back over MCP via `run_findings`
(each finding's `finding_key` + `action`) — so a downstream agent can see, e.g.,
which test scenarios a human passed vs failed.

## Node fields

Every node has an `id` and exactly **one body** (`prompt` / `bash` / `command` /
`script` / `loop` / `approval` / `cancel`). Optional fields:

| Field | Meaning |
|---|---|
| `depends_on` | Node ids that must reach a terminal state before this one is considered. |
| `when` | Conditional gate evaluated after `trigger_rule` (see [Conditions](#conditions)). |
| `trigger_rule` | How this node reacts to its deps' terminal states (see below). |
| `provider` / `model` | Override the workflow default (AI bodies only — `prompt`/`command`/`loop`). |
| `context` | `shared` (default, inherit the prior sequential node's session) or `fresh` (new session). |
| `timeout` | Milliseconds, for `bash`/`script` bodies. |
| `retries` | Re-run **this node only** on failure, up to N times, before the run fails (default `0` = run once). Same worktree/session — for transient failures (tests, builds, flaky network). Do **not** set on side-effecting steps (e.g. a push/PR step), where a retry could double-act. Capped at 10. |
| `output_format` | JSON schema the AI body's output should match (see [output_format](#output_format)). |

### Node bodies

**`prompt`** — inline AI prompt:
```yaml
- id: classify
  prompt: "Classify this issue: $ARGUMENTS"
  model: haiku
```

**`bash`** — shell, no AI, stdout captured as the node's output:
```yaml
- id: changed
  bash: "git diff --name-only origin/$BASE_BRANCH...HEAD"
  timeout: 15000
```

**`command`** — resolve a markdown prompt from `.harness/commands/<name>.md`
(project) or the bundled defaults; YAML frontmatter is stripped:
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
    prompt: "Review and fix. When clean: <promise>REVIEW_CLEAN</promise>"
    until: REVIEW_CLEAN          # signal string; emitting it ends the loop
    max_iterations: 5            # hard cap — size it to the worst case
    fresh_context: true          # optional: each iteration a new session
    until_bash: "cargo test"     # optional: exit 0 also ends the loop
    provider: pi                 # optional: loop-body provider/model override
    model: kimi-code/kimi-for-coding
```
The previous iteration's output is available as `$LOOP_PREV_OUTPUT`.

**`cancel`** — terminate the run with a reason (usually `when:`-gated):
```yaml
- id: abort
  depends_on: [classify]
  when: "$classify.output.safe != 'true'"
  cancel: "refusing to proceed: input flagged unsafe"
```

**`approval`** — human gate. **Partially implemented:** the executor currently
records an approval node as *skipped* with a "not yet supported" note (no input
channel yet). Don't rely on it to actually pause a run.


## Caching inside a workflow

`$HARNESS_CACHE_DIR` is a directory on the server's persistent volume, private to
the project, that survives the run. Everything else a run touches does not: the
worktree is thrown away, and `$ARTIFACTS_DIR` with it.

Package-manager downloads are already cached by the harness itself (pnpm store,
NuGet packages, Go modules, …) — you do not need to do anything for those. Use
this for what only you know is reusable: BC symbol packages keyed by `app.json`,
a downloaded SDK, a fixture that costs more to rebuild than to keep.

Three rules, learned from the BC symbol cache (`bc-idea-to-pr`'s `setup-bc`):

1. **Key on the inputs, in the top-level directory name.** The harness bounds
   this cache by evicting whole immediate subdirectories, so
   `alpackages-<hash>/` lets one stale entry go without taking the live one;
   `alpackages/<hash>/` loses every entry at once.
2. **Build in a temp dir and `mv` it into place, with a marker file written
   inside before the move.** A half-written cache is worse than none — it looks
   like a hit and fails much later, somewhere unrelated. Check the marker, not
   the directory.
3. **Expire it if the source can change without the key changing.** The BC cache
   keys on `app.json`, but the endpoint serves whatever the *server* has, so it
   also refreshes after 7 days. A cache with no expiry and an incomplete key is
   a wrong answer that never corrects itself.

Absent (an older server), `$HARNESS_CACHE_DIR` is simply unset — write the step
so it still works, uncached.
## trigger_rule

After a node's deps finish, `trigger_rule` decides run vs skip. A dependency
that never ran (skipped/upstream-skipped) counts as `Skipped` — **not** success.

| Rule | Runs when |
|---|---|
| `all_success` (default) | every dependency succeeded |
| `one_success` | at least one dependency succeeded |
| `none_failed_min_one_success` | no dependency failed **and** at least one succeeded |
| `all_done` | every dependency reached any terminal state (use for cleanup / "run regardless") |

**The classic gotcha:** after a `when:`-gated branch, the merge node sees a
*skipped* dependency, so the default `all_success` would skip the merge. Use
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

Substituted in `prompt`, `bash`, `script`, `when`, and resolved-`command` text.

| Variable | Value |
|---|---|
| `$ARGUMENTS` / `$USER_MESSAGE` | the task input |
| `$ARTIFACTS_DIR` | per-run scratch dir for passing state between nodes |
| `$BASE_BRANCH` | the run's base branch |
| `$WORKFLOW_ID` | unique run id |
| `$1`..`$9` | positional command args |
| `$nodeId.output` | an upstream node's output (see below) |

Referencing a **recognized harness variable** that is unset is a hard error
(fail-loud). Unknown `$tokens` (e.g. shell `$HOME`, `${arr[@]}`) pass through
verbatim, so bash bodies are safe.

> **Worktree gotcha (learned the hard way):** in a per-run worktree there is no
> local `main` branch — only the remote-tracking `origin/main`. Always diff
> against **`origin/$BASE_BRANCH`**, never bare `$BASE_BRANCH`
> (`git diff origin/$BASE_BRANCH...HEAD`). A bare ref fails to resolve and your
> scoping silently falls back to the whole repo.

### `$node.output` — passing state between nodes

A downstream node reads an upstream node's output with `$id.output`, and a JSON
field of it with `$id.output.field[.field…]`:

```yaml
- id: plan
  prompt: "write a plan"
- id: build
  depends_on: [plan]
  prompt: "implement this plan:\n\n$plan.output"
```

- Deep paths do best-effort JSON navigation (objects by key, arrays by index).
- A missing node, unparseable JSON, or absent field resolves to the **empty
  string** — lenient by design, because an upstream node may legitimately have
  been skipped.
- Every `$id.output` reference must point to a **declared** node, or the
  workflow fails to parse (`UnknownNodeReference`).
- For reliable field access, make the upstream node emit JSON — see
  [output_format](#output_format).
- `context: fresh` nodes have no memory of prior nodes; `$node.output` (and
  `$ARTIFACTS_DIR` files) are the *only* way state reaches them.

## Conditions

`when:` gates whether a node runs, evaluated **after** `trigger_rule`. Grammar:

```
expr  := or
or    := and ('||' and)*
and   := cmp ('&&' cmp)*
cmp   := value (('=='|'!=') value)?
value := $node.output[.field…] | $HARNESS_VAR | 'literal' | "literal" | bareword
```

- Comparisons are string equality after resolution. A lone `value` is **truthy**
  unless it resolves to `""`, `false`, `0`, or `null`.
- A `false` `when` skips the node; a **malformed** `when` fails it (and is also
  caught at parse time as `InvalidCondition`).
- Make the upstream node `output_format` a stable shape so field access is
  reliable — don't pattern-match free-form prose:

```yaml
# GOOD
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

## output_format

An optional JSON schema on an AI node. The runner appends a directive telling
the agent to respond with **only** conforming JSON, so the node's output is a
stable shape that `$node.output.field` and `when:` can read. The schema is not
hard-validated against the reply — it shapes the agent, it doesn't reject it.
Use it on any node whose output a downstream `when:` reads, and on verdict nodes
(see the `validate` → `finalize-pr` gate in `idea-to-pr.yaml`).

## Per-node hooks

An optional `hooks:` block on any node lets you intercept tool calls **before**
they run (`pre_tool_use`) or inject non-blocking context **after** they run
(`post_tool_use`). The hook config is provider-agnostic; the runner translates it
per provider at dispatch.

```yaml
- id: plan
  provider: claude
  prompt: "Investigate and propose a plan. Do not edit files."
  hooks:
    pre_tool_use:
      - matcher: "Write|Edit"
        decision: deny
        reason: "This node is read-only; edits are out of plan."
- id: implement
  depends_on: [plan]
  provider: pi
  prompt: "Implement the plan."
  hooks:
    post_tool_use:
      - matcher: "Write|Edit"
        additional_context: "Run `cargo check`. If this isn't actually simpler, justify or revert."
```

### Hook rule shape

| Field | Meaning |
|---|---|
| `matcher` | Regex over the tool name (e.g. `Write\|Edit`, `Bash`). Absent/empty = match every tool. |
| `decision` | `deny` blocks the call (pre_tool_use only). `allow`/`ask` or omitted = observe/inject. |
| `reason` | Text surfaced to the agent when a call is denied. |
| `additional_context` | Non-blocking text injected into the agent's context. |
| `system_message` | Non-blocking message surfaced to the user/agent. |

### Provider mappings

- **`claude` nodes** — hooks become Claude Code `settings.json` hooks delivered
  via `--settings`. A `deny` rule produces a `PreToolUse` hook that returns
  `permissionDecision: deny`; a `post_tool_use` rule produces a `PostToolUse`
  hook that injects `additionalContext`.
- **`pi` / `omp` nodes** — hooks become the `harness-hooks` omp extension loaded
  via `--plugin-dir`. The serialized config is passed in the `HARNESS_HOOKS`
  environment variable. `deny` rules block via `{ block: true, reason }`;
  `post_tool_use` rules emit `sendMessage(..., { deliverAs: "steer" })`.
- **`codex` nodes** — codex has no hook mechanism; hooks are silently ignored.
- **`bash`/`script`/`approval`/`cancel` nodes** — parsed and stored but never
  delivered (no agent to intercept).

## Good practices

1. **Deterministic work → `bash`/`script`, not a prompt.** "Run the tests" is a
   `bash` node; only the *reaction* to failures is an AI node. AI nodes are
   expensive, non-reproducible, and can hallucinate a passing result.
2. **`output_format` on every node a downstream `when:` reads.** Otherwise you're
   string-matching free-form AI text.
3. **`none_failed_min_one_success` after conditional branches.** Skipped ≠ success.
4. **Cheap models for glue, strong models for substance.** `model: haiku` for
   classify/route/format; reserve `sonnet`/`opus`/Kimi for code and analysis.
5. **`context: fresh` + artifacts for state.** A fresh node must read
   `$ARTIFACTS_DIR/...` or `$upstream.output`; it remembers nothing on its own.
   Design the artifact chain before writing command bodies — each node's
   artifact is the spec for the next.
6. **Gate side-effecting nodes on a verdict.** Don't open a PR / deploy on an
   unverified branch: have a verify node emit `{passed}` (`output_format`) and
   gate the side-effecting node `when: "$verify.output.passed == 'true'"`, with a
   `cancel` node on the negation to fail loud.
7. **Scope to the change.** Diff against `origin/$BASE_BRANCH...HEAD` and act only
   on those files; run the narrowest verify chain that covers them (the full
   chain only as the final gate).
8. **Write the `description` for routing** — imperative, mention triggers and
   what it does *not* do.
9. **Validate before shipping** a new workflow: `parse_workflow` checks id
   uniqueness, one-body-per-node, dependency + `$node.output` references resolve,
   and `when:` syntax. Cycles are caught by `topological_layers`.

### Anti-patterns

- ❌ Asking AI to run a deterministic check (`prompt: "run the tests and tell me
  if they passed"`).
- ❌ Pattern-matching free-form AI output in `when:` (no `output_format`).
- ❌ A `context: fresh` node whose prompt assumes "the bug we discussed".
- ❌ Bare `$BASE_BRANCH` in a worktree diff (see the worktree gotcha).
- ❌ Tiny `max_iterations` on an open-ended loop — size it to the worst case.
- ❌ Relying on `approval` nodes to pause a run today (not yet wired).
- ❌ Naming a concrete build chain in a prompt (`cd web && bunx vitest run`,
  `cargo nextest run --workspace`). A workflow runs against whatever project
  triggers it; the harness already auto-loads each project's `CLAUDE.md` /
  `AGENTS.md`, so tell the agent to read the chain from there. Branching on
  paths (`starts with web/`, `under crates/`) has the same problem — the next
  project's layout won't match, and the agent is left improvising.
