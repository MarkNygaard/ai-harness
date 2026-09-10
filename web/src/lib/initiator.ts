/**
 * Reading the person a run is attributed to.
 *
 * The server writes `trigger_actor` as `Name <email>` — the same shape git uses
 * for an author, and for the same reason: one field that stays readable on its
 * own while still carrying the address. Either half may be missing, so a bare
 * name and a bare address are both valid values here.
 */

/** Where a run came in from, as the API reports it. */
const SOURCE_LABELS: Record<string, string> = {
  ui: "from the dashboard",
  mcp: "over MCP",
  "linear-webhook": "delegated in Linear",
  "linear-poller": "moved into a Linear column",
  // Rows written before the source was split out of `triggered_by`.
  linear: "from Linear",
};

export interface Initiator {
  /** What to show: the name, or the address when that is all there is. */
  label: string;
  /** The address, when the actor string carried one. */
  email: string | null;
  /** One or two letters for the badge. */
  initials: string;
}

/**
 * Split `Name <email>` into its halves. Tolerates a bare name, a bare address,
 * blank input, and a missing closing bracket.
 */
function splitActor(actor: string): { name: string; email: string | null } {
  const open = actor.lastIndexOf("<");
  if (open === -1) {
    // No brackets: it is one or the other. An `@` is the only thing that makes
    // it an address rather than a name.
    const only = actor.trim();
    return only.includes("@")
      ? { name: "", email: only }
      : { name: only, email: null };
  }
  const close = actor.indexOf(">", open);
  const email = actor.slice(open + 1, close === -1 ? undefined : close).trim();
  return { name: actor.slice(0, open).trim(), email: email || null };
}

/**
 * The words to build initials from.
 *
 * For a name that is the words themselves. For an address it is the local part
 * split on its separators, so `mark.nygaard@x.com` gives `MN` rather than the
 * `M` that the address as a whole would.
 */
function initialsSource(name: string, email: string | null): string[] {
  if (name) return name.split(/\s+/).filter(Boolean);
  if (!email) return [];
  const local = email.split("@")[0] ?? "";
  return local.split(/[._\-+]+/).filter(Boolean);
}

/**
 * First letter of the first word, plus first letter of the last when there is
 * more than one. `Array.from` rather than `[0]` so a name starting outside the
 * basic plane is not cut in half.
 */
function lettersFrom(words: string[]): string {
  if (words.length === 0) return "";
  const first = Array.from(words[0])[0] ?? "";
  if (words.length === 1) return first.toUpperCase();
  const last = Array.from(words[words.length - 1])[0] ?? "";
  return (first + last).toUpperCase();
}

/**
 * Read a `trigger_actor` value, or `null` when there is nobody to show.
 *
 * A run with no attribution — everything from before this was recorded, and
 * anything an unauthenticated install started — returns `null` so the views can
 * render nothing rather than a row of empty circles.
 */
export function parseInitiator(
  actor: string | null | undefined,
): Initiator | null {
  if (!actor || !actor.trim()) return null;
  const { name, email } = splitActor(actor);
  const label = name || email;
  if (!label) return null;
  const initials = lettersFrom(initialsSource(name, email));
  // A label made entirely of punctuation yields no letters; it is still a
  // label, so fall back to its first character rather than an empty circle.
  return {
    label,
    email,
    initials: initials || (Array.from(label)[0] ?? "?").toUpperCase(),
  };
}

/** How a run was started, in words. `null` for a source we do not recognise. */
export function sourceLabel(source: string | null | undefined): string | null {
  if (!source) return null;
  return SOURCE_LABELS[source] ?? null;
}

/**
 * The distinct people behind a group of runs, in first-seen order.
 *
 * A dashboard task is several runs — a build, a review, a merge — and usually
 * one person behind all of them, where three identical circles would be three
 * times the noise for no extra fact. Where it is *not* one person (someone
 * delegated the work and someone else asked for the changes) that is worth
 * seeing, so this collapses repeats rather than showing only the first.
 */
export function distinctInitiators(
  runs: {
    trigger_actor: string | null;
    trigger_source: string | null;
  }[],
): { actor: string; source: string | null }[] {
  const seen = new Set<string>();
  const out: { actor: string; source: string | null }[] = [];
  for (const run of runs) {
    const who = parseInitiator(run.trigger_actor);
    if (!who || seen.has(who.label)) continue;
    seen.add(who.label);
    out.push({
      actor: run.trigger_actor as string,
      source: run.trigger_source,
    });
  }
  return out;
}
