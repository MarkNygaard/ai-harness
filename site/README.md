# site

The public landing page and user documentation, as one Next.js app.

- **Shared chrome** — `app/(app)/layout.tsx` holds the header and footer, and
  both the landing page and the docs live under that group. The docs are a
  section of the site, not a second site, so fumadocs' own nav is disabled and
  its docs layout contributes only the sidebar.
- **Landing page** — `app/(app)/page.tsx`.
- **Docs** — markdown in `content/docs/`, rendered through
  [Fumadocs](https://fumadocs.dev) by the catch-all route
  `app/(app)/docs/[[...slug]]/page.tsx`. Adding a page means adding a file;
  ordering and section titles are the `meta.json` files, not code. Four
  sections: getting started at the root, then `workflows/`, `triggers/` and
  `operating/`, each with its own `meta.json`.
- **Identity** — `lib/config.ts` holds the product name, URL, links and nav.
  Nothing under `app/` should hard-code the name; import `siteConfig` instead.
  This is what keeps a project rename to one file plus a pass over the docs.
- **Social cards** — generated at build time by `app/opengraph-image.tsx` from
  `siteConfig`. There are no PNGs to re-cut when something changes.
- **Header controls** — `components/search-trigger.tsx` and
  `components/mode-toggle.tsx`. Both sit in the site header, so they are
  reachable from the landing page and not only inside the docs; fumadocs'
  own sidebar copies are switched off in the docs layout.
- **Search** — `app/api/search/route.ts`. The index is built once at build time
  and served as a file (`staticGET`), which is what a static export supports;
  the client is switched to the matching `type: "static"` in the root layout.
  The trigger reads its shortcut hint from fumadocs' own `hotKey` descriptor
  rather than hard-coding ⌘K, so the hint cannot drift from the real binding.

## Develop

```bash
cd site
bun install
bun run dev      # http://localhost:4000
```

## Before it goes live

Set **`NEXT_PUBLIC_SITE_URL`** to the real origin. It is the base for canonical
URLs, OG image URLs, `sitemap.xml`, `robots.txt` and `llms.txt`; without it they
are all published under a placeholder domain, which is worse than not publishing
them at all.

## Discoverability

Generated at build time, all from `siteConfig` and the docs tree:

- `sitemap.xml` — the landing page plus every doc page.
- `robots.txt` — open to everything, with the AI crawlers named explicitly.
  Several of them (Google-Extended, Applebot-Extended) only read a directive
  addressed to them by name, so a bare wildcard says nothing to those.
- `llms.txt` — an [llmstxt.org](https://llmstxt.org) index, in sidebar reading
  order rather than filesystem order.
- `llms-full.txt` — every page's markdown concatenated, served from source
  rather than from rendered HTML.
- JSON-LD — `WebSite` + `SoftwareApplication` on the landing page, `TechArticle`
  + `BreadcrumbList` on each doc page.

## Verify

```bash
cd site && bun run typecheck && bun run build
```

Use `bun run`, not `bunx`: `bunx tsc` resolves from the network and can pull a
different TypeScript major than the pinned devDependency.

## Docs chrome

Fumadocs supplies the layout engine — grid, responsive behaviour, table of
contents, mobile. Its surface treatment is overridden in `app/globals.css`,
on fumadocs' own ids, so there are no forked components to re-sync on upgrade:

- `#nd-docs-layout` gets `--fd-layout-width: 100%` for a full-width grid
  (fumadocs otherwise caps it at 97rem and pads the rest into gutters).
- `#nd-sidebar` loses its card fill and its solid end border, which ran the
  full height and collided with the header's own rule. A pseudo-element draws
  a hairline that fades in below the header and out at the foot instead.
  This cannot be `border-image` with a gradient — that renders as a plain
  solid line here, which looks deliberate and is worse than the default.
- From `md` up, the docs are a **fixed-height shell**: the region is exactly the
  viewport below the header, and the content column scrolls inside it rather
  than the page scrolling. That is what makes the separator reliable — its
  gradient spans the sticky sidebar box, so pinning that box to the visible area
  puts both ends of the fade on screen whether or not the page has enough
  content to scroll. Fumadocs' default `--fd-docs-height: 100dvh` takes no
  account of a header above it, so the box began behind ours and overhung the
  bottom by the same amount, hiding both fades.
- Below `md` the shell is off: the sidebar is a drawer and the TOC a popover, so
  it buys nothing, and a page that scrolls itself is what lets a mobile
  browser's address bar collapse.
- The site footer is rendered by the landing page, not the shared `(app)`
  layout — below a viewport-height docs region it would put the page back into
  scrolling.

## Notes

- The site is a **static export** (`output: "export"` in `next.config.mjs`), so
  it deploys to any static host. Point the host at `site/` as its root
  directory; the output lands in `site/out`.
- It is **not** bundled into the server binary. `crates/harness-server/build.rs`
  embeds `web/` only — this app is deployed separately.
- The theme comes from `web/src/styles/theme.css`, imported by `app/globals.css`
  so the site and the dashboard cannot drift apart. That file also remaps
  fumadocs' `--color-fd-*` palette onto those tokens -- without it the docs
  render in fumadocs' greys while the rest of the site uses ours. The remap
  must stay plain CSS rather than `@theme`, which Tailwind hoists to the top
  of the output where fumadocs' own values would overwrite it. `next.config.mjs` sets
  `turbopack.root` to the repository root to permit that import.
- `docs/reference/` and `docs/design/` in the repository root are deliberately
  **not** published: inherited specs and in-flight design notes.
