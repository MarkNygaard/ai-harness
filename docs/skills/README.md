# Distributable skills

Agent Skills (Anthropic's open `SKILL.md` format) that teach an AI agent how to
work with ai-harness from **another** project. Each subfolder is one skill.

| Skill | What it does |
|---|---|
| [`harness-mcp/`](harness-mcp/SKILL.md) | Trigger + monitor ai-harness runs on the cluster over MCP (turn a task into a reviewed PR from any repo). |

> These are for **other** repos, and are distinct from the repo's two in-tree
> skill systems:
> - [`/skills/`](../../skills/) — the harness's **built-in** agent skills
>   (`build-fix`, `review`, `gc`, …), compiled into the `harness-skills` crate
>   via `include_str!` and used by the harness's own runs. Harness format
>   (`# name` + `<!-- trigger-patterns -->`), **not** `SKILL.md`. Don't put
>   distributables there.
> - [`/.claude/skills/`](../../.claude/skills/) — skills this repo's *own* Claude
>   Code sessions use (e.g. `harness-workflows` for authoring workflow YAML).

## Installing a skill in another project

A skill is just a folder with a `SKILL.md`. Put it in one of two places:

- **Per-project** — `<that-repo>/.claude/skills/<name>/` — shared with anyone who
  clones that repo.
- **Global (all your projects)** — `~/.claude/skills/<name>/` — available in every
  project on your machine.

### Copy it in

From a clone of this repo:

```bash
# Global (every project picks it up):
mkdir -p ~/.claude/skills && cp -r docs/skills/harness-mcp ~/.claude/skills/

# Or just this project:
mkdir -p .claude/skills && cp -r /path/to/ai-harness/docs/skills/harness-mcp .claude/skills/
```

### One-liner (no clone)

```bash
mkdir -p ~/.claude/skills && \
  git clone --depth 1 --filter=blob:none --sparse \
    https://github.com/MarkNygaard/ai-harness /tmp/ai-harness-skills && \
  git -C /tmp/ai-harness-skills sparse-checkout set docs/skills/harness-mcp && \
  cp -r /tmp/ai-harness-skills/docs/skills/harness-mcp ~/.claude/skills/ && \
  rm -rf /tmp/ai-harness-skills
```

### Pointing an agent at it

In another project you can also just tell the agent: *"Install the `harness-mcp`
skill from the ai-harness repo (github.com/MarkNygaard/ai-harness, under
`docs/skills/`) into `.claude/skills/`."* It will copy the folder into place.

## Notes

- `SKILL.md` is **Anthropic's** format — read by **Claude Code** and the Claude
  API. Codex and other agents don't read `.claude/skills/`; for those, point
  them at the same `SKILL.md` content from their own config (e.g. `AGENTS.md`).
- After installing, the agent sees the skill's name + description always, and
  loads the full instructions when your request matches (e.g. "trigger a harness
  run"). The `harness-mcp` skill also needs the harness **MCP endpoint connected**
  in that project — it walks you through that on first use.
