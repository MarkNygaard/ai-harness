/**
 * What "Set up epics" writes: the bindings an epic needs, from three columns.
 *
 * An epic is a relay of bindings. The build binding sends a finished piece to
 * a merge column, `merge-pr` merges it and moves it on, and the supervisor
 * grades it there and starts the next piece. Each hand-off is one binding's
 * ready column being another's source column, and nothing enforces that, so
 * this writes every side of each hand-off from one choice.
 */
import type { LinearSource, LinearSourceInput } from "@/types/linear";

/** The two workflows an epic adds to a build binding. */
export const MERGE_WORKFLOW = "merge-pr";
export const SUPERVISOR_WORKFLOW = "linear-epic-supervise";
/** Rebuilds a PR from review comments; it hands pieces on like a build does. */
export const REVISE_WORKFLOW = "revise-pr";

/** Bindings that build pieces: everything that is not part of the relay. */
export function isBuilder(s: LinearSource): boolean {
  return ![MERGE_WORKFLOW, SUPERVISOR_WORKFLOW, REVISE_WORKFLOW].includes(
    s.workflow,
  );
}

/** A binding as the PUT body that would save it unchanged. */
export function asInput(s: LinearSource): LinearSourceInput {
  return {
    workflow: s.workflow,
    team_id: s.team_id,
    team_name: s.team_name,
    source_state_id: s.source_state_id,
    failed_label: s.failed_label ?? undefined,
    in_progress_state_id: s.in_progress_state_id ?? undefined,
    review_state_id: s.review_state_id ?? undefined,
    ready_state_id: s.ready_state_id ?? undefined,
    piece_ready_state_id: s.piece_ready_state_id ?? undefined,
    epic_review_state_id: s.epic_review_state_id ?? undefined,
    base_branch: s.base_branch ?? undefined,
    poll_interval_secs: s.poll_interval_secs,
    max_concurrent_runs: s.max_concurrent_runs,
    max_attempts: s.max_attempts,
    enabled: s.enabled,
    live: s.live,
  };
}

export interface EpicColumns {
  /** Where a built piece waits for `merge-pr`. */
  merge: string;
  /** Where a merged piece waits for the supervisor. */
  merged: string;
  /** Where the finished epic goes; empty leaves it where it is. */
  finished: string;
}

/**
 * Why these columns cannot work, or `null` when they can. The three columns of
 * the relay must differ: two bindings claiming from one column fight over it.
 */
export function epicColumnsProblem(
  build: LinearSource,
  columns: EpicColumns,
): string | null {
  if (!columns.merge || !columns.merged) return null;
  const distinct = new Set([
    build.source_state_id,
    columns.merge,
    columns.merged,
  ]);
  return distinct.size < 3
    ? "The build, merge and merged columns must be three different columns."
    : null;
}

/**
 * Every binding write, in order. Existing bindings keep everything except the
 * columns of the relay; missing ones are created like the build binding
 * (enabled and live together), so the relay does not stop at the first
 * binding left in dry-run.
 */
export function epicBindingWrites(
  sources: LinearSource[],
  build: LinearSource,
  columns: EpicColumns,
): LinearSourceInput[] {
  // One binding per workflow per project, so these are the ones that will be
  // overwritten, whichever team they watch now.
  const find = (w: string) => sources.find((s) => s.workflow === w);
  const merge = find(MERGE_WORKFLOW);
  const supervisor = find(SUPERVISOR_WORKFLOW);
  // Only re-pointed when it hands work on within the build binding's team.
  const revise = [find(REVISE_WORKFLOW)].find(
    (r) => r?.team_id === build.team_id,
  );

  const fresh = (workflow: string): LinearSourceInput => ({
    workflow,
    team_id: build.team_id,
    team_name: build.team_name,
    source_state_id: "",
    poll_interval_secs: build.poll_interval_secs,
    max_concurrent_runs: 1,
    max_attempts: 1,
    enabled: build.enabled,
    live: build.live,
  });

  return [
    { ...asInput(build), piece_ready_state_id: columns.merge },
    // A revised PR is a finished piece too, so it goes the same way.
    ...(revise
      ? [{ ...asInput(revise), piece_ready_state_id: columns.merge }]
      : []),
    {
      ...(merge ? asInput(merge) : fresh(MERGE_WORKFLOW)),
      // The relay runs on the build binding's team; its columns are that
      // team's, so a binding moved here from another team moves with them.
      team_id: build.team_id,
      team_name: build.team_name,
      source_state_id: columns.merge,
      ready_state_id: columns.merged,
    },
    {
      ...(supervisor
        ? asInput(supervisor)
        : {
            ...fresh(SUPERVISOR_WORKFLOW),
            // An epic's branch is cut from where its pieces would have gone.
            base_branch: build.base_branch ?? undefined,
          }),
      team_id: build.team_id,
      team_name: build.team_name,
      source_state_id: columns.merged,
      epic_review_state_id: columns.finished || undefined,
    },
  ];
}
