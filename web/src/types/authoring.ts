/**
 * Types for the workflow **authoring** API (`/api/authoring/*`), mirroring the
 * Rust DTOs in `harness-runner::authoring`.
 */

export type AuthoringSource = "bundled" | "project";

/** A left-nav entry a workflow declares (shown once it has ≥1 run). */
export interface WorkflowNav {
  label: string;
  /** Icon key from the curated allow-list (see ICONS in AppSidebar); falls back
   * to a default when unset/unknown. */
  icon: string | null;
}

/** An opt-in per-finding action a report offers. */
export type ReportAction = "build" | "issue" | "ignore";

/** The per-item status control shown on each finding. */
export type ReportStatus = "none" | "check" | "pass_fail";

/** A findings/report tab a workflow declares on its runs. */
export interface WorkflowReport {
  label: string;
  /** Node id whose JSON output holds the verdict; when null the UI scans nodes. */
  verdict_node: string | null;
  /** Show a score gauge + history sparkline (GEO-style) vs. findings-only. */
  scored: boolean;
  /** Opt-in per-finding buttons; absent/empty → a clean read-only list. */
  actions?: ReportAction[];
  /** Per-item status control; absent → `none`. */
  status?: ReportStatus;
}

/** Optional UI surfaces a workflow opts into (mirrors `harness_dag::WorkflowUi`). */
export interface WorkflowUi {
  nav: WorkflowNav | null;
  report: WorkflowReport | null;
}

export interface WorkflowSummary {
  name: string;
  source: AuthoringSource;
  description: string | null;
  node_count: number;
  /** Declarative nav/report surfaces; absent for workflows that opt out. */
  ui?: WorkflowUi | null;
}

export interface WorkflowSource {
  name: string;
  source: AuthoringSource;
  yaml: string;
  /** A bundled default of this name exists → this workflow can be reset to it
   *  (true for bundled workflows + their project overrides; false for custom). */
  has_bundled_default: boolean;
}

export interface NodeSummary {
  id: string;
  kind: string;
  depends_on: string[];
}

export interface ValidationResult {
  valid: boolean;
  error: string | null;
  nodes: NodeSummary[];
}

export interface NodeKindInfo {
  kind: string;
  label: string;
  description: string;
  ai: boolean;
}

export interface ProviderInfo {
  id: string;
  label: string;
  models: string[];
}

export interface CommandInfo {
  name: string;
  source: AuthoringSource;
}
export interface PrebuiltStep {
  id: string;
  label: string;
  description: string;
  node: EditorNode;
}

export interface Catalog {
  node_kinds: NodeKindInfo[];
  providers: ProviderInfo[];
  commands: CommandInfo[];
  context_modes: string[];
  trigger_rules: string[];
  prebuilt_steps: PrebuiltStep[];
}

/** The node-kind discriminators (the single body each editor node carries). */
export type NodeKindId =
  "prompt" | "command" | "bash" | "loop" | "script" | "approval" | "cancel";

export type ContextMode = "fresh" | "shared";
export type TriggerRule =
  "all_success" | "one_success" | "none_failed_min_one_success" | "all_done";
export type ScriptRuntime = "bun" | "uv";
/** Reasoning-effort override for AI bodies, forwarded as `--effort` to
 * claude/codex CLIs. Higher effort suits high-leverage steps like planning. */
export type EffortLevel = "low" | "medium" | "high" | "xhigh" | "max";

/** Loop body config (mirrors `LoopConfig`, editor-relevant fields). */
export interface EditorLoop {
  prompt: string;
  until: string;
  max_iterations: number;
  provider?: string;
  model?: string;
}

/** Approval body config (mirrors `ApprovalConfig`, editor-relevant fields). */
export interface EditorApproval {
  message: string;
  capture_response?: boolean;
  on_reject?: string;
}

/**
 * The flat, editor-facing node shape — mirrors the YAML node the parser accepts
 * (`RawNode`): an id, edges, AI options, and exactly one body field. The active
 * body field determines the node's kind.
 */
export interface EditorNode {
  id: string;
  depends_on?: string[];
  provider?: string;
  model?: string;
  /** Reasoning-effort override (claude/codex only); undefined → agent default. */
  effort?: EffortLevel;
  context?: ContextMode;
  trigger_rule?: TriggerRule;
  timeout?: number;
  /** Category id for overview grouping/colouring (from the categories registry). */
  category?: string;
  /** Artifact file this node produces (relative to the artifacts dir). */
  artifact?: string;
  /** Conditional-execution expression evaluated after trigger_rule. */
  when?: string;
  /** JSON schema the AI body's output should match (prompt/command nodes). */
  output_format?: unknown;
  // Mutually exclusive bodies:
  prompt?: string;
  bash?: string;
  command?: string;
  script?: string;
  runtime?: ScriptRuntime;
  /** Script dependencies (uv only). */
  deps?: string[];
  loop?: EditorLoop;
  approval?: EditorApproval;
  cancel?: string;
}

/** A whole workflow in the flat editor shape (round-trips to YAML via js-yaml). */
export interface EditorWorkflow {
  name: string;
  description?: string;
  provider?: string;
  model?: string;
  nodes: EditorNode[];
}
