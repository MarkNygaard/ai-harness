# ai-harness

A Rust-native orchestration layer for AI coding agents. It turns a task — typed
in a UI, sent over MCP, or pulled from a Linear column — into a run of a
user-authored **workflow DAG**, drives coding agents (Claude Code, Codex,
Pi/Kimi, Cursor) through it in an isolated git worktree, and opens a pull
request at the end. The control plane is a single binary backed by Postgres, and
runs anywhere a container does (Kubernetes or plain Docker).

## What it does

- **Workflow DAGs.** Author multi-node pipelines (e.g. explore → plan →
  implement → validate → PR → review loops) in YAML or the visual editor. Nodes
  run different agents/models, with `when:` gating, `$node.output` wiring, and
  loop/until constructs.
- **Bundled workflows.** Several ship in the box, ready to run or fork into a
  project's `.harness/workflows/`: `idea-to-pr` (a task → a reviewed PR),
  `revise-pr` (address review feedback on an open PR), `merge-pr` (resolve
  conflicts + merge a ready PR), `architect` (behavior-preserving codebase
  health sweep), `geo-audit` (audit a site for AI-search readiness),
  `judge-ab` (score an A/B model comparison), and `bc-idea-to-pr` (the
  idea-to-pr flow for Business Central / AL repos — `al compile` build gate).
- **Multiple agents in one pipeline.** Claude Code, Codex, Pi/Kimi (`omp`), and
  Cursor (`cursor-agent`) nodes, each picking its own model.
- **Three ways to trigger a run:**
  - the **web UI**;
  - an **MCP-over-HTTP** endpoint — `run_trigger` / `run_list` / `run_status`
    plus the workflow-authoring tools — so an MCP-connected assistant can author
    *and* fire workflows;
  - **Linear delegation** — the harness registers as a Linear *agent*: delegate an
    issue to it (or @-mention it) and it picks the work up, walks the issue through
    a configurable status map (e.g. In Progress → In Review → Ready for merge),
    and reports progress inside the issue's agent-session thread. A **column
    poller** remains for stage-to-stage pipelines, gated per binding by `live`.
- **Linear as an app, not as a person.** The workspace is connected once from the
  Credentials page through an OAuth install with `actor=app`, so the comments,
  status moves and run links the harness writes are authored by the application
  instead of by whoever's personal API key was pasted. See
  [Connecting Linear](site/content/docs/triggers/linear-connect.mdx).
- **Projects.** Register a git repo; runs operate on an isolated worktree off
  its base branch. Per-project **GitHub credentials** (with global fallback) and
  Linear trigger bindings are managed from the Projects page.
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
  `docker-compose.yml`) and runs the server. See [CLAUDE.md](CLAUDE.md) for the
  build/verify commands.
- **Container:** run the published image
  (`ghcr.io/marknygaard/ai-harness`) with `HARNESS_DATABASE_URL`,
  `HARNESS_SECRET_KEY` (the AES key for stored credentials — keep it stable), an
  optional `HARNESS_API_TOKEN`, and a persistent volume for `HARNESS_DATA_DIR` /
  `HARNESS_PROJECT_ROOT` (project checkouts). Deploy via Kubernetes or
  `docker compose`. The image bundles the agent runtimes (`bun`, `mise`, `omp` +
  the `pi-web-access` plugin) and listens on `:8080`.

## Skills

ai-harness ships a **distributable Agent Skill** (Anthropic's `SKILL.md` format)
so an AI assistant working in *any* repo can drive the harness — trigger a task
into a reviewed PR, and author custom workflows over MCP — without leaving that
project. It lives in [`skills/`](skills/).

Install the `ai-harness` skill with the open
[`skills` CLI](https://github.com/vercel-labs/skills). Run it and it asks which
agent(s) to install to and whether to install for the project or globally:

```bash
npx skills add MarkNygaard/ai-harness --skill ai-harness
```

Name an agent with `-a` to skip the agent prompt:

```bash
# Claude Code → .claude/skills/
npx skills add MarkNygaard/ai-harness --skill ai-harness -a claude-code

# Any other agent (Codex, Cursor, OpenCode, Zed, Gemini CLI, …) → shared .agents/skills/
npx skills add MarkNygaard/ai-harness --skill ai-harness -a codex   # or -a cursor, -a opencode, …
```

Claude Code reads **only** `.claude/skills/` (so it needs `-a claude-code`);
most other agents share `.agents/skills/`.

See [`skills/README.md`](skills/README.md) for manual install and the full list.

### Connecting the MCP endpoint

The skill drives the harness over its **MCP-over-HTTP** endpoint. The dashboard
builds the configuration for you: **Settings -> Editor connection** shows the
endpoint, the MCP key, and a ready-to-paste snippet for Claude Code, Cursor,
VS Code and Claude Desktop.

The key is generated by the server on first start and stored encrypted -- there
is nothing to put in your manifest. For Claude Code:

```bash
claude mcp add --transport http harness https://<your-harness-host>/mcp   --header "Authorization: Bearer <your MCP key>"
```

Or, for an editor that reads a config file:

```json
{
  "mcpServers": {
    "harness": {
      "type": "http",
      "url": "https://<your-harness-host>/mcp",
      "headers": { "Authorization": "Bearer <your MCP key>" }
    }
  }
}
```

> **Keep the key out of a project's `.mcp.json`.** That file is normally
> committed, and this configuration carries a secret. Use the `claude mcp add`
> command above (which writes to your user configuration) or your editor's
> user-level config. Keys are prefixed `hrn_mcp_` so secret scanners can catch
> one that slips into a repo.

**With sign-in enabled, mint a personal access token** on that same page instead
of using the shared key. The snippet then carries your token, so a run triggered
from your editor is attributed to you rather than to the install -- and revoking
one person's access does not disturb anybody else's editor. A minted token is
shown in the clear only while the page is open.

A deployment that still sets `HARNESS_API_TOKEN` can keep using it as the bearer
token instead; all three are accepted.

## Documentation

- **[CLAUDE.md](CLAUDE.md)** — the canonical guide for humans and agents:
  build/verify commands, architecture glossary, server-operation & worktree
  rules, and the PR workflow. **Start here.** (`AGENTS.md` just points here.)
- **User documentation** lives in [`site/content/docs/`](site/content/docs/) and
  is published by the site in [`site/`](site/), in four sections:
  - **Getting started** — [introduction](site/content/docs/index.mdx),
    [quickstart](site/content/docs/quickstart.mdx),
    [concepts](site/content/docs/concepts.mdx).
  - **[Workflows](site/content/docs/workflows/)** —
    [authoring](site/content/docs/workflows/authoring.mdx) (the canonical YAML
    reference) and [the bundled ones](site/content/docs/workflows/bundled.mdx).
  - **[Triggering runs](site/content/docs/triggers/)** — the three routes in,
    the [MCP tool reference](site/content/docs/triggers/mcp.mdx), and Linear
    [connection](site/content/docs/triggers/linear-connect.mdx) and
    [epics](site/content/docs/triggers/linear-epics.mdx).
  - **[Operating](site/content/docs/operating/)** —
    [deploying](site/content/docs/operating/deploy.mdx),
    [configuration](site/content/docs/operating/configuration.mdx),
    [CLI](site/content/docs/operating/cli.mdx) and
    [agents/models](site/content/docs/operating/agents.mdx).
- [docs/reference/](docs/reference/) and [docs/design/](docs/design/) stay
  internal: inherited specs and in-flight design notes, not published.

## Acknowledgements

Originally seeded from [majiayu000/harness](https://github.com/majiayu000/harness)
(MIT); substantial portions of its runtime are still used. See
[LICENSE](LICENSE).

Workflow design draws inspiration from
[Archon](https://github.com/coleam00/archon).
