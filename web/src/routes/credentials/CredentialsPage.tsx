import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Check, ExternalLink, KeyRound, Loader2, Trash2 } from "lucide-react";
import { AppShell } from "@/components/AppShell";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  startKimiConnect,
  pollKimiConnect,
  startCodexConnect,
  completeCodexConnect,
  useCredentials,
  useDeleteCredential,
  useSetCredential,
  type KimiConnectStart,
  type CodexConnectStart,
} from "@/lib/credentials";
import { LANE_FOR_CREDENTIAL } from "@/lib/billing";
import { BillingFields } from "./BillingFields";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";

/** A field the user pastes for a provider. */
interface ProviderField {
  key: string;
  label: string;
  multiline?: boolean;
  /** Non-secret fields (e.g. an email) render as a plain text input. */
  secret?: boolean;
  placeholder?: string;
  help: string;
}

/** Per-provider form fields (mirrors the server's materialization contract). */
const PROVIDERS: {
  id: string;
  label: string;
  help: string;
  fields: ProviderField[];
}[] = [
  {
    id: "claude",
    label: "Claude (subscription)",
    help: "Uses your Claude subscription via the official CLI. Paste the contents of ~/.claude/.credentials.json from a machine where you've run `claude login`.",
    fields: [
      {
        key: "credentials_json",
        label: "~/.claude/.credentials.json (full JSON)",
        multiline: true,
        help: "Written to ~/.claude/.credentials.json (carries a refresh token, so it self-renews).",
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
      {
        key: "git_author_email",
        label: "Commit author email (optional)",
        secret: false,
        placeholder: "you@users.noreply.github.com",
        help: "Authors PR commits with this email so platforms that validate the commit author against a GitHub account (e.g. Vercel) accept them. Use your GitHub-verified or no-reply address. A per-project override wins; unset → a per-step synthetic address.",
      },
    ],
  },
  {
    id: "cursor",
    label: "Cursor",
    help: "Runs the Cursor CLI (cursor-agent) for `provider: cursor` nodes. Generate a user API key from the Cursor dashboard → API Keys.",
    fields: [
      {
        key: "api_key",
        label: "Cursor API key",
        help: "Sets CURSOR_API_KEY for the cursor-agent CLI.",
      },
    ],
  },
];

/** Provider IDs that have a dashboard usage card (and so a show/hide toggle). */
const USAGE_CARD_PROVIDERS = new Set(["claude", "codex", "pi", "cursor"]);

export function CredentialsPage() {
  const creds = useCredentials();
  const configured = new Map(
    (creds.data ?? []).map((c) => [c.provider, c.configured]),
  );

  return (
    <AppShell title="Credentials">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <div>
          <h1 className="flex items-center gap-2 text-lg font-semibold">
            <KeyRound className="h-5 w-5 text-accent-orange" /> Provider
            credentials
          </h1>
          <p className="mt-1 text-sm text-muted-foreground">
            Entered here, encrypted at rest in Postgres, and injected into the
            agent environment at run time. Never stored in cluster secrets.
            Values are write-only — they're never shown back.
          </p>
        </div>

        {creds.isError && (
          <p className="text-sm text-destructive">
            Credentials unavailable: {creds.error.message} (is{" "}
            <code>HARNESS_SECRET_KEY</code> set?)
          </p>
        )}

        {PROVIDERS.map((p) => (
          <ProviderSummary
            key={p.id}
            label={p.label}
            help={p.help}
            configured={configured.get(p.id) ?? false}
            usageCardProvider={
              USAGE_CARD_PROVIDERS.has(p.id) ? p.id : undefined
            }
          >
            <ProviderCard
              provider={p}
              configured={configured.get(p.id) ?? false}
            />
          </ProviderSummary>
        ))}
        <ProviderSummary
          label="Kimi-for-Coding"
          help="Connect your Kimi subscription for `provider: pi` nodes."
          configured={configured.get("pi") ?? false}
          usageCardProvider="pi"
        >
          <KimiConnectCard configured={configured.get("pi") ?? false} />
        </ProviderSummary>
        <ProviderSummary
          label="ChatGPT (Codex)"
          help="Connect your ChatGPT/Codex subscription for gpt-5.5 review steps."
          configured={configured.get("codex") ?? false}
          usageCardProvider="codex"
        >
          <CodexConnectCard configured={configured.get("codex") ?? false} />
        </ProviderSummary>
      </div>
    </AppShell>
  );
}

/**
 * Compact provider row: name + connection status + a button that opens a dialog
 * with the full settings (credential fields / connect flow + subscription cost).
 * Keeps the page scannable — details live behind "Configure"/"Connect".
 */
/**
 * A non-secret per-credential toggle: show or hide this provider's usage card on
 * the dashboard. Saves immediately (its own mutation, separate from the secret
 * form / connect flow) and the value reflects what's stored.
 */
function UsageCardToggle({ provider }: { provider: string }) {
  const creds = useCredentials();
  const save = useSetCredential();
  const shown =
    creds.data?.find((c) => c.provider === provider)?.showUsageCard ?? true;
  return (
    <label className="flex items-center gap-2 border-t pt-4 text-sm">
      <input
        type="checkbox"
        className="size-4"
        checked={shown}
        disabled={save.isPending}
        onChange={(e) =>
          save.mutate({
            provider,
            fields: { show_usage_card: e.target.checked ? "true" : "false" },
          })
        }
      />
      <span>Show usage card on dashboard</span>
    </label>
  );
}

function ProviderSummary({
  label,
  help,
  configured,
  usageCardProvider,
  children,
}: {
  label: string;
  help?: string;
  configured: boolean;
  /** When set, render a "show usage card" toggle for this provider id. */
  usageCardProvider?: string;
  children: React.ReactNode;
}) {
  return (
    <Card>
      <CardContent className="flex items-center justify-between gap-3 py-3">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-sm font-medium">{label}</span>
          {configured ? (
            <Badge variant="success">
              <Check className="h-3 w-3" /> connected
            </Badge>
          ) : (
            <Badge variant="outline">not set</Badge>
          )}
        </div>
        <Dialog>
          <DialogTrigger
            render={
              <Button variant={configured ? "outline" : "default"} size="sm" />
            }
          >
            {configured ? "Configure" : "Connect"}
          </DialogTrigger>
          <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-2xl">
            {/* The settings card carries its own title/help; keep an a11y title
                without visually duplicating it. */}
            <DialogHeader className="sr-only">
              <DialogTitle>{label}</DialogTitle>
              {help && <DialogDescription>{help}</DialogDescription>}
            </DialogHeader>
            {children}
            {usageCardProvider && (
              <UsageCardToggle provider={usageCardProvider} />
            )}
          </DialogContent>
        </Dialog>
      </CardContent>
    </Card>
  );
}

/** Kimi-for-Coding device login: Connect → approve in browser → poll until done. */
function KimiConnectCard({ configured }: { configured: boolean }) {
  const qc = useQueryClient();
  const [phase, setPhase] = useState<
    "idle" | "pending" | "connected" | "error"
  >("idle");
  const [info, setInfo] = useState<KimiConnectStart | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const stopped = useRef(false);

  useEffect(
    () => () => {
      stopped.current = true;
    },
    [],
  );

  async function connect() {
    setMsg(null);
    setInfo(null);
    setPhase("pending");
    try {
      const start = await startKimiConnect();
      setInfo(start);
      window.open(start.verification_uri, "_blank", "noopener");
      const deadline = Date.now() + start.expires_in * 1000;
      const tick = async () => {
        if (stopped.current) return;
        if (Date.now() > deadline) {
          setPhase("error");
          setMsg("Code expired — start again.");
          return;
        }
        try {
          const r = await pollKimiConnect(start.device_code);
          if (stopped.current) return;
          if (r.status === "connected") {
            setPhase("connected");
            qc.invalidateQueries({ queryKey: ["credentials"] });
            return;
          }
          if (r.status === "error") {
            setPhase("error");
            setMsg(r.message ?? "Authorization failed.");
            return;
          }
          setTimeout(tick, Math.max(2, start.interval) * 1000);
        } catch (e) {
          setPhase("error");
          setMsg((e as Error).message);
        }
      };
      setTimeout(tick, Math.max(2, start.interval) * 1000);
    } catch (e) {
      setPhase("error");
      setMsg((e as Error).message);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-2 text-sm font-semibold">
        Kimi-for-Coding
        {configured ? (
          <Badge variant="success">
            <Check className="h-3 w-3" /> connected
          </Badge>
        ) : (
          <Badge variant="outline">not connected</Badge>
        )}
      </div>
      <Button
        size="sm"
        className="self-start"
        onClick={connect}
        disabled={phase === "pending"}
      >
        {phase === "pending" && (
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
        )}
        {configured ? "Reconnect" : "Connect Kimi"}
      </Button>
      <div>
        <p className="mb-3 text-xs text-muted-foreground">
          The Kimi-for-Coding subscription (<code>kimi-code/*</code> models)
          uses a device login — no API key. Click Connect, approve in the
          browser tab that opens; the credential is stored encrypted and written
          to omp's auth db (and self-refreshes).
        </p>
        {phase === "pending" && info && (
          <div className="flex flex-col gap-1 text-sm">
            <span>
              Approve at{" "}
              <a
                href={info.verification_uri}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1 text-accent-orange hover:underline"
              >
                {info.verification_uri} <ExternalLink className="h-3 w-3" />
              </a>
            </span>
            <span className="text-muted-foreground">
              Code if prompted:{" "}
              <span className="font-mono font-semibold text-foreground">
                {info.user_code}
              </span>{" "}
              — waiting for approval…
            </span>
          </div>
        )}
        {phase === "connected" && (
          <p className="text-sm text-status-success">
            Connected ✓ — Kimi is ready.
          </p>
        )}
        {phase === "error" && <p className="text-sm text-destructive">{msg}</p>}
        <BillingFields lane="kimi" />
      </div>
    </div>
  );
}

/**
 * Codex (ChatGPT) browser/PKCE login: Connect → sign in in the tab that opens →
 * the browser redirects to a localhost URL that won't load → paste that URL back
 * → we exchange it for tokens. (PKCE, not device-code — device-code needs a
 * ChatGPT workspace setting many accounts don't have.)
 */
function CodexConnectCard({ configured }: { configured: boolean }) {
  const qc = useQueryClient();
  const [phase, setPhase] = useState<
    "idle" | "await_paste" | "exchanging" | "connected" | "error"
  >("idle");
  const [start, setStart] = useState<CodexConnectStart | null>(null);
  const [redirect, setRedirect] = useState("");
  const [msg, setMsg] = useState<string | null>(null);

  async function begin() {
    setMsg(null);
    setRedirect("");
    try {
      const s = await startCodexConnect();
      setStart(s);
      setPhase("await_paste");
      window.open(s.authorize_url, "_blank", "noopener");
    } catch (e) {
      setPhase("error");
      setMsg((e as Error).message);
    }
  }

  async function complete() {
    if (!start || !redirect.trim()) return;
    setMsg(null);
    setPhase("exchanging");
    try {
      const r = await completeCodexConnect(
        redirect.trim(),
        start.state,
        start.verifier,
      );
      if (r.status === "connected") {
        setPhase("connected");
        qc.invalidateQueries({ queryKey: ["credentials"] });
      } else {
        setPhase("error");
        setMsg(r.message ?? "Authorization failed.");
      }
    } catch (e) {
      setPhase("error");
      setMsg((e as Error).message);
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-2 text-sm font-semibold">
        ChatGPT subscription
        {configured ? (
          <Badge variant="success">
            <Check className="h-3 w-3" /> connected
          </Badge>
        ) : (
          <Badge variant="outline">not connected</Badge>
        )}
      </div>
      <Button
        size="sm"
        className="self-start"
        onClick={begin}
        disabled={phase === "exchanging"}
      >
        {phase === "exchanging" && (
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
        )}
        {configured ? "Reconnect" : "Connect ChatGPT"}
      </Button>
      <div>
        <p className="mb-3 text-xs text-muted-foreground">
          Uses your ChatGPT subscription for both the <strong>Codex</strong> CLI
          and <strong>omp</strong> (the <code>openai-codex/*</code> models on
          the Pi&nbsp;/&nbsp;omp CLI). Click Connect, sign in in the tab that
          opens. Your browser then redirects to a <code>localhost:1455</code>{" "}
          URL that <em>won’t load</em> — copy that URL from the address bar and
          paste it below. The credential is stored encrypted, written to{" "}
          <code>~/.codex/auth.json</code>, and imported into omp (and
          self-refreshes).
        </p>
        {(phase === "await_paste" || phase === "exchanging") && start && (
          <div className="flex flex-col gap-2 text-sm">
            <span className="text-muted-foreground">
              Didn’t open?{" "}
              <a
                href={start.authorize_url}
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-1 text-accent-orange hover:underline"
              >
                Open the sign-in page <ExternalLink className="h-3 w-3" />
              </a>
            </span>
            <input
              value={redirect}
              onChange={(e) => setRedirect(e.target.value)}
              placeholder="http://localhost:1455/auth/callback?code=…&state=…"
              className="h-8 rounded-md border border-input bg-transparent px-2.5 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
            />
            <div>
              <Button
                size="sm"
                onClick={complete}
                disabled={phase === "exchanging" || !redirect.trim()}
              >
                {phase === "exchanging" ? "Exchanging…" : "Complete connection"}
              </Button>
            </div>
          </div>
        )}
        {phase === "connected" && (
          <p className="text-sm text-status-success">
            Connected ✓ — Codex is ready.
          </p>
        )}
        {phase === "error" && <p className="text-sm text-destructive">{msg}</p>}
        <BillingFields lane="gpt" />
      </div>
    </div>
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
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-2 text-sm font-semibold">
        {provider.label}
        {configured ? (
          <Badge variant="success">
            <Check className="h-3 w-3" /> configured
          </Badge>
        ) : (
          <Badge variant="outline">not set</Badge>
        )}
      </div>
      <div>
        <p className="mb-3 text-xs text-muted-foreground">{provider.help}</p>
        <form onSubmit={submit} className="flex flex-col gap-3">
          {provider.fields.map((f) => (
            <label key={f.key} className="flex flex-col gap-1">
              <span className="text-xs font-medium text-muted-foreground">
                {f.label}
              </span>
              {f.multiline ? (
                <textarea
                  rows={4}
                  value={values[f.key] ?? ""}
                  onChange={(e) =>
                    setValues((v) => ({ ...v, [f.key]: e.target.value }))
                  }
                  placeholder="paste here"
                  className="rounded-md border border-input bg-transparent p-2 font-mono text-[12px] outline-none focus:ring-2 focus:ring-ring"
                />
              ) : (
                <input
                  type={f.secret === false ? "text" : "password"}
                  autoComplete="off"
                  value={values[f.key] ?? ""}
                  onChange={(e) =>
                    setValues((v) => ({ ...v, [f.key]: e.target.value }))
                  }
                  placeholder={f.placeholder ?? "paste here"}
                  className="h-8 rounded-md border border-input bg-transparent px-2.5 text-[12px] outline-none focus:ring-2 focus:ring-ring"
                />
              )}
              <span className="text-[10px] text-muted-foreground">
                {f.help}
              </span>
            </label>
          ))}
          <div className="flex items-center gap-2">
            <Button type="submit" size="sm" disabled={save.isPending}>
              {save.isPending ? "Saving…" : save.isSuccess ? "Saved" : "Save"}
            </Button>
            {save.isError && (
              <span className="text-xs text-destructive">
                {save.error.message}
              </span>
            )}
            {configured && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="ml-auto"
                onClick={() => del.mutate(provider.id)}
                disabled={del.isPending}
                title="Clear stored credential"
              >
                <Trash2 className="h-3.5 w-3.5" /> Clear
              </Button>
            )}
          </div>
        </form>
        {LANE_FOR_CREDENTIAL[provider.id] && (
          <BillingFields lane={LANE_FOR_CREDENTIAL[provider.id]} />
        )}
      </div>
    </div>
  );
}
