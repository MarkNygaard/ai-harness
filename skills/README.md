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
[`skills` CLI](https://github.com/vercel-labs/skills) discovers them by name:

```bash
# Into the current project's .claude/skills/ (Claude Code):
npx skills add MarkNygaard/ai-harness --skill ai-harness -a claude-code

# Globally (every project on your machine), non-interactive:
npx skills add MarkNygaard/ai-harness --skill ai-harness -g -a claude-code -y

# See everything this repo offers:
npx skills add MarkNygaard/ai-harness --list
```

`-a claude-code` targets Claude Code's `.claude/skills/`. **Without `-a`** the
CLI installs to the shared `.agents/skills/` directory — which Codex, Cursor,
Zed, and others read, but **Claude Code does not** (it only reads
`.claude/skills/`), so pass `-a claude-code` for Claude Code. `-g` installs
globally to `~/.claude/skills/`; omit it for the current project only.

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
