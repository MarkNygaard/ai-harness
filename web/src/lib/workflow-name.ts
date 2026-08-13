/**
 * Human titles for workflow slugs, and back again.
 *
 * A workflow's **slug** (`idea-to-pr`) is its identity everywhere that matters:
 * the YAML filename, the runs API, MCP calls, Linear trigger bindings. It is not
 * pleasant to read in a heading, so the UI derives a **title** ("Idea to PR")
 * from it for display only.
 *
 * The pair is designed so that `slugify(titleFromSlug(slug)) === slug` for every
 * slug — the title can therefore never imply an identity the harness wouldn't
 * resolve. That invariant is what keeps this safe to show in place of the slug,
 * and it is asserted over the real bundled names in the tests.
 */

/**
 * Tokens shown upper-case rather than capitalised. Values are letters/digits
 * only: adding punctuation (`A/B`) would break the slug round-trip.
 */
const UPPERCASE: Record<string, string> = {
  ab: "AB",
  ai: "AI",
  api: "API",
  bc: "BC",
  cli: "CLI",
  geo: "GEO",
  id: "ID",
  json: "JSON",
  mcp: "MCP",
  pr: "PR",
  prs: "PRs",
  qa: "QA",
  sdk: "SDK",
  seo: "SEO",
  ui: "UI",
  url: "URL",
  yaml: "YAML",
};

/** Words left lower-case unless they lead the title. */
const MINOR = new Set([
  "a",
  "an",
  "and",
  "at",
  "by",
  "for",
  "from",
  "in",
  "of",
  "on",
  "or",
  "the",
  "to",
  "vs",
  "with",
]);

/** Split on anything that isn't a letter or digit, dropping empties. */
function tokenize(input: string): string[] {
  return input.split(/[^a-zA-Z0-9]+/).filter(Boolean);
}

/**
 * The canonical slug for a free-text title: lower-case, alphanumeric, joined
 * with single hyphens. This is what gets written to disk and used everywhere.
 */
export function slugify(input: string): string {
  return tokenize(input).join("-").toLowerCase();
}

/**
 * A display title for a slug — `idea-to-pr` → `Idea to PR`.
 *
 * Unrecognised tokens are simply capitalised, so a project workflow with any
 * name gets something readable without needing to be in a list. Falls back to
 * the input unchanged when there is nothing to work with.
 */
export function titleFromSlug(slug: string): string {
  const tokens = tokenize(slug);
  if (tokens.length === 0) return slug;
  return tokens
    .map((raw, i) => {
      const lower = raw.toLowerCase();
      const upper = UPPERCASE[lower];
      if (upper) return upper;
      // Minor words stay lower-case, except when they lead.
      if (i > 0 && MINOR.has(lower)) return lower;
      return lower.charAt(0).toUpperCase() + lower.slice(1);
    })
    .join(" ");
}
