# GEO audit — design

**Status:** proposed (design only; nothing built yet).

A feature for ai-harness: audit a project's live site for **GEO** (Generative
Engine Optimization — how well a page is set up to be cited by AI search:
ChatGPT, Claude, Perplexity, Gemini, Google AI Overviews), produce a **scored
report inside the harness**, and turn each finding into a one-click `idea-to-pr`
run that lands the fix as a PR against the same project's repo — then re-audit
and watch the score move.

Reference / inspiration: the public `zubair-trabzada/geo-seo-claude` repo (a
Claude Code skill bundle). We take its **audit + scoring + findings**
methodology as *ideas* and **cut its agency/sales half** (prospect CRM,
proposals/pricing, white-label, client PDF reports). If we ever lift its prompt
text or scoring rubric verbatim, check that repo's license first.

## Guiding principle

**All GEO domain logic lives in the workflow layer** — a bundled `geo-audit`
workflow plus a couple of `script` helpers (fetch/probe/score) and prompts. The
Rust core gets only **small, general** additions (a project field, a variable, a
generic "findings → trigger" affordance, two read-only views). Nothing
GEO-specific belongs in the orchestrator core — consistent with the existing
philosophy (even git/gh live in agent prompts, not the core).

## The loop

```
project (repo + external_url)
        │  trigger "GEO audit"
        ▼
   geo-audit workflow ──► structured score + findings + report artifact
        │                         │
        │                         ▼  per finding: "Build this"
        │                    idea-to-pr (same project) ──► PR ──► merge
        │                         │
        └──────── re-audit ◄──────┘   → score delta over time
```

The **project is the join**: its `external_url` is what gets audited; its repo
is where fixes land. The harness's unique value over a standalone audit skill:
it **implements** findings as PRs and **measures the score improving** — not just
recommends.

## Why it fits the system (not a divergence)

- It's methodology (prompts + scripts), not a scraper engine → fits "domain
  logic in the workflow layer."
- The audit is already a fan-out/synthesis **DAG** (discovery → parallel
  analysis → synthesis), which maps almost 1:1 onto a harness workflow.
- "Findings → triggerable tasks → PR" is the **Linear-poller pattern
  generalized** (external signal → task → run → PR).
- A GEO report is just another **run artifact**; the UI already renders
  artifacts and per-run overviews.

It *would* diverge if built as a bespoke SEO/crawler/dashboard product in the
core — so we don't.

## The `geo-audit` workflow (DAG)

```
discover ─┬─► technical     ─┐
(script:  ├─► crawlers       ─┤
 fetch    ├─► schema         ─┼─► synthesize ─► report
 raw HTML,├─► content/EEAT   ─┤   (composite     (geo-report.md
 robots,  └─► citability     ─┘    score +         artifact)
 headers,                          severity +
 a few pages)                      action plan)
```

- **discover** — `script` node (uv/python: `requests` + `bs4`). Fetches **raw
  HTML preserving `<head>`** (the agent's WebFetch strips it), `robots.txt`,
  response headers/status, sitemap; collects homepage + a few key pages; writes
  artifacts. **No JS rendering on purpose** — that's exactly what a (non-JS) AI
  crawler sees, which is what we're grading. (A rendered-vs-raw comparison would
  need a headless browser; that's a later nice-to-have, not core.)
- **analysis nodes (parallel,** `depends_on: [discover]`**)** — a mix of script
  and LLM, each emitting `{ score, findings[] }` via `output_format`:
  - `crawlers` (mostly script): parse `robots.txt` for the ~14 AI bot
    user-agents; probe for 403-on-bot-UA (the #1 silent killer — see the
    Electron example in the source repo: 28/100, root cause a 403 blocking
    non-browser UAs).
  - `schema` (script + light LLM): extract/validate `<script type=ld+json>`.
  - `technical` (script + LLM): SSR-vs-CSR heuristic (CSR-only ≈ invisible to AI
    crawlers), meta tags, security headers, Core-Web-Vitals risk.
  - `content`/`EEAT` + `citability` (LLM judgment): passage-level analysis of the
    fetched content (answerability, self-containment, fact density, author/trust
    signals).
  - Source cross-reference (Slice 3): because the run executes in the project's
    worktree, an analysis node can tie a finding to the source that produced it
    ("missing JSON-LD; add it in `app/layout.tsx:40`") — which makes the
    downstream `idea-to-pr` task precise. A standalone scraper can't do this.
- **synthesize** — reads all dimension outputs, computes the composite **0–100**
  score (weighted formula below), classifies findings by severity, builds a
  prioritized action plan, emits the structured report + a `geo-report.md`
  artifact.

### Scoring (from the source repo's rubric)

```
GEO = Citability*0.25 + Brand*0.20 + EEAT*0.20 + Technical*0.15
      + Schema*0.10 + Platform*0.10        # 0–100
```

Rating bands: 90–100 excellent · 75–89 good · 60–74 fair · 40–59 poor · 0–39
critical. **Brand** (needs external-platform scans) and **Platform** are the
network-heavy / least-repo-fixable dimensions → deferred past the MVP (see
slices); the MVP weights are renormalised over the dimensions it does compute.

### Synthesis output schema (`output_format`)

```jsonc
{
  "score": 0,                         // 0–100 composite
  "rating": "critical|poor|fair|good|excellent",
  "categories": [
    { "key": "technical", "weight": 0.15, "score": 0, "summary": "" }
  ],
  "findings": [{
    "severity": "critical|high|medium|low",
    "category": "crawlers",
    "title": "AI crawlers blocked by 403 on bot user-agents",
    "detail": "GPTBot / ClaudeBot receive 403 …",
    "fix": "Allow these UAs in the edge/server config",
    "effort": "quick|medium|strategic"
  }],
  "pages": [{ "url": "", "title": "", "issues": 0 }]
}
```

This one shape drives the dashboard, the per-finding trigger buttons, and the
trackable `score`.

## Core additions (small, general — NOT GEO-specific)

1. **`project.external_url`** — column + register/update API + a field on the
   projects page. Useful project metadata regardless of GEO.
2. **`$EXTERNAL_URL` variable** — substituted into a run when its project has a
   URL, like `$BASE_BRANCH` / `$ARTIFACTS_DIR`.
3. **Findings → trigger affordance** — a generic UI: a run whose structured
   output carries a findings/action-plan array renders each item with a
   one-click "send to `idea-to-pr`" (reuses `run_trigger`, project pre-filled).
   GEO is its first consumer; code-review/audit workflows could reuse the same
   shape.
4. **GEO report view** (web route) — reads the audit run's structured output →
   score dashboard + severity-grouped findings + the trigger buttons.
5. **Score-trajectory view** (web) — reads the project's `geo-audit` runs, plots
   `score` over time + delta vs last. v1 derives from persisted run
   outputs/artifacts; no new storage.

Everything else (fetch/probe scripts, the dimension criteria, the rubric, all
prompts) is **bundled workflow content**, not core.

## Explicitly cut (the agency/sales half)

Prospect CRM, auto-proposals/pricing tiers, white-label, client PDF reports. We
keep **audit + scoring + findings + delta-tracking** only.

## Build plan (slices — each ships standalone)

- **Slice 0 — foundation (tiny):** `external_url` on projects + `$EXTERNAL_URL`
  injection. Mergeable alone; generally useful.
- **Slice 1 — audit MVP:** a `geo-audit` workflow = `discover` + **one
  consolidated analysis prompt** (not yet parallel) → the structured score + a
  `geo-report.md` artifact. Trigger from the UI (pick a project that has a URL);
  view it as an artifact (already renders). The cheap end-to-end proof — do this
  before anything fancy. Defines the score schema.
- **Slice 2 — close the loop:** the generic findings→`idea-to-pr` trigger + a
  proper GEO report view (dashboard + per-finding "Build this").
- **Slice 3 — depth:** split analysis into parallel dimension nodes + the
  script-based checks (crawler probing, raw-HTML schema, SSR detection) + source
  cross-reference (audit reads the repo).
- **Slice 4 — measurement:** per-project score trajectory + delta-vs-last (the
  audit → fix → re-audit loop made visible). The showcase.
- **Deferred:** multi-page crawl at scale; Brand-mention external scans
  (Wikipedia/Reddit/etc.); `llms.txt` generation as its own fix workflow;
  Platform-specific scoring; rendered-vs-raw comparison via headless browser.

## Open decisions

- **v1 dimensions:** include all 6, or skip **Brand** (external network scans)
  and **Platform** in Slice 1, add in Slice 3? Leaning: skip Brand in the MVP.
- **Findings trigger granularity:** one `idea-to-pr` per finding (start here) vs
  batching related findings into one task (multi-select later).
- **Score storage:** derive from run artifacts (v1, zero new schema) vs a
  denormalised `geo_score` column for fast charts (later if needed).
- **License:** lift the rubric/criteria as *ideas* (safe) vs copy prompt text
  verbatim (check the source repo's license first).
