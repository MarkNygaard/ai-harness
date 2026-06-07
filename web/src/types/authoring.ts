/**
 * Types for the workflow **authoring** API (`/api/authoring/*`), mirroring the
 * Rust DTOs in `harness-runner::authoring`.
 */

export type AuthoringSource = "bundled" | "project";

export interface WorkflowSummary {
  name: string;
  source: AuthoringSource;
  description: string | null;
  node_count: number;
}

export interface WorkflowSource {
  name: string;
  source: AuthoringSource;
  yaml: string;
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
  | "prompt"
  | "command"
  | "bash"
  | "loop"
  | "script"
  | "approval"
  | "cancel";

export type ContextMode = "fresh" | "shared";
export type TriggerRule =
  | "all_success"
  | "one_success"
  | "none_failed_min_one_success"
  | "all_done";
export type ScriptRuntime = "bun" | "uv";

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
