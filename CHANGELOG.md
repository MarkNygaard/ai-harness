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

### Fixed

- **A run can no longer commit in a repo it never touched.** On a frontend-only
  ticket, `pi-simplify` reported *"backend: 79 files in scope, simplified"* for a
  repo whose diff was empty, rewrote analytics code there, and `finalize-pr`
  opened a 42-file pull request against an untouched service. It had happened
  before, in the same step, on a different ticket — its prompt already forbade it
  in bold, which is the evidence that prose is not a mechanism: an agent that
  computes its own changed-file set gets a plausible answer from the wrong base
  or from a `git` call at the multi-repo root, and plausible is worse than an
  error. The set is now computed once by a `scope` node — per repo, against
  **that repo's own** base branch rather than the run's, which is a second bug in
  the same place (a repo based on `develop` diffed against `origin/main` reports
  two release lines of work as this run's) — and written to `scope.json` with
  each repo's head. `pi-simplify` reads it and is told not to re-derive it, and a
  `guard-scope` node then reverts any repo whose file set was empty but whose
  tree moved, before `validate` sees it. Reverting is safe by construction: an
  empty set means the repo carried none of this run's commits. Ignored files are
  left alone, so the warm build state survives.
- **Every pull request title is now gated, not requested.** `verify-pr-title` is
  told that a Linear task's title must end with the issue identifier; on a
  two-PR run it renamed the first and declared the second *"TITLE OK"* with no
  identifier, so Linear never linked it — a three-part condition in a paragraph,
  applied to one PR out of two. A `gate-pr-titles` node now checks every entry in
  `.pr-list`: the conventional-commits format, and the identifier read from
  Linear by issue id rather than pattern-matched out of the task text. Appending
  a missing identifier is mechanical, so the gate does it (via `gh api`, which
  also sidesteps the `read:org` scope `gh pr edit` needs and this deployment's
  token lacks); a title that does not match the format at all needs a type and
  scope chosen for the change, so that fails the node instead of being guessed
  at. Nothing proceeds to review with a wrong title.
- **Commands stop reading an artifact the default pipeline never writes.**
  `plan-context.md` is written by `plan-setup`, which no bundled workflow uses —
  the default planner writes `plan.md` — so `implement-tasks`, `validate` and
  `finalize-pr` named a file that was reliably absent, and the resulting misses
  were among the most frequent entries in `run_activity_errors` (10 occurrences
  across 3 runs of one project). They now name the artifact the run actually
  wrote, with `plan-context.md` as the `plan-setup` variant, and say that the
  directory listing is the authority.
- **A multi-repo project can name the primary repo's folder, and listing it no
  longer checks it out twice.** The project's Git URL is always the first repo of
  the layout, and its folder name was derived from the URL — so a project wanting
  a folder called `frontend` added a row for that same repo, and got *three*
  checkouts of two repos: the implicit primary as `frontend-monorepo`, the listed
  one as `frontend`, and the other repo. Both folders then held the same repo on
  the same `run/<id>` branch, where a second push either rejects as
  non-fast-forward or clobbers the first; every planning run also spent a
  paragraph working out that two of its three repos were the same one, and
  `install-deps` installed that monorepo twice. A row naming the same remote as
  the Git URL is now recognised as *being* the primary: its folder and role are
  used and no second checkout is made. URL spelling doesn't matter — `https://`,
  `git@`, a `.git` suffix or a trailing slash all compare equal, via the same
  normalisation the git mirror cache uses. Not listing the primary still works
  exactly as before; checking out only what is listed was the alternative, and it
  would silently drop the project's own repo whenever a row was forgotten. The
  run's base branch still wins for the primary, which is what keeps an epic run
  (triggered with `epic/<ID>` as its base) building on the epic branch rather
  than off `main`. Registering a project now also rejects two rows naming the
  same remote, and the UI says the Git URL is already the first repo.


### Added

- **A workflow can cache what only it knows is reusable — `$HARNESS_CACHE_DIR`.**
  The caches added last release are the ones the harness can infer (package-manager
  downloads, git objects). A project whose expensive input is none of those had no
  way to keep anything: the worktree is thrown away after a run and `ARTIFACTS_DIR`
  with it. Runs now inherit `HARNESS_CACHE_DIR`, a per-project directory on the
  persistent volume, bounded by `HARNESS_PROJECT_CACHE_CAP_GB` (default 5 GiB per
  project) and reported in the project's cache dialog. Eviction is by whole
  immediate subdirectory, so a workflow puts its cache key in the top-level name
  (`alpackages-<hash>/`) and one stale entry can go without taking the live one.
  Documented for authors in `docs/authoring-workflows.md`, including the two
  mistakes that make a cache worse than none: a directory that can be read while
  half-written, and a key that cannot see its source change.
- **`bc-idea-to-pr` stops re-downloading 96 MB of BC symbols every run.** The
  setup step fetched every symbol package from the on-prem dev endpoint on every
  run — 17 packages, `Base Application` alone 45 MB, behind a 15-minute timeout —
  and any single failed download failed the whole run. They are now cached under a
  key covering everything that decides their content: `app.json`'s dependency set,
  its pinned `platform`/`application`, and the endpoint URL. Restoring is gated on
  a completeness marker written inside a directory that is moved into place whole,
  because a half-restored `.alpackages` is the worst outcome available here — it
  surfaces hundreds of lines later as cryptic `AL1022` errors rather than as a
  failure to fetch symbols. The cache also expires after 7 days
  (`BC_SYMBOL_CACHE_TTL_DAYS`): the endpoint serves whatever the *server* has, and
  a BC cumulative update moves the symbols without `app.json` changing, so a key
  alone would compile against symbols older than the server indefinitely. The set
  is stored after the AL1022 resolve loop, so the platform packages it discovers
  are cached too. One consequence worth having: with a complete set on disk, an
  unreachable endpoint no longer fails the run — it falls back, loudly, to symbols
  that may be older than the server.
- **Runs reuse dependency downloads and git objects, not just Rust build artifacts.**
  Only `CARGO_TARGET_DIR` was pointed at the persistent volume, so a project with no
  Rust in it had literally nothing cached — the cache dialog read `0` for a
  JS/.NET project because there was nothing to read. Every run now inherits a shared
  download cache at `<projects_dir>/.deps-cache`: pnpm's store (both the pnpm ≤10
  `npm_config_*` and pnpm 11 `pnpm_config_*` spellings), npm/bun/yarn caches, NuGet's
  global packages folder and HTTP cache, `GOMODCACHE`/`GOCACHE`, uv/pip/poetry, and
  Composer. It sits on the same filesystem as the worktrees, which is what lets
  `pnpm install` hardlink `node_modules` into a fresh tree instead of downloading it.
  Multi-repo runs additionally clone from persistent bare mirrors under
  `.git-cache` — previously the only path that reused a checkout was the single-repo
  worktree, so each repo of a multi-repo project transferred its whole history every
  run; the mirror fetches only what is new and the run's clone hardlinks its objects.
  A mirror that can't be brought up to date is bypassed rather than trusted, so a
  stale mirror can never decide what a run builds. Both caches are swept: the
  dependency cache by whole ecosystem subdirectory when it passes
  `HARNESS_DEPS_CACHE_CAP_GB` (default 20), git mirrors when idle longer than
  `HARNESS_GIT_MIRROR_TTL_DAYS` (default 30). Whole subdirectories rather than
  individual files, because these caches are indexed — evicting one file out of a
  content-addressed store leaves an index entry pointing at nothing. Build *task*
  caches (turbo/nx, `.next/cache`) are deliberately still cold: they decide whether
  to re-run a step, and codegen driven by a remote schema (Sanity types) has inputs
  no hash of the tree can see, so replaying a cached `typecheck` would check the
  wrong types. The cache dialog now reports all three sizes, marking the shared ones
  as shared.
- **`run_activity_errors` (MCP)** — what the agents keep tripping over, across runs
  rather than one at a time. The activity table already recorded every tool result,
  but the only reader was a per-run tail feeding the live UI, so a repeated obstacle
  was visible only as a line in a feed nobody re-reads. Identical failures are now
  collapsed into one group: occurrences, how many distinct runs hit it, the workflow
  and nodes, a verbatim sample, and first/last seen — most-repeated first. A high
  run count is the signal worth acting on: it means the obstacle is a property of
  the project (a generated file that isn't there, an absent credential, a command
  somewhere other than where the agent looked) and belongs in that project's
  `CLAUDE.md` rather than being rediscovered every run. Grouping is by a coarse
  fingerprint that collapses digits and long hex runs, so per-run noise ("took
  1243ms", a uuid) doesn't split a group; the verbatim sample means nothing is lost
  to it.
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

- **A search that matches nothing is no longer recorded as a failure.** Providers
  flag a no-match lookup `isError`, so a run that probed for four generated files
  and correctly found none appeared as four red errors in the activity feed. Not
  merely cosmetic: it would have made the single most common *successful* agent
  pattern — checking whether something exists before acting — the loudest entry in
  any error report built on this data. An `isError` result from a lookup tool
  (`glob`, `grep`, `search`, `find`, `ls`) whose message says nothing matched is now
  recorded as an ordinary result. Keyed on the tool rather than the text, because a
  real failure can say "not found" too: `bash` reporting `command not found` stays an
  error, as does a lookup that failed for a reason of its own such as a permission
  denial. The line is still shown; only its severity changes.
- **A claim recorded before its run row is no longer retired on sight.** `start_run`
  returns as soon as it has a run id; the run row itself is written by the spawned
  run task moments later. The Linear webhook records its claim in between, so a
  claim legitimately references a run that does not exist yet — 6.3 seconds on the
  run that exposed this. The claim sweep treated that as "run gone", set the claim
  to `done`, and thereby ended all Linear reporting for that run permanently: no
  step activities, no status transitions, for its entire life, from a single tick
  landing in a few-second window (a ~20% chance at a 30s cadence). A missing row is
  now tolerated for 10 minutes before the claim is dropped, and the drop is logged.
  Affects delegated and poller-claimed runs alike, since both record the claim after
  `start_run` returns.
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
