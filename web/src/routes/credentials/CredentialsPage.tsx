import { useState } from "react";
import { Check, KeyRound, Trash2 } from "lucide-react";
import { AppShell } from "@/components/AppShell";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useCredentials, useDeleteCredential, useSetCredential } from "@/lib/credentials";

/** A field the user pastes for a provider. */
interface ProviderField {
  key: string;
  label: string;
  multiline?: boolean;
  help: string;
}

/** Per-provider form fields (mirrors the server's materialization contract). */
const PROVIDERS: { id: string; label: string; help: string; fields: ProviderField[] }[] = [
  {
    id: "claude",
    label: "Claude (subscription)",
    help: "Uses your Claude subscription via the official CLI. Provide a long-lived token from `claude setup-token`, or paste the contents of ~/.claude/.credentials.json from a machine where you've run `claude login`.",
    fields: [
      {
        key: "oauth_token",
        label: "OAuth token (from `claude setup-token`)",
        help: "Sets CLAUDE_CODE_OAUTH_TOKEN for the claude CLI.",
      },
      {
        key: "credentials_json",
        label: "…or ~/.claude/.credentials.json (full JSON)",
        multiline: true,
        help: "Written to ~/.claude/.credentials.json (carries a refresh token, so it self-renews).",
      },
    ],
  },
  {
    id: "codex",
    label: "Codex",
    help: "Paste the contents of ~/.codex/auth.json from a machine where you've run `codex login`.",
    fields: [
      {
        key: "auth_json",
        label: "~/.codex/auth.json (full JSON)",
        multiline: true,
        help: "Written to ~/.codex/auth.json (access/refresh tokens + account id).",
      },
    ],
  },
  {
    id: "pi",
    label: "Pi / Kimi",
    help: "The omp CLI. For the Kimi-for-Coding subscription (kimi-coding/* models) just paste your KIMI_API_KEY below — the harness sets the env var and writes the standard ~/.pi/agent/auth.json for you. MOONSHOT_API_KEY is only for the per-token Moonshot API (moonshotai/* models).",
    fields: [
      {
        key: "kimi_api_key",
        label: "KIMI_API_KEY (Kimi-for-Coding subscription)",
        help: "Just the key. Sets KIMI_API_KEY and writes ~/.pi/agent/auth.json as {\"kimi-coding\":{\"type\":\"api_key\",\"key\":…}}.",
      },
      {
        key: "auth_json",
        label: "Advanced: full ~/.pi/agent/auth.json (overrides the above)",
        multiline: true,
        help: "Only needed for a non-api-key login. Paste a complete auth.json; written verbatim.",
      },
      {
        key: "moonshot_api_key",
        label: "MOONSHOT_API_KEY (per-token Moonshot API, optional)",
        help: "Only for moonshotai/* per-token models.",
      },
    ],
  },
  {
    id: "github",
    label: "GitHub",
    help: "Global token used to clone private project repos and to open PRs with `gh`. A fine-grained or classic PAT with repo + pull-request access to the repos you register as projects.",
    fields: [
      {
        key: "token",
        label: "GitHub token (PAT)",
        help: "Sets GH_TOKEN / GITHUB_TOKEN and authenticates git clone/fetch over HTTPS.",
      },
    ],
  },
];

export function CredentialsPage() {
  const creds = useCredentials();
  const configured = new Map((creds.data ?? []).map((c) => [c.provider, c.configured]));

  return (
    <AppShell title="Credentials">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <div>
          <h1 className="flex items-center gap-2 text-lg font-semibold">
            <KeyRound className="h-5 w-5 text-accent-orange" /> Provider credentials
          </h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Entered here, encrypted at rest in Postgres, and injected into the agent environment at
            run time. Never stored in cluster secrets. Values are write-only — they're never shown
            back.
          </p>
        </div>

        {creds.isError && (
          <p className="text-sm text-destructive">
            Credentials unavailable: {creds.error.message} (is <code>HARNESS_SECRET_KEY</code> set?)
          </p>
        )}

        {PROVIDERS.map((p) => (
          <ProviderCard
            key={p.id}
            provider={p}
            configured={configured.get(p.id) ?? false}
          />
        ))}
      </div>
    </AppShell>
  );
}

function ProviderCard({
  provider,
  configured,
}: {
  provider: (typeof PROVIDERS)[number];
  configured: boolean;
}) {
  const save = useSetCredential();
  const del = useDeleteCredential();
  const [values, setValues] = useState<Record<string, string>>({});

  function submit(e: React.FormEvent) {
    e.preventDefault();
    const fields = Object.fromEntries(
      Object.entries(values).filter(([, v]) => v.trim() !== ""),
    );
    if (Object.keys(fields).length === 0) return;
    save.mutate(
      { provider: provider.id, fields },
      { onSuccess: () => setValues({}) },
    );
  }

  return (
    <Card>
      <CardHeader className="flex-row items-center justify-between gap-2">
        <CardTitle className="flex items-center gap-2">
          {provider.label}
          {configured ? (
            <Badge variant="success">
              <Check className="h-3 w-3" /> configured
            </Badge>
          ) : (
            <Badge variant="outline">not set</Badge>
          )}
        </CardTitle>
        {configured && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => del.mutate(provider.id)}
            disabled={del.isPending}
            title="Clear stored credential"
          >
            <Trash2 className="h-3.5 w-3.5" /> Clear
          </Button>
        )}
      </CardHeader>
      <CardContent>
        <p className="mb-3 text-xs text-muted-foreground">{provider.help}</p>
        <form onSubmit={submit} className="flex flex-col gap-3">
          {provider.fields.map((f) => (
            <label key={f.key} className="flex flex-col gap-1">
              <span className="text-xs font-medium text-muted-foreground">{f.label}</span>
              {f.multiline ? (
                <textarea
                  rows={4}
                  value={values[f.key] ?? ""}
                  onChange={(e) => setValues((v) => ({ ...v, [f.key]: e.target.value }))}
                  placeholder="paste here"
                  className="rounded-md border border-input bg-transparent p-2 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
                />
              ) : (
                <input
                  type="password"
                  autoComplete="off"
                  value={values[f.key] ?? ""}
                  onChange={(e) => setValues((v) => ({ ...v, [f.key]: e.target.value }))}
                  placeholder="paste here"
                  className="h-8 rounded-md border border-input bg-transparent px-2.5 text-[12px] outline-none focus:ring-2 focus:ring-ring"
                />
              )}
              <span className="text-[10px] text-muted-foreground">{f.help}</span>
            </label>
          ))}
          <div className="flex items-center gap-2">
            <Button type="submit" size="sm" disabled={save.isPending}>
              {save.isPending ? "Saving…" : save.isSuccess ? "Saved" : "Save"}
            </Button>
            {save.isError && <span className="text-xs text-destructive">{save.error.message}</span>}
          </div>
        </form>
      </CardContent>
    </Card>
  );
}
