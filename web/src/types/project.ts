/**
 * Types for the project registry (`/api/projects`). A project scopes runs to a
 * git repo; mirrors the Rust `Project` DTO in `harness-persist`.
 */

export interface Project {
  name: string;
  git_url: string;
  base_branch: string;
  default_workflow: string | null;
  /** mise tool specs provisioned before runs (e.g. "rust", "node@22", "pnpm"). */
  toolchains: string[];
  created_at: string;
  updated_at: string;
}

export interface RegisterProjectRequest {
  name: string;
  git_url: string;
  base_branch?: string;
  default_workflow?: string | null;
  toolchains?: string[];
}
