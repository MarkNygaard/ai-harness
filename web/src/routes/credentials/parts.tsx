/**
 * The pieces the three credential pages are built from.
 *
 * Agents, Subscriptions and Integrations are three views over one credential
 * store, so the rows, the connect flows and the provider table live here rather
 * than being duplicated or owned by whichever page happened to come first.
 */
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  ArrowUpCircle,
  Check,
  ExternalLink,
  Loader2,
  Trash2,
} from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ProviderMark } from "@/components/providers/ProviderMark";
import { describeProvider } from "@/lib/provider-status";
import { useProviderHealth, useUpdateAgentCli } from "@/lib/system";
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
import {
  AddLinearConnection,
  LinearConnectionCard,
} from "@/components/credentials/LinearConnect";
import { useLinearConnections } from "@/lib/linear";
import type { LinearConnection } from "@/types/linear";
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

/** A provider whose credential is a set of pasted fields. */
interface ProviderDef {
  id: string;
  label: string;
  help: string;
  fields: ProviderField[];
}

/** Per-provider form fields (mirrors the server's materialization contract). */
const PROVIDERS: ProviderDef[] = [
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
  {
    id: "linear",
    label: "Linear",
    help: "The OAuth application backing the workspace connection above. Create it in Linear → Settings → API → OAuth applications, registering the callback URL shown above.",
    fields: [
      {
        key: "client_id",
        label: "OAuth client ID",
        secret: false,
        help: "From the Linear OAuth application. Used to build the authorization URL.",
      },
      {
        key: "client_secret",
        label: "OAuth client secret",
        help: "The same application's secret. Used only for the code exchange and token refresh.",
      },
      {
        key: "webhook_secret",
        label: "Webhook signing secret",
        help: "From the OAuth application's webhook (subscribe it to agent session events, pointed at the webhook URL above). Every inbound delegation is verified against this; without it the webhook rejects everything.",
      },
      {
        key: "api_key",
        label: "Personal API key (legacy)",
        placeholder: "lin_api_…",
        help: "Fallback for a workspace not yet connected as an app. Linear attributes every comment and status change made with this key to the person who owns it — prefer the app install.",
      },
    ],
  },
];

/** Look up a field-based provider definition by id. */
export function providerDef(id: string): ProviderDef {
  const def = PROVIDERS.find((p) => p.id === id);
  if (!def) throw new Error(`no provider definition for \`${id}\``);
  return def;
}

// Which providers have a dashboard usage card (and so a show/hide toggle) is now
// expressed by the `usageCardProvider` passed per row below — it is exactly the
// agent-provider group, which is why the page is grouped that way.

export function LinearAccounts() {
  const connections = useLinearConnections();
  const list = connections.data ?? [];
  // Before the first account exists there is still the legacy row to configure,
  // so fall back to showing one unconnected card rather than nothing.
  const accounts: LinearConnection[] =
    list.length > 0
      ? list
      : [
          {
            id: "default",
            label: null,
            workspace_name: null,
            workspace_url_key: null,
            mode: "none",
            client_configured: false,
            webhook_secret_configured: false,
            agent_scopes_granted: false,
            refresh_error: null,
            projects: [],
          },
        ];

  return (
    <div className="flex flex-col gap-3">
      {/* No box around each account. With one account -- the usual case -- it
          duplicated the dialog's own edge and collided with the close button.
          Several accounts are told apart by a rule between them instead, which
          separates just as well and never fights the chrome. */}
      <div className="flex flex-col divide-y">
        {accounts.map((c) => (
          <div
            key={c.id}
            className="flex flex-col gap-3 py-4 first:pt-0 last:pb-0"
          >
            <LinearConnectionCard
              connection={c}
              removable={accounts.length > 1}
            />
            <ProviderCard
              provider={providerDef("linear")}
              configured={c.client_configured}
              credentialKey={credentialKeyFor(c.id)}
            />
          </div>
        ))}
      </div>
      <AddLinearConnection />
      {connections.isError && (
        <span className="text-[10px] text-destructive">
          {connections.error.message}
        </span>
      )}
    </div>
  );
}

/**
 * Where an account's OAuth application is stored. The first account predates
 * named connections and keeps the bare `linear` row, so existing installs need
 * no migration.
 */
function credentialKeyFor(id: string): string {
  return id === "default" ? "linear" : `linear:${id}`;
}

/** A titled group of provider rows, so agents and integrations read apart. */
export function Section({
  title,
  help,
  children,
}: {
  title: string;
  help: string;
  children: React.ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2">
      <div>
        <h2 className="text-xs font-semibold uppercase tracking-wide text-muted-foreground">
          {title}
        </h2>
        <p className="mt-0.5 text-xs text-muted-foreground">{help}</p>
      </div>
      {/* One card per group rather than per provider: the rows read as a list
          you scan down, which a stack of separate boxes does not. */}
      <Card>
        <CardContent className="flex flex-col divide-y px-4 py-1">
          {children}
        </CardContent>
      </Card>
    </section>
  );
}

/**
 * A non-secret per-credential toggle: show or hide this provider's usage card on
 * the dashboard. Saves immediately (its own mutation, separate from the secret
 * form / connect flow) and the value reflects what's stored.
 */
export function UsageCardToggle({ provider }: { provider: string }) {
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

/**
 * One provider, at a glance: its mark, whether it can actually run, and a way in.
 *
 * The status line underneath is the point. A credential and a CLI are separate
 * things that can each be missing, and "connected" alone cannot tell you which
 * — so the row states it in words, and the dot on the mark only repeats it.
 *
 * Everything else stays behind "Configure": the page is a list to scan, not a
 * form to fill.
 */
export function ProviderSummary({
  provider,
  label,
  help,
  configured,
  usageCardProvider,
  credentialOnly,
  mark,
  children,
}: {
  /** Credential-store key — picks the brand mark and the CLI health entry. */
  provider: string;
  label: string;
  help?: string;
  configured: boolean;
  /** When set, render a "show usage card" toggle for this provider id. */
  usageCardProvider?: string;
  /**
   * Brand to draw, when it differs from the credential key. The `pi` credential
   * *is* Kimi-for-Coding, so on this page it wears Kimi's mark — while the
   * agent it backs is Pi, and wears Pi's.
   */
  mark?: string;
  /**
   * Report the credential only, ignoring whether a CLI is installed.
   *
   * Subscriptions and Agents are two views over the same key, and merging CLI
   * health into both means a missing binary is reported twice in different
   * words. The Agents page is where "can this run" is answered; here the
   * question is only whether the account is connected.
   */
  credentialOnly?: boolean;
  children: React.ReactNode;
}) {
  const healthQuery = useProviderHealth();
  const health = credentialOnly
    ? undefined
    : healthQuery.data?.find((h) => h.provider === provider);
  const {
    status,
    label: statusLabel,
    detail,
  } = describeProvider(configured, health);

  return (
    <div className="flex flex-col gap-1 py-2.5 first:pt-1 last:pb-1">
      <div className="flex items-center gap-2">
        <ProviderMark
          provider={mark ?? provider}
          status={status}
          label={statusLabel}
        />
        <span className="truncate text-sm font-medium">{label}</span>
        <div className="ml-auto flex shrink-0 items-center gap-1.5">
          <Dialog>
            <DialogTrigger
              render={
                <Button
                  variant={configured ? "outline" : "default"}
                  size="sm"
                />
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
        </div>
      </div>
      {/* Only when there is something the dot cannot say. */}
      {detail && <p className="text-xs text-muted-foreground">{detail}</p>}
    </div>
  );
}

/**
 * Install the newer CLI the row just reported.
 *
 * This used to live in the sidebar footer, where it was permanently visible to
 * everyone and actionable by nobody but an admin. It belongs beside the version
 * it is updating. The install goes into the container's persistent
 * `$HOME/.local` (see `system_routes.rs`), so it survives restarts.
 *
 * Shown for any provider the server says has an update, which is any CLI it can
 * actually install -- Claude Code and Codex today.
 */
export function CliUpdateButton({
  provider,
  label,
  to,
}: {
  provider: string;
  label: string;
  to: string | null;
}) {
  const update = useUpdateAgentCli();
  const failed = update.isError
    ? update.error.message
    : update.data && !update.data.ok
      ? update.data.message
      : null;

  if (failed) {
    return (
      <span className="text-[11px] text-status-failed" title={failed}>
        Update failed — see server logs
      </span>
    );
  }

  return (
    <Button
      type="button"
      size="sm"
      variant="outline"
      className="gap-1"
      disabled={update.isPending}
      onClick={() => update.mutate(provider)}
      title={`Update ${label} to ${to}`}
    >
      {update.isPending ? (
        <>
          <Loader2 className="size-3 animate-spin" /> Updating…
        </>
      ) : (
        <>
          <ArrowUpCircle className="size-3" /> Update to {to}
        </>
      )}
    </Button>
  );
}

/** Kimi-for-Coding device login: Connect → approve in browser → poll until done. */
export function KimiConnectCard({ configured }: { configured: boolean }) {
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
export function CodexConnectCard({ configured }: { configured: boolean }) {
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

export function ProviderCard({
  provider,
  configured,
  credentialKey,
}: {
  provider: (typeof PROVIDERS)[number];
  configured: boolean;
  /**
   * Credential row to write to; defaults to the provider's own id. Each named
   * Linear account stores its OAuth application under `linear:<id>`, so the
   * same set of fields is saved against a different row per account.
   */
  credentialKey?: string;
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
      { provider: credentialKey ?? provider.id, fields },
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
