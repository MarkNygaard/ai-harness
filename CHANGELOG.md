# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

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
