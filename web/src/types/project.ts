/**
 * Types for the project registry (`/api/projects`). A project scopes runs to a
 * git repo; mirrors the Rust `Project` DTO in `harness-persist`.
 */

export interface Project {
  name: string;
  git_url: string;
  base_branch: string;
  default_workflow: string | null;
  /** Deployed/live site URL; exposed to runs as `$EXTERNAL_URL` (e.g. GEO audit). */
  external_url: string | null;
  /** mise tool specs provisioned before runs (e.g. "rust", "node@22", "pnpm"). */
  toolchains: string[];
  /** Per-project build-cache cap in GiB; `null` falls back to the env default. */
  cargo_target_cap_gb: number | null;
  created_at: string;
  updated_at: string;
}

export interface RegisterProjectRequest {
  name: string;
  git_url: string;
  base_branch?: string;
  default_workflow?: string | null;
  /** Deployed/live site URL; exposed to runs as `$EXTERNAL_URL`. */
  external_url?: string | null;
  toolchains?: string[];
  /** Per-project build-cache cap in GiB; omitted/`null`/≤0 → env default. */
  cargo_target_cap_gb?: number | null;
}

export interface CacheSize {
  bytes: number;
  cap_gb: number;
}
