/**
 * How long ago something went into a bin, and how long it has left there.
 * Shared by the workflow bin and the projects/bindings bin. Times are unix
 * seconds.
 */

const DAY = 86_400;

/** "today", "yesterday", "3 days ago" — a bin entry's age, in days. */
export function daysAgo(then: number, now: number): string {
  const days = Math.floor((now - then) / DAY);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  return `${days} days ago`;
}

/** "in 1 day", "in 12 days", "today" — until a bin entry is cleared out. */
export function clearedIn(expires: number, now: number): string {
  const days = Math.ceil((expires - now) / DAY);
  if (days <= 0) return "today";
  return days === 1 ? "in 1 day" : `in ${days} days`;
}
