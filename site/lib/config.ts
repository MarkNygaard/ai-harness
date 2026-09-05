/**
 * Single source of truth for the product's public identity.
 *
 * The project name is still undecided. Everything the site renders -- page
 * titles, nav, metadata, OG images, the copy-paste install commands -- reads
 * from here, so a rename is an edit to this file plus a pass over
 * `content/docs`. Nothing else in `app/` should hard-code the name; if you find
 * yourself typing it into a component, import it instead.
 *
 * The identifiers below are deliberately separate from `name`: the display name
 * can change without touching the binary, the env-var prefix or the config
 * directory, and those three are the expensive half of a rename because they
 * live in other people's deployments.
 */
export const siteConfig = {
  /** Display name. Change this first when the name is decided. */
  name: "ai-harness",
  /** One-line positioning, used as the meta description and the hero subtitle. */
  description:
    "Turn a written issue into a reviewed pull request. Self-hosted orchestration for AI coding agents, so the person who writes the acceptance criteria can ship and a developer reviews the code.",
  /**
   * Public origin. Used for canonical URLs, OG images, `sitemap.xml`,
   * `robots.txt` and `llms.txt`.
   *
   * Set `NEXT_PUBLIC_SITE_URL` at build time. Until a real domain is wired up
   * this falls back to a placeholder, and the fallback is not harmless: a
   * sitemap and a set of canonicals published under someone else's domain is
   * worse than having none.
   */
  url: process.env.NEXT_PUBLIC_SITE_URL ?? "https://example.com",

  /**
   * Interface identifiers. These appear in users' shells, config files and
   * deployment manifests -- renaming them is a breaking change, so they are
   * tracked apart from the display name above.
   */
  binary: "harness",
  envPrefix: "HARNESS_",
  configDir: ".harness",
  image: "ghcr.io/marknygaard/ai-harness",

  links: {
    github: "https://github.com/MarkNygaard/ai-harness",
  },

  /**
   * Three items, each going somewhere genuinely different: the pitch, the
   * orientation, the manual. Individual doc pages do not belong here. The
   * sidebar covers 22 of them across five groups, and promoting two of those
   * to the header claims they matter more than the rest.
   */
  navItems: [
    { href: "/", label: "Home" },
    { href: "/why", label: "Where it fits" },
    { href: "/docs", label: "Docs" },
  ],
} as const

export type SiteConfig = typeof siteConfig
