import { describe, expect, it } from "vitest";
import { slugify, titleFromSlug } from "./workflow-name";

/** Every workflow bundled with the harness today. */
const BUNDLED = [
  "architect",
  "bc-idea-to-pr",
  "geo-audit",
  "idea-to-pr",
  "judge-ab",
  "merge-pr",
  "review-area",
  "revise-pr",
];

describe("titleFromSlug", () => {
  it("titles the bundled workflows readably", () => {
    expect(titleFromSlug("idea-to-pr")).toBe("Idea to PR");
    expect(titleFromSlug("merge-pr")).toBe("Merge PR");
    expect(titleFromSlug("revise-pr")).toBe("Revise PR");
    expect(titleFromSlug("bc-idea-to-pr")).toBe("BC Idea to PR");
    expect(titleFromSlug("geo-audit")).toBe("GEO Audit");
    expect(titleFromSlug("judge-ab")).toBe("Judge AB");
    expect(titleFromSlug("review-area")).toBe("Review Area");
    expect(titleFromSlug("architect")).toBe("Architect");
  });

  it("keeps minor words lower-case unless they lead", () => {
    expect(titleFromSlug("plan-of-the-week")).toBe("Plan of the Week");
    // Leading minor word still gets capitalised — it starts the title.
    expect(titleFromSlug("to-do-sweep")).toBe("To Do Sweep");
  });

  it("capitalises unknown tokens so any project workflow reads well", () => {
    expect(titleFromSlug("nightly-dependency-bump")).toBe(
      "Nightly Dependency Bump",
    );
    expect(titleFromSlug("sync_shopify_catalog")).toBe("Sync Shopify Catalog");
  });

  it("handles digits, separators and degenerate input", () => {
    expect(titleFromSlug("gpt-5-review")).toBe("Gpt 5 Review");
    expect(titleFromSlug("double--hyphen")).toBe("Double Hyphen");
    expect(titleFromSlug("")).toBe("");
    // Nothing usable → hand back what we were given rather than an empty string.
    expect(titleFromSlug("---")).toBe("---");
  });
});

describe("slugify", () => {
  it("produces canonical slugs from free text", () => {
    expect(slugify("Idea to PR")).toBe("idea-to-pr");
    expect(slugify("  Nightly   Dependency Bump  ")).toBe(
      "nightly-dependency-bump",
    );
    expect(slugify("GEO Audit")).toBe("geo-audit");
    expect(slugify("Sync: Shopify catalog!")).toBe("sync-shopify-catalog");
  });

  it("is idempotent on an existing slug", () => {
    for (const name of BUNDLED) expect(slugify(name)).toBe(name);
  });
});

describe("the round trip", () => {
  // The invariant that makes it safe to show a title where the slug used to be:
  // a displayed title always slugifies back to the identity it came from, so it
  // can never imply a workflow the harness wouldn't resolve.
  it("slugify(titleFromSlug(slug)) === slug", () => {
    const slugs = [
      ...BUNDLED,
      "nightly-dependency-bump",
      "plan-of-the-week",
      "gpt-5-review",
      "sync-shopify-catalog",
      "a",
      "ui-only",
    ];
    for (const slug of slugs) {
      expect(slugify(titleFromSlug(slug))).toBe(slug);
    }
  });
});
