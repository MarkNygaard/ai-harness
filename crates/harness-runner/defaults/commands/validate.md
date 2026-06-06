---
description: Run the project's QUICK pre-check (format/lint/build — not the full test suite), fix what's fixable, emit a pass/fail verdict
argument-hint: (no arguments — reads the verify chain from the project's CLAUDE.md / AGENTS.md)
---

# Validate Implementation (quick pre-check)

**Workflow ID**: $WORKFLOW_ID

This is an **early sanity gate** before the run invests in opening a PR and
running review. Run the project's **quick pre-check** — format check, lint, and
build/compile — fix what you can, and report a machine-readable verdict the
workflow uses to decide whether to open a PR.

**Do NOT run the full test suite here.** It's expensive, and the run has a
dedicated **final verify gate** at the end (after review fixes land) that runs
the complete test suite. Running it now — before review — just doubles the cost.

This step is **language-agnostic**. Do NOT assume Node/npm, Rust, or any
particular toolchain — discover the project's actual verify chain.

---

## Phase 1 — Find the verify chain

The authoritative commands live in the project's `CLAUDE.md` / `AGENTS.md`
(auto-loaded into your context) — look for a "Build & verify" / "Validation
commands" section. Many projects define **two tiers** (a quick pre-check vs. a
full gate); run the **quick pre-check** tier here — format check, lint, and
build/compile — and leave the full test suite for the final gate.

If the project docs don't specify, infer the quick pre-check from the repo
(format + lint + build/compile; **skip the test run** here):
- `Cargo.toml` → `cargo fmt --check`, `cargo clippy --workspace --all-targets`,
  `cargo build --workspace`. (Do not run `cargo test` — that's the final gate.)
- `package.json` → `tsc`/`type-check`, `lint`, `format:check`, and `build` via
  the detected package manager (`bun.lockb`→bun, `pnpm-lock.yaml`→pnpm,
  `yarn.lock`→yarn, else npm). Skip the `test` script here.
- `pyproject.toml` / `uv.lock` → `ruff` (lint/format), type check — skip `pytest`.
- Mixed repos → run each ecosystem's quick pre-check for the parts the PR touched.

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
{ "passed": true,  "summary": "fmt+clippy+build green; fixed 2 type errors" }
```

or, if any check is still red after your fixes:

```json
{ "passed": false, "summary": "cargo build: type error in harness-runner (token parse)" }
```

Rules for the verdict:
- `passed` is `true` **only if the quick pre-check is green** — format check,
  lint, and build/compile all exit clean. If you could not run a critical check
  at all (e.g. the build never compiled), that is `false`, not `true`. (The full
  test suite is the *final gate's* job, not this verdict's.)
- `summary` is one line: what passed, what you fixed, or what is still red.
- Be honest. A false `true` ships a broken PR; the workflow trusts this field.
