import { describe, expect, it } from "vitest";
import { emptyNode, fromYaml, nodeKind, prebuiltNode, toYaml } from "./workflow-yaml";
import type { EditorWorkflow, PrebuiltStep } from "@/types/authoring";
describe("nodeKind", () => {
  it("derives the kind from the active body field", () => {
    expect(nodeKind({ id: "a", prompt: "x" })).toBe("prompt");
    expect(nodeKind({ id: "a", bash: "x" })).toBe("bash");
    expect(nodeKind({ id: "a", command: "c" })).toBe("command");
    expect(nodeKind({ id: "a", script: "s", runtime: "bun" })).toBe("script");
    expect(
      nodeKind({
        id: "a",
        loop: { prompt: "", until: "D", max_iterations: 1 },
      }),
    ).toBe("loop");
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
        {
          id: "explore",
          provider: "claude",
          model: "sonnet",
          context: "fresh",
          prompt: "look",
        },
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

  it("round-trips cancel, when, category, artifact, and output_format (the idea-to-pr gate)", () => {
    const yaml = `name: gated
nodes:
  - id: validate
    command: validate
    category: validation
    artifact: exploration.md
    output_format:
      type: object
      properties:
        passed: { type: boolean }
  - id: abort-on-invalid
    depends_on: [validate]
    when: "$validate.output.passed != 'true'"
    cancel: "validation failed"
  - id: finalize-pr
    depends_on: [validate]
    when: "$validate.output.passed == 'true'"
    command: finalize-pr
`;
    // Load → save → reload must preserve every field (no silent body loss).
    const back = fromYaml(toYaml(fromYaml(yaml)));
    const validate = back.nodes.find((n) => n.id === "validate")!;
    const abort = back.nodes.find((n) => n.id === "abort-on-invalid")!;
    expect(nodeKind(abort)).toBe("cancel");
    expect(abort.cancel).toBe("validation failed");
    expect(abort.when).toBe("$validate.output.passed != 'true'");
    expect(validate.category).toBe("validation");
    expect(validate.artifact).toBe("exploration.md");
    expect(validate.output_format).toMatchObject({ type: "object" });
    expect(abort.artifact).toBeUndefined();
  });
});

describe("prebuiltNode", () => {
  it("clones the template node with a fresh id and no inbound deps or when", () => {
    const step: PrebuiltStep = {
      id: "validate",
      label: "Validate",
      description: "x",
      node: {
        id: "validate",
        command: "validate",
        category: "validation",
        provider: "pi",
        model: "kimi-code/kimi-for-coding",
        depends_on: ["upstream"],
        when: "$upstream.output.passed == 'true'",
      },
    };
    const node = prebuiltNode(step, "validate-2");
    expect(node.id).toBe("validate-2");
    expect(node.depends_on).toEqual([]);
    expect(node.when).toBeUndefined();
    expect(nodeKind(node)).toBe("command");
    expect(node.command).toBe("validate");
    expect(node.category).toBe("validation");
    // The template object is not mutated.
    expect(step.node.id).toBe("validate");
    expect(step.node.depends_on).toEqual(["upstream"]);
    expect(step.node.when).toBe("$upstream.output.passed == 'true'");
  });
});

it("round-trips artifact through toYaml/fromYaml", () => {
  const wf: EditorWorkflow = {
    name: "demo",
    nodes: [
      {
        id: "explore",
        prompt: "explore",
        artifact: "exploration.md",
      },
      {
        id: "build",
        prompt: "build",
      },
    ],
  };
  const yaml = toYaml(wf);
  expect(yaml).toContain("artifact: exploration.md");
  const back = fromYaml(yaml);
  expect(back.nodes[0].artifact).toBe("exploration.md");
  expect(back.nodes[1].artifact).toBeUndefined();
});

it("round-trips effort through toYaml/fromYaml", () => {
  const wf: EditorWorkflow = {
    name: "demo",
    nodes: [
      { id: "plan", prompt: "plan", effort: "max" },
      { id: "build", prompt: "build" },
    ],
  };
  const yaml = toYaml(wf);
  expect(yaml).toContain("effort: max");
  const back = fromYaml(yaml);
  expect(back.nodes[0].effort).toBe("max");
  expect(back.nodes[1].effort).toBeUndefined();
});
