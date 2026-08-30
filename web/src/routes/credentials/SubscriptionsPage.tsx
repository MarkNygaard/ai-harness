import { KeyRound } from "lucide-react";
import { SettingsShell } from "@/components/SettingsShell";
import { useCredentials } from "@/lib/credentials";
import {
  CodexConnectCard,
  KimiConnectCard,
  ProviderCard,
  ProviderSummary,
  Section,
  providerDef,
} from "./parts";

/**
 * The accounts that pay for model access.
 *
 * Separate from Agents because the two cross: one ChatGPT account backs both
 * the `codex` CLI and `omp`, and `omp` is also backed by Kimi. Keyed on the
 * account, so connecting one is done once rather than once per agent that
 * happens to use it.
 *
 * Status here is the credential alone — whether the CLI is installed is the
 * Agents page's question, and answering it twice in different words is how a
 * page stops being scannable.
 */
export function SubscriptionsPage() {
  const creds = useCredentials();
  const configured = new Map(
    (creds.data ?? []).map((c) => [c.provider, c.configured]),
  );
  const is = (key: string) => configured.get(key) ?? false;

  return (
    <SettingsShell title="Subscriptions">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <div>
          <h1 className="flex items-center gap-2 text-lg font-semibold">
            <KeyRound className="h-5 w-5 text-accent-orange" /> Subscriptions
          </h1>
          <p className="mt-1 text-sm text-muted-foreground">
            The accounts your agents run on. Entered here, encrypted at rest in
            Postgres, and injected into the agent environment at run time. Never
            stored in cluster secrets. Values are write-only — they're never
            shown back.
          </p>
        </div>

        {creds.isError && (
          <p className="text-sm text-destructive">
            Credentials unavailable: {creds.error.message} (is{" "}
            <code>HARNESS_SECRET_KEY</code> set?)
          </p>
        )}

        <Section
          title="Model access"
          help="Each has a usage card and a billing lane. Which agent uses which is on the Agents page."
        >
          <ProviderSummary
            provider="claude"
            label={providerDef("claude").label}
            help={providerDef("claude").help}
            configured={is("claude")}
            credentialOnly
            usageCardProvider="claude"
          >
            <ProviderCard
              provider={providerDef("claude")}
              configured={is("claude")}
            />
          </ProviderSummary>
          <ProviderSummary
            provider="codex"
            label="ChatGPT"
            help="Your ChatGPT/Codex subscription. Backs both the `codex` CLI and `openai-codex/*` models on omp."
            configured={is("codex")}
            credentialOnly
            usageCardProvider="codex"
          >
            <CodexConnectCard configured={is("codex")} />
          </ProviderSummary>
          <ProviderSummary
            provider="pi"
            mark="kimi"
            label="Kimi-for-Coding"
            help="Your Kimi subscription. Backs `kimi-code/*` models on omp."
            configured={is("pi")}
            credentialOnly
            usageCardProvider="pi"
          >
            <KimiConnectCard configured={is("pi")} />
          </ProviderSummary>
          <ProviderSummary
            provider="cursor"
            label={providerDef("cursor").label}
            help={providerDef("cursor").help}
            configured={is("cursor")}
            credentialOnly
            usageCardProvider="cursor"
          >
            <ProviderCard
              provider={providerDef("cursor")}
              configured={is("cursor")}
            />
          </ProviderSummary>
        </Section>
      </div>
    </SettingsShell>
  );
}
