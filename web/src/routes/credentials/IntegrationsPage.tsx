import { Plug } from "lucide-react";
import { SettingsShell } from "@/components/SettingsShell";
import { useCredentials } from "@/lib/credentials";
import { LinearCallbackBanner } from "@/components/credentials/LinearConnect";
import {
  LinearAccounts,
  ProviderCard,
  ProviderSummary,
  Section,
  providerDef,
} from "./parts";

/** Where work comes from and where it goes. No usage card, no billing lane. */
export function IntegrationsPage() {
  const creds = useCredentials();
  const configured = new Map(
    (creds.data ?? []).map((c) => [c.provider, c.configured]),
  );
  const is = (key: string) => configured.get(key) ?? false;

  return (
    <SettingsShell title="Integrations">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <div>
          <h1 className="flex items-center gap-2 text-lg font-semibold">
            <Plug className="h-5 w-5 text-accent-orange" /> Integrations
          </h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Where work comes from and where results land: the repos runs operate
            on, and the issue tracker that triggers them.
          </p>
        </div>

        {creds.isError && (
          <p className="text-sm text-destructive">
            Credentials unavailable: {creds.error.message} (is{" "}
            <code>HARNESS_SECRET_KEY</code> set?)
          </p>
        )}

        {/* Outcome of a returning Linear OAuth redirect, if any. */}
        <LinearCallbackBanner />

        <Section
          title="Connected services"
          help="Read and written as the harness itself, so runs act under their own identity rather than a person's."
        >
          <ProviderSummary
            provider="github"
            label={providerDef("github").label}
            help={providerDef("github").help}
            configured={is("github")}
          >
            <ProviderCard
              provider={providerDef("github")}
              configured={is("github")}
            />
          </ProviderSummary>
          <ProviderSummary
            provider="linear"
            label="Linear"
            help="Connect each Linear account as an app so the harness's comments and status changes are authored by the app, not by a person. Projects pick which account their issues come from."
            configured={is("linear")}
          >
            <LinearAccounts />
          </ProviderSummary>
        </Section>
      </div>
    </SettingsShell>
  );
}
