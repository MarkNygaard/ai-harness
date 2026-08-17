# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **Default Claude models moved to the 5 series.** `claude-sonnet-4-6` → `claude-sonnet-5`
  and `claude-opus-4-6` → `claude-opus-5`, across the Claude Code CLI default, the
  Anthropic API adapter, and the reasoning-budget tiers (`xhigh` → Opus, `high` →
  Sonnet). Only pinned ids changed: the bundled workflows use the `sonnet`/`opus`
  aliases, which the CLI resolves to the current model on its own, and the cost table
  matches on family substrings, so per-token rates were already right (Sonnet 5 is
  $3/$15 like 4.6; Opus 5 is $5/$25 like 4.6).
- **The Anthropic API adapter's default `max_tokens` is 16000, up from 4096.** That
  budget caps thinking *and* response text together, and on the 5 series adaptive
  thinking is on whenever a request omits a `thinking` field — which this adapter
  does. At 4096 it would have spent much of the budget reasoning and truncated the
  answer. 16000 is the recommended ceiling for a non-streaming request.

### Added

- **Runs report progress into a Linear agent session.** A poller-claimed run now opens
  a session of its own (`agentSessionCreateOnIssue`), so retries report into a thread like a
  delegated run instead of leaving detached comments.
- **The Linear session says which step is running, not only which finished.** An
  `action` when a step starts as well as when it ends, so a thread whose last line was
  "Finished explore" no longer reads as stalled for the ten-plus minutes `create-plan`
  takes. Every node of a parallel layer is announced, each exactly once. The heartbeat
  is now what it was meant to be — a fallback for when a single long step is in flight
  — rather than the only way to learn what is happening.
- **Step activities name the step, its position, and the workflow.** They rendered as
  "Finished explore idea-to-pr": three names in a row, since Linear concatenates an
  action activity's `action` and `parameter`. Now "Finished the validate step (6 of 15)
  of workflow idea-to-pr", so a reader can see how far through the run is. The counter
  is the workflow's authored numbering (what the graph view shows), not execution order,
  and the total counts declared steps including any a `when:` will skip. Each step also
  carries one timing figure, on the line where it means something: the clock time it
  started — "(6 of 15, started 13:14 CEST)" — and, when it ends, how long it took —
  "(6 of 15, took 1m 46s)", coarsening to `1h 4m` as it grows. A failure reports how
  long it ran before failing. Times are Danish (`Europe/Copenhagen`, so summer time is
  handled and the label reads `CET` or `CEST` accordingly), overridable with
  `HARNESS_DISPLAY_TZ`. A failed or cancelled step no longer claims to have finished, and the
  poller's start activity drops the redundant issue identifier (the session is already
  on the issue) to match the delegated wording.
- **The Linear keep-alive heartbeat fires after 20 minutes of silence, not 10.** Now
  that starts and finishes are both reported, the only silence left is a single step
  in flight, so the old threshold fired for every ordinary long step — four times in
  one 16-step run, none needed, its longest step being 18m 7s. Each one also cost
  thread structure: Linear bundles consecutive `action` activities into one
  collapsible "Used N tools" group and a `thought` closes the bundle, so every
  heartbeat split the step list into another group. Twenty minutes still leaves 10
  minutes of slack under Linear's 30-minute stale window.
- **Delegated runs report progress into the Linear session.** An `action` as each
  workflow step finishes, plus the keep-alive above. Previously two activities were
  posted at the start and nothing until the run ended, so Linear marked the session
  stale after its 30-minute limit and showed "stopped responding" for most of a long
  run while the harness worked normally.

### Fixed

- **A poller-claimed run can actually open its Linear agent session.** It called
  `agentSessionCreate`, which the schema marks `[Internal] … on behalf of the current
  user`; Linear answers a third-party app with `Access denied`. Every claimed run
  therefore fell back to plain comments and showed no threaded progress — the very
  thing the session was added for. Switched to the public `agentSessionCreateOnIssue`,
  which infers the app from the token rather than naming an `appUserId` (that explicit
  naming was the privilege being withheld). Session creation now also requires the app
  (OAuth) connection up front instead of spending a round-trip to be refused: a
  personal API key would open a session belonging to the human who minted it.
- **The bundled workflows' verify steps no longer assume ai-harness's own layout.**
  `idea-to-pr`'s `final-verify-loop` branched on paths starting with `web/` or under
  `crates/`, falling back to "run the Rust+web full gate". A pnpm monorepo whose
  paths are `apps/web/…` matched no branch, so the run's final gate was told to run
  `cargo` in a repo with no `Cargo.toml` and had to reverse-engineer the real chain
  before it could verify anything. Worse, the one instruction that did the right
  thing — "use that repo's own verify chain from its `CLAUDE.md`" — was gated on the
  change spanning *more than one* repo, so a single-repo run was routed to the
  hardcoded steps. Both nodes now read the chain from the project's `CLAUDE.md` /
  `AGENTS.md` like every other node, and per-repo handling applies to every entry in
  `.pr-list`. `architect`'s `fix-failures` had the same hardcoding and is fixed too.
- **The final gate no longer re-runs the tier `validate` already proved.** A new
  `record-verified-head` node stamps the commit `validate` certified; when nothing
  has been pushed since, the gate runs only the test suite that `validate`
  deliberately skips, instead of repeating format/lint/build on an identical tree.
- **Delegation respects a binding's "Max simultaneous tasks".** The cap was enforced
  only by the poller, so delegating three issues to a binding set to 1 started three
  runs at once. A delegation that arrives while the binding is full now gets a
  `response` (not an `error` — nothing failed) naming what is running, and the issue
  is deliberately left in its source status: a `live` binding's poller starts it
  automatically once a slot frees, so nothing has to be re-delegated.
- **A disabled Linear binding no longer wins delegation.** `enabled` governed only
  the column poller, so unchecking it did not stop work arriving by delegation — and
  where two bindings shared a source status, the disabled one could shadow the one
  meant to run (it sorts first). `enabled` now means "active" for both routes;
  `live` stays poller-only (claim vs. dry-run), so a delegation-only setup is
  `enabled` on, `live` off.
- Linear's own thread-opening comment ("This thread is for an agent session with
  …") is no longer fed to the agent as reviewer feedback. It has no emoji prefix, so
  the existing bot-comment filter missed it.
- The note appended after downloading images read "what they shows" — four separate
  singular/plural interpolations with one wrong. Rewritten as two whole sentences.

### Added

- **Linear image attachments reach the agent.** Screenshots pasted into an issue or
  its comments are downloaded with the workspace credential and handed to the agent
  as files, with the task text rewritten to local paths — previously the private
  `uploads.linear.app` URL was forwarded as text and was unfetchable, so the image
  was effectively lost. Files land outside every worktree so they can't be committed
  into a PR; only `uploads.linear.app` is ever fetched (issue text is
  user-authored); `png`/`jpeg`/`gif`/`webp` only; max 5 per task and 25MB each.
  Nothing is resized or re-encoded — the agent's tooling downscales for the model —
  and any failure leaves the URL in place rather than failing the run. Downloaded
  files are swept hourly once untouched for a week
  (`HARNESS_ATTACHMENTS_TTL_HOURS`), including on startup so a crash leaves nothing
  behind.
- **Linear OAuth (`actor=app`)** — the workspace is connected from the
  Credentials page instead of by pasting a personal API key, so the comments,
  status transitions and run links the harness writes are attributed to the
  application rather than to the person who owned the key. Tokens are refreshed
  automatically (~24h lifetime, 5-minute skew, single-flight) and revoked on
  disconnect. A global `api_key` still works as a fallback; an OAuth token takes
  precedence. See [docs/linear-connect.md](docs/linear-connect.md).
- **Delegation as the Linear trigger** — the harness is now a Linear *agent*:
  delegate an issue to the app (or @-mention it) and Linear opens an agent
  session, delivered to a new signature-verified `POST /api/linear/webhook`. The
  harness acknowledges within Linear's 10-second window, resolves the issue's team
  to a project binding, fires the workflow, and reports progress into the session
  thread as agent activities (`thought` → `action` → `response`/`error`) instead of
  as detached comments. Adds the `app:assignable` + `app:mentionable` scopes.
- **Both triggers apply the same two gates**: the issue must be *delegated to the
  harness* **and** in the binding's *source status*. The webhook refuses (with an
  explanation in the session) when a delegated issue isn't in the configured
  status, and the poller filters on `IssueFilter.delegate` — so it now claims only
  what a human handed it, making it a reconciliation path for missed webhook
  deliveries rather than a second, looser trigger. With the app's own user id
  unknown the poller claims nothing rather than everything.
- The Credentials page is grouped into **Agent providers** (Claude, ChatGPT/Codex,
  Kimi, Cursor — the backends that execute nodes, and the only ones with a usage
  card and billing lane) and **Integrations** (GitHub, Linear).

### Removed

- **The Linear eligibility label ("AI Eligible") is gone**, along with the
  `label` field on trigger bindings, its form control, and the `label=` parameter
  on the preview endpoint. Delegation replaces it as the pickup signal, in both
  the webhook and the poller. The binding is still required — it maps a team to
  its project, workflow, base branch and status map — and delegation resolves
  through it whether or not it is `enabled`. One behaviour change to note:
  "Create issue" from a report files the issue without starting it, for a human to
  delegate. The `label` column is left in existing databases, unread.
  `failed_label` is unaffected.

### Changed

- **Linear is configured globally, not per project.** The identity connected to
  Linear is the app, which is the same for every project, so the Linear fields
  have moved off the project-credentials dialog; only the trigger *bindings*
  remain per project. A per-project `linear` credential stored by an earlier
  version is inert — reconnect once on the Credentials page. Per-project GitHub
  overrides are unaffected.

Initial work. `ai-harness` is seeded from
[majiayu000/harness](https://github.com/majiayu000/harness) (MIT) and is being
adapted into a Kubernetes-native AI coding harness with Linear triggers,
user-authored workflow DAGs, and UI-managed toolchain provisioning.
