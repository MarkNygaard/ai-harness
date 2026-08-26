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
- **The Codex default model is `gpt-5.6-sol`, up from `gpt-5.4`.** Every bundled
  workflow already pins `openai-codex/gpt-5.6-sol` explicitly, so the stale default
  only applied to a `provider: codex` node that omitted `model` — latent rather than
  active, but two generations behind what the project standardised on.
- **The Anthropic API adapter's default `max_tokens` is 16000, up from 4096.** That
  budget caps thinking *and* response text together, and on the 5 series adaptive
  thinking is on whenever a request omits a `thinking` field — which this adapter
  does. At 4096 it would have spent much of the budget reasoning and truncated the
  answer. 16000 is the recommended ceiling for a non-streaming request.

### Added

- **`run_linear_claim` (MCP)** — read how a run is tied back to Linear: the claim
  row the poller sweeps every 30 seconds to report progress and move the issue.
  Returns the issue identifier, `phase`, `agent_session_id`, `reported_nodes` and
  `last_activity_at`. Added because diagnosing a silent session took a line-by-line
  read of the poller — nothing outside the process could see whether the claim
  existed, carried a session, or had simply stopped being swept. No row means the
  run was never linked; a null `agent_session_id` means there is no session to post
  into; a `last_activity_at` far behind the run's progress means the posts are
  failing or the poller is not sweeping.
- **The harness answers questions asked in a Linear agent session.** Writing in the
  session mid-run previously got a fixed "I can't change course" reply whatever you
  asked. A follow-up is now answered from the run's own state — step statuses plus the
  `exploration.md` / `plan.md` artifacts, all already in Postgres — so "what are you
  doing?" or "where's the bug?" get real answers while the run is going. Nothing
  touches the run: no worktree, and tools are denied at the CLI boundary rather than
  merely discouraged in the prompt, since the question text is written by anyone who
  can comment on the issue. Asking is always safe,
  and an instruction to work differently still gets the honest refusal. The reply is a
  `thought` rather than a `response`, because Linear treats a response as the agent's
  final word and would mark the session complete mid-run. It answers the question that
  was asked, leading with the answer: the first live use asked for a summary of the plan
  and got back which step was running, with the plan folded into a subordinate clause
  that Linear then clipped mid-word. The context had been read correctly — the reply
  named symbols that appear only in `plan.md` — but the prompt opens with a step-status
  table, which reads as the headline unless something says otherwise. Two or three
  sentences, no headings, and an admitted gap in preference to a nearby question it
  happens to be able to answer.
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

### Changed

- **The GEO audit is built for ecommerce, and audits a product page.** It only ever
  fetched the entry URL — where a store has no `Product` schema, no price, no
  availability and no reviews — so it reported the same non-findings for every shop
  and could not see the markup that decides whether an AI shopping surface can quote
  a price. It now samples the catalogue: `pick-pages` chooses a real product and
  category page, preferring the site's own `llms.txt` / `llms-full.txt` listing where
  it exists (curated, and each entry carries a description, so a category is
  identifiable without inferring URL shapes), then the sitemap — following one level
  of sitemap index — where individual products actually live, then the homepage nav,
  and finally a product link taken off the chosen category page for a store whose
  PDPs never reach the sitemap. `fetch-pages` retrieves them without JavaScript, and each page
  is reduced to visible text so citability isn't scored against nav and footer
  chrome. Five dimensions replace four — `schema` is now the heaviest at 25% and
  scores Product/Offer against Google's merchant-listing ladder (`aggregateRating` →
  identifiers → `shippingDetails` → `hasMerchantReturnPolicy` → reviews) plus the
  validation errors that void a listing (`"249,00 kr"` in `price`, a bare `InStock`,
  an expired `priceValidUntil`, variants as duplicate `Product` blocks); a new
  `entity` dimension (15%) looks off-site, where brand mentions correlate with AI
  citation roughly 3x more strongly than backlinks and ChatGPT/Perplexity draw on
  Wikipedia and Reddit. `content` gained measurable citability anchors instead of
  "fact-dense paragraphs", plus PDP-specific checks — unique copy rather than
  manufacturer boilerplate, specs tables, size and fit in text, on-page reviews,
  imagery and `alt`. The report now breaks readiness down per platform (AI Overviews,
  AI Mode, ChatGPT, Perplexity, shopping agents), separates codebase fixes from
  off-site ones so nobody files a PR to obtain a Wikipedia article, and states that
  the scores are heuristics over public signals rather than Google-internal ranking
  data. Prompted by reading `AgriciDaniel/claude-seo` (MIT), whose GEO skill carries
  sourced figures for most of the above.
- **The GEO audit no longer scores a page it failed to fetch.** `discover` treated a
  failed homepage fetch as a note and carried on, so a site that was down or blocking
  our user-agent produced a confident four-dimension audit of a missing file. It now
  fails the run with a message saying not to read a score into it.
- **The GEO audit grades AI crawlers by purpose.** It listed eleven bots and called a
  `Disallow` reaching any of them critical. Blocking a *training* crawler (CCBot,
  anthropic-ai, Google-Extended, Bytespider) is a deliberate choice thousands of
  brands make, and reporting it as a defect invited a PR to undo a policy decision;
  meanwhile ChatGPT-User and Google-Agent ignore robots.txt by design, so advice
  about them was inert. Blocking is critical only for the citation crawlers (GPTBot,
  OAI-SearchBot, ClaudeBot, PerplexityBot); training opt-outs are reported as
  informational; user-triggered fetchers are pointed at edge controls. Live status
  probes now cover all four citation crawlers rather than GPTBot alone, so a CDN that
  answers a browser with 200 and a bot with 403 is caught per bot.
- `llms.txt` / `llms-full.txt` remain a priority quick win in the GEO audit by
  choice, but the finding now has to state the basis honestly: Google's AI
  optimization guide says Google Search ignores these files and no major AI search
  provider has documented consuming a third-party one, so we ship them for non-Google
  surfaces, AI coding and shopping agents, and cheap optionality — not as a Google
  citation lever. The audit also checks `.well-known/ucp`, the Universal Commerce
  Protocol profile an AI shopping agent looks for, reported as an opportunity rather
  than a defect.

### Fixed

- **A Linear issue that cannot be moved no longer looks like one that was.** Every
  state transition in the poller was `let _ = client.set_issue_state(…)` — the
  failure discarded, not even logged — and the completed branch then set the claim
  to `done` unconditionally while logging `run completed → ready` regardless. So a
  rejected move stranded the issue in the wrong column permanently, with the logs
  asserting the opposite: a finished run left its issue in In Progress and nothing
  anywhere recorded why. Transitions now report failure, the claim stays open so the
  next tick retries, and after 6 hours of failed retries the poller comments on the
  issue and gives up rather than looping in the background. The success line is only
  logged when the move actually landed.
- **One stalled HTTP request could stop every Linear update the harness makes.** A
  delegated run reported nothing into its Linear session for 50 minutes — no step
  activities, and no move to In Review even after its PR was opened — while the run
  itself proceeded normally. The Linear client was built with
  `reqwest::Client::new()`, which applies **no** request timeout, and the poller
  awaits those calls inline in its tick loop: one connection that opened and then
  stalled wedged the loop for the lifetime of the process. Every claim stopped being
  swept, and because the loop never reached its next statement, nothing was logged
  to say so — the only visible symptom was Linear's own "stopped responding". The
  client now has a 30-second timeout (compile-time asserted non-zero), and a tick is
  additionally bounded and unwind-guarded, so a hang or a panic costs one tick
  rather than the poller.
- **The Linear session no longer goes quiet when an activity is rejected.**
  `report_to_session` returned `true` whenever a session id existed — even when the
  post had just failed — and callers use that return to decide whether to fall back
  to a plain issue comment. So a rejected activity produced neither a session entry
  nor the comment meant to replace it. It now reports what actually happened.
- **A credential the poller cannot use is logged at `warn`, not `debug`.**
  `linear_client_or_none` returning `None` makes the poller skip every claim, so at
  debug level the harness silently stopped transitioning issues with nothing in the
  logs to explain it.
- **One dead sitemap URL no longer blinds the whole GEO audit.** The first real run
  exposed three faults in a row. `pick-pages` took the first product in the store's
  sitemap; that URL 404'd — a stale slug left behind by a rename, which the site
  never removed and does not redirect — and `fetch-pages` gave up after a single
  attempt, so the audit scored the entire shop having never seen a product page and
  the heaviest-weighted dimension fell back to reading template source (it said so,
  at least: "no product.html captured" was its own first critical finding). The
  picker now writes a candidate list — up to four products and two categories,
  chosen to be unlikely to fail together — and `fetch-pages` walks it until a page
  actually returns; `fetch` reports failure instead of swallowing it, which is what
  the walk needs to advance. Verified against the real store: two dead candidates
  skipped, the third fetched, and the PDP signals captured.
- **The GEO audit reports a sitemap that lists dead URLs.** It tripped over one and
  said nothing. `discover` now samples the sitemap's own URLs for liveness — evenly
  spaced, because stale slugs cluster and the first entries alone give a biased
  rate — and the `crawlers` dimension reports the sampled figure as a finding, high
  severity above roughly one in ten, quoting `M of N sampled` rather than
  extrapolating. A sitemap full of 404s spends the crawl budget AI crawlers use for
  discovery on nothing, and an assistant that cites one sends a reader to a dead
  page. On the store audited, 2 of 12 sampled were dead.
- **The GEO audit's sitemap sampling no longer starves categories and editorial.**
  Child sitemaps were appended whole and the combined list truncated at 400, so the
  product children filled the entire quota: the URL list held 400 product URLs and
  not one category or article. The picker had to guess a category from the homepage
  nav, and the `content` dimension reported the site's care and repair guides as
  missing from its sitemap — a false high-severity finding, reasoned correctly from
  starved data, since those guides sat in an articles child nobody had read. The cap
  is now applied per child with a pass per kind (products, categories, editorial).
  Same store: 400 product-only URLs became 300 product + 150 category + 144
  editorial, and the 76 care-and-repair URLs that produced the false finding are now
  present.
- **A finding handed to `idea-to-pr` now says which page it was seen on.** The report's
  "Build this" action turns a finding into a task, and the task description named the
  project's entry URL as the live site — fine when the audit only looked at the
  homepage, wrong the moment it samples a product and a category page too, because a
  missing `aggregateRating` is a fact about the *product* page and an implementer sent
  to the site root would correctly report that the homepage has no Product markup and
  change nothing useful. Findings now carry `page` (observed on), `location`
  (template/component, only when confirmed in the source) and `offsite`, all carried
  through the merge unchanged; the description leads with the observed page and labels
  the entry URL as the site root. The closing instruction is assembled from what the
  finding actually holds — it previously told every implementer to "make the change in
  the repo/folder named in the location above" even though `location` was a field no
  workflow but `review-area` ever set, so for every GEO finding that sentence pointed
  at nothing. `effort` is included, so a `strategic` item doesn't arrive looking like a
  quick fix. Off-site findings ("earn a Wikipedia entity") are excluded from the build
  path altogether — they stay filable as an issue for a human, and the description
  tells the agent not to invent a source change to satisfy one.
- **Two silent shell bugs in the GEO audit's measured signals.** `grep -c` counts
  matching *lines*, and both minified store HTML and the audit's own extracted text
  are a single line — so "12 images, 3 with usable alt" was reported as "1, 1", and
  every schema-field count capped at 1. And the sitemap-index preference for a
  product child matched on `item`, which is a substring of "sitemap", so it matched
  every child and never ranked anything. Both yielded a plausible number instead of
  an error, which is why they survived; both are now pinned by a test.
- **`revise-pr` cancels when nobody actually asked for changes.** A card moved to
  "Changes requested" by hand, with no comment on the issue or the PR, made the harness
  plan and push a second speculative fix to a PR nobody had complained about. The guard
  for exactly this exists — `abort-no-feedback`, whose message says "if this issue was
  moved to 'Changes requested' by mistake, move it back" — but it never fired:
  `gather-feedback` reported one `has_feedback` boolean, and the issue's *own bug report*,
  which the poller puts in `$ARGUMENTS` on every run, passed as a tester saying the fix
  had failed. The node found 0 GitHub reviews and 0 comments, said so, and still returned
  true; the planner then invented a root cause ("the tester's repro ends in a failed
  server action") that no one had reported. Now the two sources are reported separately —
  `has_github_feedback` and `has_linear_feedback` — so feedback must be attributed before
  the gate opens, and the Linear one is a question about a string's presence ("does the
  `## Reviewer comments (Linear)` heading appear?") rather than a judgement about
  relevance. The prompts for `gather-feedback`, `explore`, `create-plan` and `implement`
  now all state that the text before that heading is the original report the PR already
  addresses. Projects with their own saved `.harness/workflows/revise-pr.yaml` shadow the
  bundled copy and need re-saving to pick this up.
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
