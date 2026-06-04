/**
 * Types for the project registry (`/api/projects`). A project scopes runs to a
 * git repo; mirrors the Rust `Project` DTO in `harness-persist`.
 */

export interface Project {
  name: string;
  git_url: string;
  base_branch: string;
  default_workflow: string | null;
  created_at: string;
  updated_at: string;
}

export interface RegisterProjectRequest {
  name: string;
  git_url: string;
  base_branch?: string;
  default_workflow?: string | null;
}
