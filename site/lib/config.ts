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
    "A Rust-native orchestration layer for AI coding agents. Turn a task into a reviewed pull request with workflow DAGs that drive Claude Code, Codex, Pi/Kimi and Cursor.",
  /** Public origin. Used for canonical URLs and OG image URLs. */
  url: "https://example.com",

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

  navItems: [
    { href: "/", label: "Home" },
    { href: "/docs", label: "Docs" },
    { href: "/docs/workflows/authoring", label: "Workflows" },
    { href: "/docs/operating/deploy", label: "Deploy" },
  ],
} as const

export type SiteConfig = typeof siteConfig
