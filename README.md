# ai-harness

A Rust-native orchestration layer for AI coding agents. It turns a task — typed
in a UI, sent over MCP, or pulled from a Linear column — into a run of a
user-authored **workflow DAG**, drives coding agents (Claude Code, Codex,
Pi/Kimi) through it in an isolated git worktree, and opens a pull request at the
end. The control plane is a single binary backed by Postgres, and runs anywhere
a container does (Kubernetes or plain Docker).

## What it does

- **Workflow DAGs.** Author multi-node pipelines (e.g. explore → plan →
  implement → validate → PR → review loops) in YAML or the visual editor. Nodes
  run different agents/models, with `when:` gating, `$node.output` wiring, and
  loop/until constructs.
- **Bundled workflows.** `idea-to-pr` (a task → a reviewed pull request) and
  `merge-pr` (resolve conflicts + merge a ready PR) ship in the box, ready to
  run or fork into a project's `.harness/workflows/`.
- **Multiple agents in one pipeline.** Claude Code, Codex, and Pi/Kimi (`omp`)
  nodes, each picking its own model.
- **Three ways to trigger a run:**
  - the **web UI**;
  - an **MCP-over-HTTP** endpoint — `run_trigger` / `run_list` / `run_status`
    plus the workflow-authoring tools — so an MCP-connected assistant can author
    *and* fire workflows;
  - a **Linear poller** — watches a column (plus an optional eligibility label),
    claims one issue at a time, walks it through a configurable status map
    (e.g. In Progress → In Review → Ready for merge), fires the bound workflow,
    and tags the PR so Linear links it back to the issue. A per-binding `live`
    flag gates dry-run vs. acting.
- **Projects.** Register a git repo; runs operate on an isolated worktree off
  its base branch. Per-project **Linear/GitHub API keys** (with global fallback)
  are managed from the Projects page.
- **Toolchain provisioning.** Declare a project's toolchains; `mise` installs
  them on demand (cached on the data volume — no image rebuild).
- **Secrets.** Credentials are encrypted at rest (AES-256-GCM), and
  control-plane secrets are scrubbed from the agent processes a run spawns.

## How it runs

The control plane is a single `harness` binary (HTTP API + the bundled web UI)
backed by Postgres. A run executes as **local child processes** inside the
server's container — there is no per-run Kubernetes Job — so the same image runs
under Kubernetes *or* plain Docker.

- **Local dev:** `./start-server.sh` brings up Postgres (via the dev
  `docker-compose.yml`) and runs the server. See [AGENTS.md](AGENTS.md) for the
  build/verify commands.
- **Container:** run the published image
  (`ghcr.io/marknygaard/ai-harness`) with `HARNESS_DATABASE_URL`,
  `HARNESS_SECRET_KEY` (the AES key for stored credentials — keep it stable), an
  optional `HARNESS_API_TOKEN`, and a persistent volume for `HARNESS_DATA_DIR` /
  `HARNESS_PROJECT_ROOT` (project checkouts). Deploy via Kubernetes or
  `docker compose`. The image bundles the agent runtimes (`bun`, `mise`, `omp` +
  the `pi-web-access` plugin) and listens on `:8080`.

## Skills

ai-harness ships **distributable Agent Skills** (Anthropic's `SKILL.md` format)
so an AI assistant working in *any* repo can drive the harness — turn a task
into a reviewed PR — without leaving that project. They live in
[`skills/`](skills/).

Install the `ai-harness` skill with the open
[`skills` CLI](https://github.com/vercel-labs/skills):

```bash
# Into the current project's .claude/skills/:
npx skills add MarkNygaard/ai-harness --skill ai-harness -a claude-code
# …or globally, for every project on your machine:
npx skills add MarkNygaard/ai-harness --skill ai-harness -g -a claude-code -y
```

`-a claude-code` is what lands it in `.claude/skills/`. Without it the CLI
targets the shared `.agents/skills/` directory — which Codex, Cursor, and others
read, but **Claude Code does not** (it only reads `.claude/skills/`).

See [`skills/README.md`](skills/README.md) for manual install and the full list.

### Connecting the MCP endpoint

The skill drives the harness over its **MCP-over-HTTP** endpoint. Point your
agent at your deployment by adding it to the project's `.mcp.json`:

```json
{
  "mcpServers": {
    "harness": {
      "type": "http",
      "url": "https://<your-harness-host>/mcp"
    }
  }
}
```

If the server runs with `HARNESS_API_TOKEN` set, send it as a bearer header:

```json
{
  "mcpServers": {
    "harness": {
      "type": "http",
      "url": "https://<your-harness-host>/mcp",
      "headers": { "Authorization": "Bearer <HARNESS_API_TOKEN>" }
    }
  }
}
```

## Documentation

- **[AGENTS.md](AGENTS.md)** — the canonical guide for humans and agents:
  build/verify commands, architecture glossary, server-operation & worktree
  rules, and the PR workflow. **Start here.**
- [docs/authoring-workflows.md](docs/authoring-workflows.md) — authoring
  workflow DAGs (node types, `when:` / `$node.output` / `output_format`,
  `trigger_rule`, and good practices).

## Acknowledgements

Originally seeded from [majiayu000/harness](https://github.com/majiayu000/harness)
(MIT); substantial portions of its runtime are still used. See
[LICENSE](LICENSE).
