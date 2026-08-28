import { useEffect, useState } from "react";
import {
  IconBolt,
  IconCopy,
  IconPlugConnected,
  IconPlus,
  IconTrash,
} from "@tabler/icons-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  useConnectLinear,
  useCreateLinearConnection,
  useDeleteLinearConnection,
  useDisconnectLinear,
  useLinearOauthStatus,
} from "@/lib/linear";
import { connectionName } from "@/types/linear";
import type { LinearConnection } from "@/types/linear";

/** A copyable URL the operator has to paste into the Linear OAuth app. */
function UrlRow({ label, url }: { label: string; url: string }) {
  return (
    <div className="flex items-center gap-2">
      <span className="w-20 shrink-0 text-[10px] text-muted-foreground">
        {label}
      </span>
      <code className="min-w-0 flex-1 truncate rounded bg-muted px-1.5 py-0.5 font-mono text-[10px]">
        {url}
      </code>
      <Button
        variant="ghost"
        size="icon-sm"
        title="Copy — register this exact URL on the Linear OAuth app"
        onClick={() => navigator.clipboard?.writeText(url)}
      >
        <IconCopy className="size-3.5" />
      </Button>
    </div>
  );
}

/** One readiness line: satisfied, or what to do about it. */
function Check({
  ok,
  okText,
  badText,
}: {
  ok: boolean;
  okText: string;
  badText: string;
}) {
  return (
    <span
      className={
        ok
          ? "text-[11px] text-muted-foreground"
          : "text-[11px] text-destructive"
      }
    >
      {ok ? "✓ " : "• "}
      {ok ? okText : badText}
    </span>
  );
}

/** Human-readable "expires in …" for the app token, or null when unknown. */
function expiresIn(atMs: number | null): string | null {
  if (!atMs) return null;
  const mins = Math.round((atMs - Date.now()) / 60_000);
  if (mins <= 0) return "expired — refreshes on next use";
  if (mins < 90) return `expires in ${mins} min`;
  return `expires in ${Math.round(mins / 60)} h`;
}

/**
 * One connected Linear account, as an **app** (`actor=app` OAuth) so the
 * harness's comments and status moves are authored by the application instead
 * of by the person whose personal API key was pasted.
 *
 * The harness can hold several accounts; each project's issues come from one of
 * them. A single-account install has exactly one of these cards and never has to
 * choose.
 */
export function LinearConnectionCard({
  connection,
  removable,
}: {
  connection: LinearConnection;
  /** Whether to offer Remove — hidden for a lone account, which has nothing to
   *  fall back to and is better disconnected than deleted. */
  removable: boolean;
}) {
  const id = connection.id;
  const status = useLinearOauthStatus(id);
  const connect = useConnectLinear(id);
  const disconnect = useDisconnectLinear(id);
  const remove = useDeleteLinearConnection();

  const s = status.data;
  const mode = s?.mode ?? connection.mode;
  const refreshError = s?.refresh_error?.trim() ? s.refresh_error : null;

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <IconBolt className="size-4 text-muted-foreground" />
        <span className="text-xs font-medium">
          {connectionName(connection)}
        </span>
        <Badge
          variant={
            mode === "app"
              ? refreshError
                ? "destructive"
                : "success"
              : mode === "personal_key"
                ? "secondary"
                : "outline"
          }
          className="text-[10px]"
        >
          {mode === "app"
            ? refreshError
              ? "reconnect needed"
              : "app install"
            : mode === "personal_key"
              ? "personal key"
              : "not connected"}
        </Badge>
      </div>

      {mode === "app" && (
        <p className="text-[11px] text-muted-foreground">
          Connected to{" "}
          <span className="font-medium text-foreground">
            {s?.workspace_name ?? "workspace"}
          </span>
          . Comments, status changes and run links are authored by the app.
          {s?.token_scope && ` Scopes: ${s.token_scope}.`}
          {expiresIn(s?.expires_at_ms ?? null) &&
            ` Token ${expiresIn(s?.expires_at_ms ?? null)}.`}
        </p>
      )}

      {mode === "personal_key" && (
        <p className="text-[11px] text-muted-foreground">
          Using a personal API key —{" "}
          <strong>
            Linear attributes every comment and status change to the person who
            owns that key
          </strong>
          . Connect the workspace to post as the app instead. The key stays as a
          fallback until you clear it.
        </p>
      )}

      {mode === "none" && (
        <p className="text-[11px] text-muted-foreground">
          Not connected. Connect the workspace so the harness can read issues
          and write back as the app.
        </p>
      )}

      {refreshError && (
        <p className="text-[11px] text-destructive">
          Token refresh failed: {refreshError}
        </p>
      )}

      {!s?.client_configured && (
        <p className="text-[11px] text-muted-foreground">
          First create an OAuth application in Linear (Settings → API → OAuth
          applications) with the callback URL below, and enable its webhook for{" "}
          <span className="font-mono">Agent session events</span> pointed at the
          webhook URL below. Then save its client ID, secret and webhook signing
          secret here.
        </p>
      )}

      {s?.callback_url && <UrlRow label="Callback URL" url={s.callback_url} />}
      {s?.webhook_url && <UrlRow label="Webhook URL" url={s.webhook_url} />}

      {/* Delegation readiness — the two things beyond a plain connect. */}
      {mode === "app" && (
        <div className="flex flex-col gap-1 rounded border border-border/60 bg-muted/30 p-2">
          <span className="text-[10px] font-medium text-muted-foreground">
            Delegation
          </span>
          <Check
            ok={s?.agent_scopes_granted ?? false}
            okText="App can be delegated issues and @-mentioned."
            badText="This token predates the agent scopes — reconnect to allow delegation and @-mentions."
          />
          <Check
            ok={s?.webhook_secret_configured ?? false}
            okText="Webhook signing secret stored."
            badText="No webhook signing secret — register the webhook URL above on the OAuth app (subscribing to agent session events), then save its signing secret below."
          />
        </div>
      )}
      {!s?.callback_url && !status.isLoading && (
        <p className="text-[11px] text-destructive">
          No public URL configured — set HARNESS_PUBLIC_URL so Linear has a
          callback address.
        </p>
      )}

      <div className="flex items-center gap-2 pt-0.5">
        <Button
          size="sm"
          onClick={() => connect.mutate()}
          disabled={
            !s?.client_configured || !s?.callback_url || connect.isPending
          }
          title={
            s?.client_configured
              ? "Authorize in Linear (installs as an app actor)"
              : "Save the OAuth client ID and secret first"
          }
        >
          <IconPlugConnected className="size-3.5" />
          {connect.isPending
            ? "Redirecting…"
            : mode === "app"
              ? "Reconnect"
              : "Connect as app"}
        </Button>
        {mode === "app" && (
          <Button
            variant="ghost"
            size="sm"
            onClick={() => disconnect.mutate()}
            disabled={disconnect.isPending}
            title="Revoke the token at Linear and clear it"
          >
            {disconnect.isPending ? "Disconnecting…" : "Disconnect"}
          </Button>
        )}
        {removable && (
          <Button
            variant="ghost"
            size="sm"
            className="ml-auto text-muted-foreground"
            onClick={() => remove.mutate(id)}
            disabled={remove.isPending || connection.projects.length > 0}
            title={
              connection.projects.length > 0
                ? `Used by ${connection.projects.join(", ")} — point those projects at another account first`
                : "Revoke the token and remove this account"
            }
          >
            <IconTrash className="size-3.5" />
            {remove.isPending ? "Removing…" : "Remove"}
          </Button>
        )}
      </div>

      {connection.projects.length > 0 && (
        <span className="text-[10px] text-muted-foreground">
          Issues for {connection.projects.join(", ")} come from this account.
        </span>
      )}

      {(connect.isError ||
        disconnect.isError ||
        remove.isError ||
        status.isError) && (
        <span className="text-[10px] text-destructive">
          {connect.error?.message ??
            disconnect.error?.message ??
            remove.error?.message ??
            status.error?.message}
        </span>
      )}
    </div>
  );
}

/**
 * Add another Linear account. Creating it only makes the row — its OAuth app
 * details are saved against it below, and then it is connected.
 */
export function AddLinearConnection() {
  const [label, setLabel] = useState("");
  const create = useCreateLinearConnection();

  function submit(e: React.FormEvent) {
    e.preventDefault();
    const name = label.trim();
    if (!name) return;
    create.mutate({ label: name }, { onSuccess: () => setLabel("") });
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-1.5">
      <div className="flex items-center gap-2">
        <input
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          placeholder="Another Linear account (e.g. Acme)"
          aria-label="Name for the Linear account to add"
          className="h-8 min-w-0 flex-1 rounded-md border border-input bg-transparent px-2 text-[13px] outline-none focus:ring-2 focus:ring-ring"
        />
        <Button
          type="submit"
          size="sm"
          variant="outline"
          disabled={!label.trim() || create.isPending}
        >
          <IconPlus className="size-3.5" />
          {create.isPending ? "Adding…" : "Add account"}
        </Button>
      </div>
      <span className="text-[10px] text-muted-foreground">
        Each account needs its own OAuth application in Linear, registering the
        same callback and webhook URLs shown above.
      </span>
      {create.isError && (
        <span className="text-[10px] text-destructive">
          {create.error.message}
        </span>
      )}
    </form>
  );
}

/**
 * Outcome banner for the OAuth round-trip: the callback redirects the browser to
 * `/projects?linear=…`, which this reads once and then strips from the URL.
 */
export function LinearCallbackBanner() {
  const [result, setResult] = useState<{
    status: string;
    message: string | null;
  } | null>(null);

  useEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const status = params.get("linear");
    if (!status) return;
    setResult({ status, message: params.get("linear_message") });
    // Drop the params so a refresh doesn't re-show the banner.
    params.delete("linear");
    params.delete("linear_message");
    const query = params.toString();
    window.history.replaceState(
      {},
      "",
      `${window.location.pathname}${query ? `?${query}` : ""}`,
    );
  }, []);

  if (!result) return null;
  const ok = result.status === "connected";
  return (
    <div
      className={
        ok
          ? "rounded-md border border-status-success/40 bg-status-success/10 px-3 py-2 text-xs"
          : "rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"
      }
    >
      {ok
        ? `Linear connected${result.message ? ` to ${result.message}` : ""} — the harness now writes as the app.`
        : `Linear connection ${result.status}${result.message ? `: ${result.message}` : ""}`}
    </div>
  );
}
