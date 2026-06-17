# Distributable skills

Agent Skills (Anthropic's open `SKILL.md` format) that teach an AI agent how to
work with ai-harness from **another** project. Each subfolder is one skill.

| Skill | What it does |
|---|---|
| [`ai-harness/`](ai-harness/SKILL.md) | Trigger + monitor ai-harness runs on the cluster over MCP (turn a task into a reviewed PR from any repo). |

> This folder is for **other** repos. It's distinct from
> [`/.claude/skills/`](../.claude/skills/) — the skills this repo's *own* Claude
> Code sessions use (e.g. `harness-workflows` for authoring workflow YAML).

## Install with `npx skills` (recommended)

These live at the repo's top-level `skills/`, so the open
[`skills` CLI](https://github.com/vercel-labs/skills) discovers them by name.
Run it and it asks which agent(s) to install to and whether to install for the
current project or globally:

```bash
npx skills add MarkNygaard/ai-harness --skill ai-harness
```

Name an agent with `-a` to skip the agent prompt (it still asks project vs.
global):

### Claude Code

Claude Code reads **only** `.claude/skills/`:

```bash
npx skills add MarkNygaard/ai-harness --skill ai-harness -a claude-code
```

### Any other agent

Codex, Cursor, OpenCode, Zed, Gemini CLI, GitHub Copilot, and most others read
the shared `.agents/skills/` directory:

```bash
npx skills add MarkNygaard/ai-harness --skill ai-harness -a codex
# …or -a cursor, -a opencode, -a zed, -a gemini-cli, …
```

See everything this repo offers with
`npx skills add MarkNygaard/ai-harness --list`.

## Install by hand

A skill is just a folder with a `SKILL.md`; copy it into `.claude/skills/`
(per-project) or `~/.claude/skills/` (global). From a clone of this repo:

```bash
# Global (every project picks it up):
mkdir -p ~/.claude/skills && cp -r skills/ai-harness ~/.claude/skills/

# Or just this project:
mkdir -p .claude/skills && cp -r /path/to/ai-harness/skills/ai-harness .claude/skills/
```

No clone, one-liner:

```bash
mkdir -p ~/.claude/skills && \
  git clone --depth 1 --filter=blob:none --sparse \
    https://github.com/MarkNygaard/ai-harness /tmp/ai-harness-skills && \
  git -C /tmp/ai-harness-skills sparse-checkout set skills/ai-harness && \
  cp -r /tmp/ai-harness-skills/skills/ai-harness ~/.claude/skills/ && \
  rm -rf /tmp/ai-harness-skills
```

## Notes

- `SKILL.md` is **Anthropic's** format — read by **Claude Code** and the Claude
  API. Codex and other agents don't read `.claude/skills/`; for those, point
  them at the same `SKILL.md` content from their own config (e.g. `AGENTS.md`).
- After installing, the agent sees the skill's name + description always, and
  loads the full instructions when your request matches (e.g. "trigger a harness
  run"). The `ai-harness` skill also needs the harness **MCP endpoint connected**
  in that project (see the root [README](../README.md#skills)) — it also walks
  you through that on first use.
