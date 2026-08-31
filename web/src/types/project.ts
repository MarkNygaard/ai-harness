/**
 * Types for the project registry (`/api/projects`). A project scopes runs to a
 * git repo; mirrors the Rust `Project` DTO in `harness-persist`.
 */

/**
 * One repo in a multi-repo project. The harness makes no stack assumption — the
 * agent inspects the checkout to learn its language/framework. `role` is a
 * free-text hint, not a fixed taxonomy.
 */
export interface ProjectRepo {
  url: string;
  base_branch: string;
  /** Subdirectory in the run workspace this repo is checked out into. */
  folder: string;
  role?: string;
}

export interface Project {
  name: string;
  git_url: string;
  base_branch: string;
  default_workflow: string | null;
  /** Deployed/live site URL; exposed to runs as `$EXTERNAL_URL` (e.g. GEO audit). */
  external_url: string | null;
  /** mise tool specs provisioned before runs (e.g. "rust", "node@22", "pnpm"). */
  toolchains: string[];
  /** Extra repos for a multi-repo project; empty = single-repo (`git_url`). */
  repos: ProjectRepo[];
  /** Per-project build-cache cap in GiB; `null` falls back to the env default. */
  cargo_target_cap_gb: number | null;
  /**
   * Which Linear account this project's issues come from. `null` = not pinned,
   * which resolves to the sole connected account — so a single-account install
   * never sets it.
   */
  linear_connection: string | null;
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
  /** Extra repos for a multi-repo project; empty/omitted = single-repo. */
  repos?: ProjectRepo[];
  /** Per-project build-cache cap in GiB; omitted/`null`/≤0 → env default. */
  cargo_target_cap_gb?: number | null;
}

export interface CacheSize {
  /** This project's Rust build cache (`CARGO_TARGET_DIR`). */
  bytes: number;
  cap_gb: number;
  /** Package-manager downloads (pnpm store, NuGet, …) — shared by all projects. */
  deps_bytes: number;
  deps_cap_gb: number;
  /** Bare git mirrors runs clone from — shared by all projects. */
  git_bytes: number;
}
