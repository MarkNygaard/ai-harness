import { describe, expect, it } from "vitest";
import { emptyNode, fromYaml, nodeKind, toYaml } from "./workflow-yaml";
import type { EditorWorkflow } from "@/types/authoring";

describe("nodeKind", () => {
  it("derives the kind from the active body field", () => {
    expect(nodeKind({ id: "a", prompt: "x" })).toBe("prompt");
    expect(nodeKind({ id: "a", bash: "x" })).toBe("bash");
    expect(nodeKind({ id: "a", command: "c" })).toBe("command");
    expect(nodeKind({ id: "a", script: "s", runtime: "bun" })).toBe("script");
    expect(nodeKind({ id: "a", loop: { prompt: "", until: "D", max_iterations: 1 } })).toBe("loop");
    expect(nodeKind({ id: "a" })).toBe("prompt"); // default
  });
});

describe("emptyNode", () => {
  it("creates a node with the right body for each kind", () => {
    expect(nodeKind(emptyNode("bash", "x"))).toBe("bash");
    expect(nodeKind(emptyNode("loop", "x"))).toBe("loop");
    expect(emptyNode("script", "x").runtime).toBe("bun");
    expect(emptyNode("loop", "x").loop?.max_iterations).toBe(3);
  });
});

describe("toYaml / fromYaml round-trip", () => {
  it("preserves workflow + node fields and drops empty ones", () => {
    const wf: EditorWorkflow = {
      name: "demo",
      provider: "pi",
      model: "kimi-coding/kimi-for-coding",
      nodes: [
        { id: "explore", provider: "claude", model: "sonnet", context: "fresh", prompt: "look" },
        {
          id: "review",
          depends_on: ["explore"],
          trigger_rule: "one_success",
          loop: {
            prompt: "review",
            until: "CLEAN",
            max_iterations: 5,
            provider: "pi",
            model: "kimi-coding/kimi-for-coding",
          },
        },
      ],
    };
    const yaml = toYaml(wf);
    expect(yaml).toContain("name: demo");
    expect(yaml).not.toContain("description"); // empty/undefined dropped

    const back = fromYaml(yaml);
    expect(back.name).toBe("demo");
    expect(back.provider).toBe("pi");
    expect(back.nodes).toHaveLength(2);
    expect(back.nodes[0].prompt).toBe("look");
    expect(back.nodes[0].context).toBe("fresh");
    expect(back.nodes[1].depends_on).toEqual(["explore"]);
    expect(back.nodes[1].trigger_rule).toBe("one_success");
    expect(back.nodes[1].loop?.max_iterations).toBe(5);
    expect(back.nodes[1].loop?.provider).toBe("pi");
  });

  it("tolerates a minimal / empty document", () => {
    const wf = fromYaml("name: tiny\nnodes: []\n");
    expect(wf.name).toBe("tiny");
    expect(wf.nodes).toEqual([]);
    expect(fromYaml("").name).toBe("untitled");
  });
});
