/**
 * Types mirroring the Rust Linear discovery DTOs and the persisted
 * `harness_linear_sources` row.
 */

export interface LinearLabel {
  id: string;
  name: string;
}

export interface LinearState {
  id: string;
  name: string;
  kind: string;
  position: number;
}

export interface LinearTeam {
  id: string;
  name: string;
  key: string;
  states: LinearState[];
  labels: LinearLabel[];
}

export interface LinearDiscovery {
  teams: LinearTeam[];
}

/** A persisted Linear trigger binding (matches `harness_linear_sources`). */
export interface LinearSource {
  project: string;
  workflow: string;
  team_id: string;
  team_name: string;
  source_state_id: string;
  label: string | null;
  /** Label applied on give-up; while present, excludes the issue from pickup. */
  failed_label: string | null;
  in_progress_state_id: string | null;
  review_state_id: string | null;
  ready_state_id: string | null;
  base_branch: string | null;
  poll_interval_secs: number;
  /** How many runs this binding may have in flight at once (default 1). */
  max_concurrent_runs: number;
  /** How many times an issue is (re-)fired before the poller gives up (default 1). */
  max_attempts: number;
  enabled: boolean;
  live: boolean;
  created_at: string;
  updated_at: string;
}

/** A Linear issue created via the harness (the fields surfaced back). */
export interface CreatedLinearIssue {
  id: string;
  identifier: string;
  url: string;
}

/** Body for creating a Linear issue from a task/finding. */
export interface CreateLinearIssueInput {
  /** Binding to file against; defaults to `idea-to-pr` server-side. */
  workflow?: string;
  title: string;
  description: string;
}

/** Fields accepted when saving a Linear source binding. */
export interface LinearSourceInput {
  workflow: string;
  team_id: string;
  team_name: string;
  source_state_id: string;
  label?: string;
  failed_label?: string;
  in_progress_state_id?: string;
  review_state_id?: string;
  ready_state_id?: string;
  base_branch?: string;
  poll_interval_secs: number;
  max_concurrent_runs: number;
  max_attempts: number;
  enabled: boolean;
  live: boolean;
}
