import { Bot } from "lucide-react";
import { SettingsShell } from "@/components/SettingsShell";
import { ProviderMark } from "@/components/providers/ProviderMark";
import { describeProvider } from "@/lib/provider-status";
import { useProviderHealth } from "@/lib/system";
import { useCredentials } from "@/lib/credentials";
import { CliUpdateButton, Section } from "./parts";

/**
 * What a workflow node can pick, and what backs it.
 *
 * `provider` is the value a workflow author types in `provider:`, which is why
 * this page is keyed on it rather than on the subscription: the question it
 * answers is "can `provider: pi` run", and that needs the CLI *and* a
 * credential.
 *
 * `subscriptions` is a list because the two dimensions genuinely cross. Pi runs
 * `kimi-code/*` on a Kimi plan and `openai-codex/gpt-5.5` on a ChatGPT one, so
 * one agent is backed by two accounts — and that same ChatGPT account also
 * backs the separate `codex` CLI. A row showing one subscription would have to
 * pick a side and be wrong about the other.
 *
 * The label is **Pi**, not omp: `build_args` sends only plain Pi flags and the
 * harness supplies its own `--plugin-dir`, so omp is the distribution it
 * happens to be installed from, not the agent. The binary name still reaches
 * the user through the CLI-missing status, which is where it matters.
 */
export const AGENTS: {
  provider: string;
  label: string;
  what: string;
  subscriptions: { credential: string; label: string }[];
}[] = [
  {
    provider: "claude",
    label: "Claude Code",
    what: "Anthropic's coding CLI. The default for planning and review nodes.",
    subscriptions: [{ credential: "claude", label: "Claude" }],
  },
  {
    provider: "codex",
    label: "Codex",
    what: "OpenAI's coding CLI, for gpt-5.x review steps.",
    subscriptions: [{ credential: "codex", label: "ChatGPT" }],
  },
  {
    provider: "pi",
    label: "Pi",
    what: "Installed as `omp`, a Pi distribution bundling the extensions the harness uses. The model a node names decides which account it runs on.",
    subscriptions: [
      { credential: "pi", label: "Kimi-for-Coding" },
      { credential: "codex", label: "ChatGPT" },
    ],
  },
  {
    provider: "cursor",
    label: "Cursor",
    what: "Cursor's headless agent.",
    subscriptions: [{ credential: "cursor", label: "Cursor" }],
  },
];

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
            />
          ))}
        </Section>
      </div>
    </SettingsShell>
  );
}

/**
 * One agent, and whether it can run.
 *
 * The status merges credential and CLI on purpose — see `describeProvider`. An
 * agent with a stored credential and no binary on PATH reads as connected right
 * up until the node fails, which is exactly the case this row exists to catch.
 */
function AgentRow({
  agent,
  configured,
}: {
  agent: (typeof AGENTS)[number];
  configured: Map<string, boolean>;
}) {
  const health = useProviderHealth().data?.find(
    (h) => h.provider === agent.provider,
  );
  // Any one of an agent's subscriptions is enough to run it: Pi with only a
  // Kimi plan runs kimi-code/* perfectly well, and saying "not connected"
  // because ChatGPT is absent would be false.
  const backed = agent.subscriptions.some(
    (s) => configured.get(s.credential) ?? false,
  );
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
        <code className="shrink-0 rounded bg-muted px-1 py-0.5 font-mono text-[11px] text-muted-foreground">
          provider: {agent.provider}
        </code>
        {health?.version && (
          <span className="shrink-0 font-mono text-[11px] text-muted-foreground">
            v{health.version}
          </span>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-1.5">
          {health?.update_available && (
            <CliUpdateButton
              provider={agent.provider}
              label={agent.label}
              to={health.latest}
            />
          )}
        </div>
      </div>
      <p className="text-xs text-muted-foreground">{agent.what}</p>
      <p className="text-xs text-muted-foreground">
        Backed by{" "}
        {agent.subscriptions.map((s, i) => (
          <span key={s.credential}>
            {i > 0 && " or "}
            <span
              className={
                configured.get(s.credential)
                  ? "text-foreground"
                  : "line-through opacity-60"
              }
            >
              {s.label}
            </span>
          </span>
        ))}
        {!backed && " — connect one on Subscriptions."}
      </p>
      {/* Only when there is something the dot cannot say. */}
      {detail && <p className="text-xs text-muted-foreground">{detail}</p>}
    </div>
  );
}
