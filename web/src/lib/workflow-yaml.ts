/**
 * Round-trip between the flat [`EditorWorkflow`] shape and YAML text via js-yaml.
 *
 * The editor manipulates `EditorWorkflow`/`EditorNode` objects; this module is
 * the single place that (de)serializes them to the YAML the server validates and
 * saves — so what the canvas produces is exactly what gets persisted. Pure.
 */
import yaml from "js-yaml";
import type {
  EditorNode,
  EditorWorkflow,
  McpEntry,
  NodeKindId,
  PrebuiltStep,
  ReportAction,
  ReportLayout,
  ReportStatus,
  WorkflowNav,
  WorkflowReport,
  WorkflowUi,
} from "@/types/authoring";

/** The body field that defines a node's kind (first present wins; default prompt). */
export function nodeKind(node: EditorNode): NodeKindId {
  if (node.bash !== undefined) return "bash";
  if (node.command !== undefined) return "command";
  if (node.script !== undefined) return "script";
  if (node.loop !== undefined) return "loop";
  if (node.approval !== undefined) return "approval";
  if (node.cancel !== undefined) return "cancel";
  if (node.linear !== undefined) return "linear";
  if (node.route !== undefined) return "route";
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
    // Status is the common case by a distance -- it is what the binding's
    // In-progress/Review/Ready map used to do -- so a fresh Linear step starts
    // there rather than making you pick an action before you can type.
    case "linear":
      return { ...base, linear: { move_state: "" } };
    // Two routes is the least a route can have; the second is the default so
    // the step leads somewhere before any rule or prompt is written.
    case "route":
      return {
        ...base,
        route: {
          prompt: "",
          routes: [{ name: "a" }, { name: "b" }],
          default: "b",
        },
      };
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

/**
 * A node's `mcp:` list as written: bare names and `{ server, tools }` objects
 * both pass through untouched, so a save from the canvas changes neither.
 * Anything else (a number, a nameless object) is something the server would
 * refuse anyway, and is left out.
 */
function mcpFromDoc(raw: unknown): McpEntry[] | undefined {
  if (!Array.isArray(raw)) return undefined;
  return raw.filter(
    (e): e is McpEntry =>
      typeof e === "string" ||
      (typeof e === "object" &&
        e !== null &&
        typeof (e as { server?: unknown }).server === "string"),
  );
}

/** Serialize the `ui` block to a terse doc, or undefined when it declares
 *  nothing — a nav/report entry needs a label, so empty ones are omitted. */
function uiToDoc(
  ui: WorkflowUi | null | undefined,
): Record<string, unknown> | undefined {
  if (!ui) return undefined;
  const nav =
    ui.nav && ui.nav.label
      ? clean({ label: ui.nav.label, icon: ui.nav.icon })
      : undefined;
  const report =
    ui.report && ui.report.label
      ? clean({
          label: ui.report.label,
          verdict_node: ui.report.verdict_node,
          scored: ui.report.scored,
          // `none` is the implicit default — omit it rather than writing it out.
          status:
            ui.report.status && ui.report.status !== "none"
              ? ui.report.status
              : undefined,
          actions: ui.report.actions,
          // Likewise `list`.
          layout:
            ui.report.layout && ui.report.layout !== "list"
              ? ui.report.layout
              : undefined,
        })
      : undefined;
  const out = clean({ nav, report });
  return Object.keys(out).length ? out : undefined;
}

/** Parse the `ui` block from raw YAML into the structured editor shape. */
function uiFromRaw(raw: unknown): WorkflowUi | undefined {
  if (!raw || typeof raw !== "object") return undefined;
  const r = raw as Record<string, unknown>;
  const navRaw = r.nav as Record<string, unknown> | undefined;
  const nav: WorkflowNav | null = navRaw
    ? {
        label: String(navRaw.label ?? ""),
        icon: (navRaw.icon as string | undefined) ?? null,
      }
    : null;
  const repRaw = r.report as Record<string, unknown> | undefined;
  const report: WorkflowReport | null = repRaw
    ? {
        label: String(repRaw.label ?? ""),
        verdict_node: (repRaw.verdict_node as string | undefined) ?? null,
        scored: Boolean(repRaw.scored),
        actions: Array.isArray(repRaw.actions)
          ? (repRaw.actions as ReportAction[])
          : undefined,
        status: repRaw.status as ReportStatus | undefined,
        layout: repRaw.layout as ReportLayout | undefined,
      }
    : null;
  if (!nav && !report) return undefined;
  return { nav, report };
}

/** One node as it is written to YAML: its single body + set options, terse.
 *  Also what "Save as step" stores, so a saved step is exactly what a
 *  workflow file would hold. */
export function nodeToDoc(n: EditorNode): Record<string, unknown> {
  const loop = n.loop
    ? clean(n.loop as unknown as Record<string, unknown>)
    : undefined;
  const approval = n.approval
    ? clean(n.approval as unknown as Record<string, unknown>)
    : undefined;
  // `clean` drops empty values, so an action still being typed (`move_state:
  // ""`) would otherwise serialize as a bare `linear: {}` and come back as a
  // prompt node, silently changing the step's kind under the author.
  const linear = n.linear
    ? (n.linear as unknown as Record<string, unknown>)
    : undefined;
  const route = n.route ? routeToDoc(n.route) : undefined;
  return clean({
    id: n.id,
    depends_on: n.depends_on,
    when: n.when,
    provider: n.provider,
    model: n.model,
    effort: n.effort,
    context: n.context,
    category: n.category,
    artifact: n.artifact,
    trigger_rule: n.trigger_rule,
    timeout: n.timeout,
    output_format: n.output_format,
    // Fields the editor has no controls for yet: carried through so a
    // save from the canvas doesn't delete what was written in YAML.
    retries: n.retries,
    hooks: n.hooks,
    mcp: n.mcp,
    // `false` is the default, so only `true` is written.
    linear_comment: n.linear_comment || undefined,
    prompt: n.prompt,
    bash: n.bash,
    command: n.command,
    script: n.script,
    runtime: n.runtime,
    deps: n.deps,
    loop,
    approval,
    cancel: n.cancel,
    linear,
    route,
  });
}

/** The `route:` body, terse: each route keeps its name even while blank. */
function routeToDoc(
  r: NonNullable<EditorNode["route"]>,
): Record<string, unknown> {
  return {
    ...clean({ prompt: r.prompt }),
    routes: r.routes.map((o) => ({
      name: o.name,
      ...clean({ description: o.description, when: o.when }),
    })),
    ...clean({ default: r.default }),
  };
}

/** Serialize a workflow to YAML, keeping each node's single body + set options. */
export function toYaml(wf: EditorWorkflow): string {
  const doc = clean({
    name: wf.name,
    description: wf.description,
    provider: wf.provider,
    model: wf.model,
    nodes: wf.nodes.map(nodeToDoc),
    ui: uiToDoc(wf.ui),
    improvements: wf.improvements?.every
      ? { every: wf.improvements.every }
      : undefined,
    // `false` is the default, so only `true` is written.
    handles_epics: wf.handles_epics || undefined,
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
    effort: n.effort as EditorNode["effort"],
    context: n.context as EditorNode["context"],
    trigger_rule: n.trigger_rule as EditorNode["trigger_rule"],
    timeout: typeof n.timeout === "number" ? n.timeout : undefined,
    category: n.category as string | undefined,
    artifact: n.artifact as string | undefined,
    output_format: n.output_format,
    retries: typeof n.retries === "number" ? n.retries : undefined,
    hooks: n.hooks ?? undefined,
    mcp: mcpFromDoc(n.mcp),
    linear_comment: n.linear_comment === true ? true : undefined,
    prompt: n.prompt as string | undefined,
    bash: n.bash as string | undefined,
    command: n.command as string | undefined,
    script: n.script as string | undefined,
    runtime: n.runtime as EditorNode["runtime"],
    deps: Array.isArray(n.deps) ? (n.deps as string[]) : undefined,
    loop: n.loop as EditorNode["loop"],
    approval: n.approval as EditorNode["approval"],
    cancel: n.cancel as string | undefined,
    linear: n.linear as EditorNode["linear"],
    route: n.route as EditorNode["route"],
  }));
  return {
    name: String(raw.name ?? "untitled"),
    description: raw.description as string | undefined,
    provider: raw.provider as string | undefined,
    model: raw.model as string | undefined,
    nodes,
    ui: uiFromRaw(raw.ui),
    improvements: improvementsFromRaw(raw.improvements),
    handles_epics: raw.handles_epics === true ? true : undefined,
  };
}

function improvementsFromRaw(raw: unknown): EditorWorkflow["improvements"] {
  if (!raw || typeof raw !== "object") return undefined;
  const every = (raw as { every?: unknown }).every;
  return typeof every === "number" ? { every } : undefined;
}
