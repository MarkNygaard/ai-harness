/**
 * Types for the control-plane **runs** API (harness-dag execution model).
 * Mirrors the Rust DTOs in `harness-persist` and the `RunEvent` enum in
 * `harness-dag` (serde `tag = "type"`, snake_case).
 */

/**
 * Terminal node status, plus two derived live states (`pending` = declared but
 * not yet started, `running` = executing) that the graph uses before a node is
 * persisted. Neither live state is ever stored.
 */
export type NodeStatus =
  | "pending"
  | "running"
  | "success"
  | "failed"
  | "skipped"
  | "cancelled";
/** Terminal run status (+ derived live "running"). */
export type RunStatus = "running" | "completed" | "failed" | "cancelled";

/** Per-invocation token usage; counters a provider doesn't report are null. */
export interface Usage {
  input: number | null;
  output: number | null;
  cache_read: number | null;
  cache_write: number | null;
}

/** Static DAG topology entry: a node id and what it depends on. */
export interface NodeMeta {
  id: string;
  depends_on: string[];
  /** Optional category id for overview grouping/colouring. */
  category?: string | null;
  /** Declared artifact path (relative to the run's artifacts dir). */
  artifact?: string | null;
}

/** A run row for the list view (matches `RunSummary`). */
export interface RunSummary {
  id: string;
  workflow_name: string;
  /** Human task title (the trigger title); null for older/CLI runs. */
  title: string | null;
  /** The task spec; null in list responses and for older/CLI runs. */
  description: string | null;
  status: RunStatus;
  project: string | null;
  node_count: number;
  recorded_at: string;
  /** Earliest node start across the run (ISO); null when no node has timing. */
  started_at: string | null;
  /** Latest node end across the run (ISO); null when no node has finished. */
  ended_at: string | null;
  /** A/B pairing: shared id linking the two arms; null for a normal run. */
  ab_pair_id: string | null;
  /** Which arm of the pair this run is — "a" or "b"; null if not paired. */
  ab_arm: string | null;
  /** Display label for the arm's substituted model (e.g. "cursor/composer-2.5"). */
  ab_label: string | null;
}
/** One (project, day, status) tally from the runs summary endpoint. */
export interface RunDailyCount {
  project: string | null;
  day: string; // ISO timestamp at UTC midnight
  status: "completed" | "failed" | "cancelled";
  count: number;
}

/** A persisted per-node row (matches `PersistedNode`). */
export interface PersistedNode {
  node_id: string;
  ordinal: number;
  status: NodeStatus;
  provider: string | null;
  model: string | null;
  output: string;
  iterations: number;
  converged: boolean | null;
  note: string | null;
  input_tokens: number | null;
  output_tokens: number | null;
  cache_read: number | null;
  cache_write: number | null;
  started_at: string | null;
  ended_at: string | null;
  artifact_content: string | null;
}

/** A run plus its task spec, node rows, and topology (matches `RunDetail`). */
export interface RunDetail extends RunSummary {
  nodes: PersistedNode[];
  graph: NodeMeta[];
}

/** Per-node execution record carried in `node_finished` events (`NodeRun`). */
export interface NodeRun {
  id: string;
  status: NodeStatus;
  provider: string | null;
  model: string | null;
  output: string;
  usage: Usage;
  iterations: number;
  converged: boolean | null;
  note: string | null;
  artifact_content?: string | null;
  started_at?: string | null;
  ended_at?: string | null;
}

/** Live run events (SSE). Discriminated on `type`. */
export type RunEvent =
  | {
      type: "run_started";
      workflow: string;
      total_nodes: number;
      nodes: NodeMeta[];
    }
  | {
      type: "node_started";
      node_id: string;
      provider: string | null;
      model: string | null;
    }
  | { type: "node_finished"; node: NodeRun }
  | { type: "node_progress"; node_id: string; activity: string }
  | { type: "run_finished"; status: RunStatus };

export interface CreateRunRequest {
  workflow: string;
  /** Names the task (exposed to nodes as `$TASK_TITLE`). */
  title?: string;
  /** The task spec — `$ARGUMENTS` / `$USER_MESSAGE` / `$TASK_DESCRIPTION`. */
  description?: string;
  real?: boolean;
  base_branch?: string | null;
  /** Project to run within; its repo checkout becomes the workspace. */
  project?: string | null;
}

export interface CreateRunResponse {
  run_id: string;
}

/** A provider+model reference (matches the server `ModelRef`). */
export interface ModelRef {
  provider: string;
  model: string;
}

/**
 * Trigger an A/B pair: two runs of one task where the `swap_from` steps use
 * `variant_a` (arm A) vs `variant_b` (arm B). Matches the server
 * `CreateRunPairRequest`.
 */
export interface CreateRunPairRequest {
  workflow: string;
  title?: string;
  description?: string;
  real?: boolean;
  base_branch?: string | null;
  project?: string | null;
  swap_from: ModelRef;
  variant_a: ModelRef;
  variant_b: ModelRef;
}

export interface CreateRunPairResponse {
  pair_id: string;
  run_id_a: string;
  run_id_b: string;
}

/** A pairwise quality verdict from the `judge-ab` workflow. */
export interface AbVerdict {
  winner: "a" | "b" | "tie";
  score_a: number;
  score_b: number;
  reasoning: string;
  /** % of arm A's final change produced by the shared late reviewers (gpt/sonnet). */
  review_share_a?: number;
  /** % of arm B's final change produced by the shared late reviewers. */
  review_share_b?: number;
  /** Whether the cheap implementer carried the work, or the reviewers rescued it. */
  review_assessment?: string;
  /** Arm A: how completely/correctly the implementer fulfilled its plan (0–100). */
  plan_fidelity_a?: number;
  /** Arm B: how completely/correctly the implementer fulfilled its plan (0–100). */
  plan_fidelity_b?: number;
  /** Whether each implementer executed its plan faithfully, or left gaps. */
  plan_assessment?: string;
}

/** The judge run for a pair: its run id, status, and parsed verdict (if done). */
export interface AbJudge {
  run_id: string;
  status: RunStatus | string;
  verdict: AbVerdict | null;
}

/** Both arms of an A/B pair (`GET /api/runs/pair/{id}`), ordered a → b. */
export interface RunPairResponse {
  pair_id: string;
  runs: RunDetail[];
  /** The quality judgement, once one has been requested; null otherwise. */
  judge: AbJudge | null;
}

/** `POST /api/runs/pair/{id}/judge` body — optional judge-model override. */
export interface JudgePairRequest {
  judge_model?: ModelRef;
}

export interface JudgePairResponse {
  judge_run_id: string;
}

/**
 * A unified, render-ready view of a single node: topology (depends_on) merged
 * with the latest known status, timing, provider/model and token usage. Built
 * by the graph from either a persisted `RunDetail` or accumulated live events.
 */
export interface NodeView {
  id: string;
  depends_on: string[];
  status: NodeStatus;
  provider: string | null;
  model: string | null;
  iterations: number;
  usage: Usage;
  note: string | null;
  output: string;
  started_at: string | null;
  ended_at: string | null;
  /** Category id (from the DAG topology), for overview grouping/colouring. */
  category: string | null;
  artifact: string | null;
  artifact_content: string | null;
  /** Live-only latest activity line shown while the node is running (not
   * persisted; cleared when the node starts or finishes). */
  activity: string | null;
  /** Live-only accumulated activity lines (sampled, deduped, capped) shown as
   * a feed in the inspect dialog while the node runs. Not persisted; cleared
   * when the node starts or finishes. */
  activityLog: string[];
  /** Live-only task progress (e.g. 5 of 13) parsed from the implement agent's
   * `📋 n/N` markers; sticky across activity updates so it persists between
   * markers, cleared when the node starts or finishes. Null when the step
   * reports no task count. */
  taskProgress: { done: number; total: number } | null;
}
