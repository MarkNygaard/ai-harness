import { Bot } from "lucide-react";
import { SettingsShell } from "@/components/SettingsShell";
import { ProviderMark } from "@/components/providers/ProviderMark";
import { AGENTS } from "@/lib/agents";
import { describeProvider } from "@/lib/provider-status";
import {
  useCliUpdateStatus,
  useProviderHealth,
  type CliUpdateStatus,
  type CompletedCliUpdate,
} from "@/lib/system";
import { useCredentials } from "@/lib/credentials";
import { CliUpdateButton, Section } from "./parts";

/**
 * The agents that execute workflow nodes: what is installed, at what version,
 * and whether it can actually run.
 *
 * Deliberately read-mostly. The only action here is an update, because that is
 * the only thing about an agent that changes without anyone deciding it should
 * — a CLI goes stale on its own. Connecting an account is a decision, and lives
 * on Subscriptions.
 */
export function AgentsPage() {
  const creds = useCredentials();
  // One query for the whole table: the queue is instance-wide, and asking per
  // row would poll it four times over.
  const status = useCliUpdateStatus().data;
  const configured = new Map(
    (creds.data ?? []).map((c) => [c.provider, c.configured]),
  );

  return (
    <SettingsShell title="Agents">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <div>
          <h1 className="flex items-center gap-2 text-lg font-semibold">
            <Bot className="h-5 w-5 text-accent-orange" /> Agents
          </h1>
          <p className="mt-1 text-sm text-muted-foreground">
            The CLIs that execute workflow nodes. Each node names one of these
            in <code>provider:</code>, and it can only run when the binary is
            installed <em>and</em> a subscription backs it.
          </p>
        </div>

        <Section
          title="Installed agents"
          help="Baked into the image and updated into the container's persistent home, so an update survives a restart."
        >
          {AGENTS.map((agent) => (
            <AgentRow
              key={agent.provider}
              agent={agent}
              configured={configured}
              queue={status}
            />
          ))}
        </Section>
      </div>
    </SettingsShell>
  );
}

/**
 * One agent: whether it can run, what it is called, what version, and a way to
 * update it when there is a newer one.
 *
 * The dot carries the state and `detail` speaks only when something is wrong —
 * a stored credential with no binary on PATH reads as working right up until
 * the node fails, and that is the one thing this row must not stay quiet about.
 */
function AgentRow({
  agent,
  configured,
  queue,
}: {
  agent: (typeof AGENTS)[number];
  configured: Map<string, boolean>;
  queue: CliUpdateStatus | undefined;
}) {
  const health = useProviderHealth().data?.find(
    (h) => h.provider === agent.provider,
  );
  // Any one account is enough: Pi with only a Kimi plan runs `kimi-code/*`
  // perfectly well, and calling that "not connected" would be false.
  const backed = agent.subscriptions.some((s) => configured.get(s) ?? false);
  const {
    status,
    label: statusLabel,
    detail,
  } = describeProvider(backed, health);

  return (
    <div className="flex flex-col gap-1 py-2.5 first:pt-1 last:pb-1">
      <div className="flex items-center gap-2">
        <ProviderMark
          provider={agent.provider}
          status={status}
          label={statusLabel}
        />
        <span className="truncate text-sm font-medium">{agent.label}</span>
        {/* The value a workflow types. Worth keeping: the name does not give it
            away — Claude Code is `claude`, Pi is `pi`. */}
        <code className="shrink-0 rounded bg-muted px-1 py-0.5 font-mono text-[11px] text-muted-foreground">
          provider: {agent.provider}
        </code>
        {health?.version && (
          <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
            v{health.version}
          </span>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-1.5">
          {(health?.update_available ||
            queue?.pending.includes(agent.provider)) && (
            <CliUpdateButton
              provider={agent.provider}
              label={agent.label}
              to={health?.latest ?? null}
              queue={queue}
            />
          )}
        </div>
      </div>
      {detail && <p className="text-xs text-muted-foreground">{detail}</p>}
      <QueueOutcome
        outcome={queue?.completed.find((c) => c.provider === agent.provider)}
      />
    </div>
  );
}

/**
 * What the queue did while nobody was watching.
 *
 * A queued update installs itself minutes or hours later, so without this the
 * only trace of it having happened is a version number that quietly changed —
 * and a failure would leave no trace at all. Kept for a day, server-side.
 */
function QueueOutcome({
  outcome,
}: {
  outcome: CompletedCliUpdate | undefined;
}) {
  if (!outcome) return null;
  return (
    <p
      className={
        outcome.ok
          ? "text-xs text-muted-foreground"
          : "text-xs text-status-failed"
      }
      title={outcome.message ?? undefined}
    >
      {outcome.ok
        ? `Updated to v${outcome.version ?? "latest"} ${whenPhrase(outcome.at)}, once runs were idle.`
        : `The queued update failed ${whenPhrase(outcome.at)} — see server logs.`}
    </p>
  );
}

/** "4 minutes ago" for something that happened within the last day. */
function whenPhrase(at: string): string {
  const ms = Date.now() - new Date(at).getTime();
  if (!Number.isFinite(ms)) return "recently";
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  const hours = Math.round(minutes / 60);
  return `${hours} hour${hours === 1 ? "" : "s"} ago`;
}
