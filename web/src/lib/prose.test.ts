import { describe, expect, it } from "vitest";
import { reflowParagraphs } from "./prose";

describe("reflowParagraphs", () => {
  it("collapses soft wraps but keeps paragraph breaks", () => {
    // Shaped like a real `description: |` block: hard-wrapped prose, blank line
    // between paragraphs.
    const input = [
      "The default pipeline: turn a task (title + description, or a PRD path)",
      "into a reviewed PR, end-to-end.",
      "",
      "Flow: Claude plans (a read-only Sonnet `explore` feeding a lean Opus",
      "`create-plan`), Kimi implements and self-reviews.",
    ].join("\n");

    expect(reflowParagraphs(input)).toBe(
      "The default pipeline: turn a task (title + description, or a PRD path) " +
        "into a reviewed PR, end-to-end." +
        "\n\n" +
        "Flow: Claude plans (a read-only Sonnet `explore` feeding a lean Opus " +
        "`create-plan`), Kimi implements and self-reviews.",
    );
  });

  it("treats runs of blank lines as one break and trims indentation", () => {
    const input = "One.\n\n\n  Two\n  continued.\n";
    expect(reflowParagraphs(input)).toBe("One.\n\nTwo continued.");
  });

  it("leaves single-paragraph and single-line text alone", () => {
    expect(reflowParagraphs("Just one line.")).toBe("Just one line.");
    expect(reflowParagraphs("Wrapped\nover two lines.")).toBe(
      "Wrapped over two lines.",
    );
  });

  it("handles empty and whitespace-only input", () => {
    expect(reflowParagraphs("")).toBe("");
    expect(reflowParagraphs("\n\n  \n")).toBe("");
  });
});
