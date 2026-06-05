---
description: Run the project's full verify chain, fix what's fixable, emit a pass/fail verdict
argument-hint: (no arguments — reads the verify chain from the project's CLAUDE.md / AGENTS.md)
---

# Validate Implementation

**Workflow ID**: $WORKFLOW_ID

Run the project's complete verification suite, fix what you can, and report a
machine-readable verdict the workflow uses to decide whether to open a PR.

This step is **language-agnostic**. Do NOT assume Node/npm, Rust, or any
particular toolchain — discover the project's actual verify chain.

---

## Phase 1 — Find the verify chain

The authoritative commands live in the project's `CLAUDE.md` / `AGENTS.md`
(auto-loaded into your context) — look for a "Build & verify" / "Validation
commands" section, and honor any scoping guidance it gives (many projects want
a scoped verify for small changes and the full chain only as a final gate;
since this is the pre-PR gate, prefer the **complete** chain it describes).

If the project docs don't specify, infer from the repo:
- `Cargo.toml` → `cargo fmt --check`, `cargo clippy`, `cargo test` (workspace or
  the affected crates).
- `package.json` → the `scripts` that exist (`tsc`/`type-check`, `lint`,
  `format:check`, `test`, `build`) via the detected package manager
  (`bun.lockb`→bun, `pnpm-lock.yaml`→pnpm, `yarn.lock`→yarn, else npm).
- `pyproject.toml` / `uv.lock` → `ruff`, `pytest`, etc.
- Mixed repos → run each ecosystem's chain for the parts the PR touched.

Read `$ARTIFACTS_DIR/plan-context.md` if present — the plan may name the exact
validation commands.

---

## Phase 2 — Run it, fix what's fixable

Run each check. On failure:
1. Read the error.
2. If it's a clear, in-scope fix (type error, lint, format, a broken test you
   understand, a compile error), fix the root cause and re-run.
3. Auto-fixers are fine for lint/format (`cargo fmt`, `lint:fix`, `ruff --fix`).
4. Never disable a check, never use `--no-verify`, never skip hooks, never run
   DB migrations to make a check pass. Honor project-specific prohibitions in
   `CLAUDE.md` / `AGENTS.md`.

Repeat until the chain is green or you hit something you cannot safely fix.

---

## Phase 3 — Write the validation artifact

Write `$ARTIFACTS_DIR/validation.md` with: the commands you ran, each result
(✅/❌), and anything you fixed or that remains blocked. This is the detailed
record for the human; keep it factual.

---

## Phase 4 — Verdict (this becomes the node's output)

Your **final message** is consumed by the workflow to gate PR creation, so it
must be the verdict and nothing else. Emit a single JSON object:

```json
{ "passed": true,  "summary": "fmt+clippy+test green; fixed 2 type errors" }
```

or, if any check is still red after your fixes:

```json
{ "passed": false, "summary": "cargo test: 1 failing in harness-runner (token parse)" }
```

Rules for the verdict:
- `passed` is `true` **only if the entire verify chain is green** — every
  command you ran exits clean. If you could not run a critical check at all
  (e.g. the build never compiled), that is `false`, not `true`.
- `summary` is one line: what passed, what you fixed, or what is still red.
- Be honest. A false `true` ships a broken PR; the workflow trusts this field.
