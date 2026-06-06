/**
 * Round-trip between the flat [`EditorWorkflow`] shape and YAML text via js-yaml.
 *
 * The editor manipulates `EditorWorkflow`/`EditorNode` objects; this module is
 * the single place that (de)serializes them to the YAML the server validates and
 * saves — so what the canvas produces is exactly what gets persisted. Pure.
 */
import yaml from "js-yaml";
import type { EditorNode, EditorWorkflow, NodeKindId, PrebuiltStep } from "@/types/authoring";

/** The body field that defines a node's kind (first present wins; default prompt). */
export function nodeKind(node: EditorNode): NodeKindId {
  if (node.bash !== undefined) return "bash";
  if (node.command !== undefined) return "command";
  if (node.script !== undefined) return "script";
  if (node.loop !== undefined) return "loop";
  if (node.approval !== undefined) return "approval";
  if (node.cancel !== undefined) return "cancel";
  return "prompt";
}

/** A fresh node of a given kind with sensible defaults. */
export function emptyNode(kind: NodeKindId, id: string): EditorNode {
  const base: EditorNode = { id, depends_on: [] };
  switch (kind) {
    case "bash":
      return { ...base, bash: 'echo "hello"' };
    case "command":
      return { ...base, command: "" };
    case "script":
      return { ...base, script: "console.log('hi')", runtime: "bun" };
    case "loop":
      return {
        ...base,
        loop: { prompt: "", until: "DONE", max_iterations: 3 },
      };
    case "approval":
      return { ...base, approval: { message: "" } };
    case "cancel":
      return { ...base, cancel: "" };
    default:
      return { ...base, prompt: "" };
  }
}
/** Clone a prebuilt step's node spec with a fresh id and no inbound deps or
 *  conditional wiring (canvas edges and the user define those). Mirrors `emptyNode`. */
export function prebuiltNode(step: PrebuiltStep, id: string): EditorNode {
  const { when: _when, ...rest } = step.node;
  return { ...rest, id, depends_on: [] };
}

/** Drop undefined/null, empty strings, and empty arrays so the YAML stays terse. */
function clean<T extends Record<string, unknown>>(
  obj: T,
): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(obj)) {
    if (v === undefined || v === null) continue;
    if (typeof v === "string" && v === "") continue;
    if (Array.isArray(v) && v.length === 0) continue;
    out[k] = v;
  }
  return out;
}

/** Serialize a workflow to YAML, keeping each node's single body + set options. */
export function toYaml(wf: EditorWorkflow): string {
  const doc = clean({
    name: wf.name,
    description: wf.description,
    provider: wf.provider,
    model: wf.model,
    nodes: wf.nodes.map((n) => {
      const loop = n.loop
        ? clean(n.loop as unknown as Record<string, unknown>)
        : undefined;
      const approval = n.approval
        ? clean(n.approval as unknown as Record<string, unknown>)
        : undefined;
      return clean({
        id: n.id,
        depends_on: n.depends_on,
        when: n.when,
        provider: n.provider,
        model: n.model,
        context: n.context,
        category: n.category,
        trigger_rule: n.trigger_rule,
        timeout: n.timeout,
        output_format: n.output_format,
        prompt: n.prompt,
        bash: n.bash,
        command: n.command,
        script: n.script,
        runtime: n.runtime,
        deps: n.deps,
        loop,
        approval,
        cancel: n.cancel,
      });
    }),
  });
  return yaml.dump(doc, { lineWidth: 100, noRefs: true });
}

/** Parse YAML into the flat editor shape (tolerant of missing optional fields). */
export function fromYaml(text: string): EditorWorkflow {
  const raw = (yaml.load(text) ?? {}) as Record<string, unknown>;
  const rawNodes = Array.isArray(raw.nodes)
    ? (raw.nodes as Record<string, unknown>[])
    : [];
  const nodes: EditorNode[] = rawNodes.map((n) => ({
    id: String(n.id ?? ""),
    depends_on: Array.isArray(n.depends_on) ? (n.depends_on as string[]) : [],
    when: n.when as string | undefined,
    provider: n.provider as string | undefined,
    model: n.model as string | undefined,
    context: n.context as EditorNode["context"],
    trigger_rule: n.trigger_rule as EditorNode["trigger_rule"],
    timeout: typeof n.timeout === "number" ? n.timeout : undefined,
    category: n.category as string | undefined,
    output_format: n.output_format,
    prompt: n.prompt as string | undefined,
    bash: n.bash as string | undefined,
    command: n.command as string | undefined,
    script: n.script as string | undefined,
    runtime: n.runtime as EditorNode["runtime"],
    deps: Array.isArray(n.deps) ? (n.deps as string[]) : undefined,
    loop: n.loop as EditorNode["loop"],
    approval: n.approval as EditorNode["approval"],
    cancel: n.cancel as string | undefined,
  }));
  return {
    name: String(raw.name ?? "untitled"),
    description: raw.description as string | undefined,
    provider: raw.provider as string | undefined,
    model: raw.model as string | undefined,
    nodes,
  };
}
