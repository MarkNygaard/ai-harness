/**
 * The agents that execute workflow nodes, and which of them the container is
 * running an out-of-date CLI for.
 *
 * The list moved here from the Agents page because two things now need the
 * `provider:` → human-label mapping: that page, and the notice that tells an
 * administrator an update is waiting on it.
 */
import type { ProviderHealth } from "./system";

/**
 * What a workflow node can pick, and what backs it.
 *
 * `provider` is the value a workflow author types in `provider:`, which is why
 * this is keyed on it: the question it answers is "can `provider: pi` run", and
 * that needs the CLI *and* a credential.
 *
 * `subscriptions` is a list because the two dimensions cross. Pi runs
 * `kimi-code/*` on a Kimi plan and `openai-codex/*` on a ChatGPT one, so one
 * agent is backed by two accounts, and either alone is enough to run it.
 *
 * No descriptions. What an agent is *used for* is a property of the workflows,
 * which change; a sentence here would be a claim nothing keeps true, and was
 * already drifting — Claude was described as the planning-and-review agent
 * while Kimi did the implementing.
 */
export const AGENTS: {
  provider: string;
  label: string;
  subscriptions: string[];
}[] = [
  { provider: "claude", label: "Claude Code", subscriptions: ["claude"] },
  { provider: "codex", label: "Codex", subscriptions: ["codex"] },
  { provider: "pi", label: "Pi", subscriptions: ["pi", "codex"] },
  { provider: "cursor", label: "Cursor", subscriptions: ["cursor"] },
];

/**
 * What to call a provider in prose. Falls back to the `provider:` value, so a
 * CLI the server knows about but this list does not still reads as something.
 */
export function agentLabel(provider: string): string {
  return AGENTS.find((a) => a.provider === provider)?.label ?? provider;
}

/** An agent whose CLI has a newer release than the installed one. */
export interface AgentUpdate {
  provider: string;
  label: string;
  /** The version on offer. */
  latest: string;
  /** What is installed now — null when the binary could not be asked. */
  installed: string | null;
}

/**
 * Which agent versions this browser has already been shown.
 *
 * A version per provider rather than one "dismissed" flag: the whole point of
 * the notice is that a CLI goes stale on its own, so dismissing 2.1.4 must not
 * also swallow the 2.2.0 that lands next week. Per browser rather than per
 * account because it records *having been told*, which is not a setting — a
 * second administrator still gets told.
 */
export const AGENT_UPDATES_SEEN_KEY = "harness.agent-updates-seen";

/** provider → the latest version this browser was told about. */
export type SeenAgentVersions = Record<string, string>;

/**
 * The dismissal record, or empty when there isn't a usable one.
 *
 * Storage can throw outright rather than come back empty — a browser set to
 * block site data, or a private window — and the stored value is only ever as
 * trustworthy as the last version of this app that wrote it. Both failures read
 * as "nothing seen", which is the safe direction: the notice shows again rather
 * than going quiet about an update.
 */
export function readSeenAgentVersions(): SeenAgentVersions {
  try {
    const raw = window.localStorage.getItem(AGENT_UPDATES_SEEN_KEY);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return {};
    }
    return Object.fromEntries(
      Object.entries(parsed as Record<string, unknown>).filter(
        (entry): entry is [string, string] => typeof entry[1] === "string",
      ),
    );
  } catch {
    return {};
  }
}

/** Remember that these versions have been shown, keeping the other providers'. */
export function markAgentVersionsSeen(updates: AgentUpdate[]): void {
  const next: SeenAgentVersions = { ...readSeenAgentVersions() };
  for (const update of updates) next[update.provider] = update.latest;
  try {
    window.localStorage.setItem(AGENT_UPDATES_SEEN_KEY, JSON.stringify(next));
  } catch {
    // Unwritable storage means the notice comes back next load. Annoying, not
    // broken — and better than failing the render over a dismissal record.
  }
}

/**
 * The updates worth interrupting someone about: an update the server offers
 * that this browser has not already been shown.
 *
 * `update_available` is the server's answer to "can I install a newer one",
 * which is narrower than "a newer one exists" — `omp` and `cursor-agent` come
 * from outside npm and always report false, so they never produce a notice
 * that has no button behind it.
 */
export function pendingAgentUpdates(
  health: ProviderHealth[] | undefined,
  seen: SeenAgentVersions,
): AgentUpdate[] {
  return (health ?? [])
    .filter(
      (h) => h.update_available && h.latest && seen[h.provider] !== h.latest,
    )
    .map((h) => ({
      provider: h.provider,
      label: agentLabel(h.provider),
      latest: h.latest as string,
      installed: h.version,
    }));
}

/**
 * How to word the notice. Separate from the component so the wording is
 * testable and the component is only plumbing.
 */
export function describeAgentUpdates(updates: AgentUpdate[]): {
  title: string;
  detail: string;
} {
  if (updates.length === 1) {
    const [update] = updates;
    return {
      title: `${update.label} ${update.latest} is available`,
      detail: update.installed
        ? `This container runs ${update.installed}.`
        : "The installed version did not report itself.",
    };
  }
  return {
    title: `${updates.length} agent updates are available`,
    detail: updates.map((u) => `${u.label} ${u.latest}`).join(", "),
  };
}
