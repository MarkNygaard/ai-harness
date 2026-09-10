import { describe, expect, it } from "vitest";

import { optionLabels } from "./PropertiesDrawer";
import { SelectItem } from "@/components/ui/select";

/**
 * Base UI's `Select.Value` renders the raw value unless it is given a
 * formatter, so every field whose label differs from its value used to display
 * the value: the agent read `claude` after picking "Claude Code", the type read
 * `prompt` rather than "Agent step", and an inherited setting read the
 * `__default__` sentinel. The labels are derived from the options themselves so
 * a new field cannot be added with the bug still in it.
 */
describe("optionLabels", () => {
  it("reads the label off each option", () => {
    const labels = optionLabels([
      <SelectItem key="claude" value="claude">
        Claude Code
      </SelectItem>,
      <SelectItem key="codex" value="codex">
        Codex
      </SelectItem>,
    ]);
    expect(labels).toEqual({ claude: "Claude Code", codex: "Codex" });
  });

  /** Options built by `.map()` arrive as a nested array, not as siblings. */
  it("looks inside a mapped list", () => {
    const labels = optionLabels([
      <SelectItem key="__default__" value="__default__">
        Workflow default (Claude Code)
      </SelectItem>,
      ["prompt", "bash"].map((k) => (
        <SelectItem key={k} value={k}>
          {k === "prompt" ? "Agent step" : "Shell"}
        </SelectItem>
      )),
    ]);
    expect(labels).toEqual({
      __default__: "Workflow default (Claude Code)",
      prompt: "Agent step",
      bash: "Shell",
    });
  });

  /**
   * A label that is not plain text has nothing to reuse, so it is left out and
   * the trigger falls back to the value — which is worse than the label and far
   * better than blank.
   */
  it("skips what it cannot read, rather than guessing", () => {
    const labels = optionLabels([
      <SelectItem key="a" value="a">
        <strong>A</strong>
      </SelectItem>,
      "a bare string child",
      null,
      undefined,
    ]);
    expect(labels).toEqual({});
  });

  it("is empty for no options at all", () => {
    expect(optionLabels(null)).toEqual({});
    expect(optionLabels([])).toEqual({});
  });
});
