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

**Multi-repo workspaces**: if `$HARNESS_REPOS` is set, this is a container with
each repo in its own folder (see the JSON for `folder`/`role`). Run the quick
pre-check **inside each repo the change touched** (`cd "$folder"`), using that
repo's own verify chain — each repo may be a different stack. The verdict
`passed` is `true` only if **every** touched repo passes; the `summary` should
name the repo that failed, if any. Single-repo (unset) → run at the root as
below.

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
- `*.sln` / `*.csproj` → `dotnet build`; for the format check use
  `dotnet format <sln|csproj> --verify-no-changes` **scoped to the files the PR
  touched** (e.g. `--include <changed files>`), never repo-wide. Skip
  `dotnet test` here.
- Mixed repos → run each ecosystem's quick pre-check for the parts the PR touched.

**Scope the format/lint check to the change.** A whole-repo
`--verify-no-changes` (e.g. `dotnet format` across the solution, `prettier
--check .`, `eslint .`) will trip on **pre-existing** formatting, line-ending
(LF/CRLF), or style drift in files this PR never touched — that is **not** a
failure of the change and must not fail the verdict. Restrict format and lint
checks to the files the change touched — get them with
`git diff --name-only origin/$BASE_BRANCH...HEAD` and pass that set to the tool
(`--include`/path args). Build/compile stays whole-project (a change can break
compilation elsewhere). **Never reformat unrelated files** to make a repo-wide
check pass.

The plan may name the exact validation commands: read the plan artifact this run
actually wrote — `plan.md`, or `plan-context.md` when a `plan-setup` step ran.
List `$ARTIFACTS_DIR` first and read only what it shows; a read of an artifact a
shorter pipeline never wrote lands in the run's error feed for nothing.

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
- Pre-existing format / lint / line-ending drift in files the change did **not**
  touch does **not** make the verdict `false` — scope the check to the diff (see
  Phase 1) and judge only the change's own files. When the touched files are
  clean and the build is green, `passed` is `true`; mention the unrelated drift
  in `summary` if you want, but it does not gate the PR.
- `summary` is one line: what passed, what you fixed, or what is still red.
- Be honest. A false `true` ships a broken PR; the workflow trusts this field.
