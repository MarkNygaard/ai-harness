import { describe, expect, it } from "vitest";
import {
  findingKey,
  findingTaskDescription,
  issueActionBlocked,
  type WorkflowFinding,
} from "./report";

const ENTRY = "https://shop.example.com";
const PDP = "https://shop.example.com/produkt/uldtroje-herre";

/** A page-specific finding of the shape geo-audit's `schema` dimension emits. */
function pdpFinding(over: Partial<WorkflowFinding> = {}): WorkflowFinding {
  return {
    title: "Product schema has no aggregateRating",
    severity: "high",
    category: "schema",
    detail:
      "The PDP's JSON-LD Offer is valid but carries no rating or review count.",
    fix: "Add aggregateRating to the Product JSON-LD on the PDP template.",
    page: PDP,
    effort: "medium",
    ...over,
  };
}

describe("findingTaskDescription", () => {
  it("sends the implementer to the page the finding was observed on", () => {
    const task = findingTaskDescription(pdpFinding(), ENTRY);
    expect(task).toContain(`Observed on: ${PDP}`);
    // The entry URL is still useful context, but must not be the page to look at:
    // a PDP defect does not reproduce on the homepage.
    expect(task).toContain(`Site root: ${ENTRY}`);
    expect(task.indexOf("Observed on:")).toBeLessThan(
      task.indexOf("Site root:"),
    );
  });

  it("does not repeat the entry URL when that is the page", () => {
    const task = findingTaskDescription(pdpFinding({ page: ENTRY }), ENTRY);
    expect(task).toContain(`Observed on: ${ENTRY}`);
    expect(task).not.toContain("Site root:");
  });

  it("falls back to no page rather than claiming the wrong one", () => {
    const task = findingTaskDescription(pdpFinding({ page: "" }), ENTRY);
    expect(task).not.toContain("Observed on:");
    expect(task).toContain(`Site root: ${ENTRY}`);
  });

  it("only points at a location when the finding carries one", () => {
    const withLoc = findingTaskDescription(
      pdpFinding({ location: "apps/web/app/produkt/[slug]/page.tsx" }),
      ENTRY,
    );
    expect(withLoc).toContain("Location: apps/web/app/produkt/[slug]/page.tsx");
    expect(withLoc).toContain("Start from the location above");

    // Without one, the task used to instruct the implementer to "make the change
    // in the repo/folder named in the location above" — a line that named nothing.
    const withoutLoc = findingTaskDescription(pdpFinding(), ENTRY);
    expect(withoutLoc).not.toContain("Location:");
    expect(withoutLoc).not.toContain("named in the location above");
    expect(withoutLoc).toContain(
      "Locate the template or component responsible",
    );
  });

  it("tells an off-site finding not to invent a code change", () => {
    const task = findingTaskDescription(
      {
        title: "Brand has no Wikipedia entity",
        category: "entity",
        severity: "high",
        fix: "Establish a Wikipedia article for the brand.",
        offsite: true,
        effort: "strategic",
      },
      ENTRY,
    );
    expect(task).toContain("NOT a code change");
    expect(task).toContain("Do not invent a source change");
    expect(task).not.toContain("Implement this fix in the project's source");
  });

  it("carries severity, category and effort so triage survives the handoff", () => {
    const task = findingTaskDescription(pdpFinding(), ENTRY);
    expect(task).toContain("Finding — schema / high:");
    expect(task).toContain("Effort: medium");
    expect(task).toContain("Fix: Add aggregateRating");
  });

  it("holds together with only a title", () => {
    const task = findingTaskDescription({ title: "Bare finding" });
    expect(task).toContain("Finding: Bare finding");
    expect(task).not.toContain("undefined");
    expect(task).not.toContain("Observed on:");
    expect(task.trim()).toBe(task);
  });
});

describe("findingKey", () => {
  it("keys on category and title so state survives a re-render", () => {
    expect(findingKey(pdpFinding())).toBe(
      "schema::Product schema has no aggregateRating",
    );
  });
});

describe("issueActionBlocked", () => {
  const IDEA = "idea-to-pr";

  it("allows filing once Linear is connected and a binding exists", () => {
    expect(issueActionBlocked("app", true, IDEA)).toBeNull();
    // A personal API key is a supported fallback, not a blocker.
    expect(issueActionBlocked("personal_key", true, IDEA)).toBeNull();
  });

  it("blocks on the global connection, not per-project credentials", () => {
    // The bug this replaced: the report asked `/api/projects/{p}/credentials` for a
    // `linear` row. Linear is configured globally — that endpoint's provider list
    // is ["github"] — so the answer was a permanent no and the button vanished on
    // every report of every project, with nothing on screen to explain it.
    expect(issueActionBlocked("none", true, IDEA)).toMatch(/not connected/);
    expect(issueActionBlocked("none", true, IDEA)).toMatch(/Credentials page/);
  });

  it("treats a still-loading status as not yet connected", () => {
    expect(issueActionBlocked(undefined, true, IDEA)).toMatch(/not connected/);
  });

  it("names the missing binding, since that is what supplies the team", () => {
    const why = issueActionBlocked("app", false, IDEA);
    expect(why).toMatch(/idea-to-pr/);
    expect(why).toMatch(/Linear settings/);
  });

  it("reports the connection first when both prerequisites are missing", () => {
    expect(issueActionBlocked("none", false, IDEA)).toMatch(/not connected/);
  });
});
