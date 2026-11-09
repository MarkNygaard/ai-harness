import { useEffect, useMemo, useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import {
  IconPencil,
  IconPlugConnected,
  IconPlugConnectedX,
  IconPlus,
  IconRefresh,
  IconServer2,
  IconTrash,
} from "@tabler/icons-react";
import { SettingsShell } from "@/components/SettingsShell";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { isAdminOf, useAuthStatus } from "@/lib/auth";
import {
  MCP_OAUTH_CALLBACK_PATH,
  parseArgs,
  serverNameProblem,
  useCreateMcpServer,
  useDeleteMcpServer,
  useDisconnectMcpOAuth,
  useMcpServers,
  useRefreshMcpTools,
  useSetMcpTools,
  useStartMcpOAuth,
  useUpdateMcpServer,
  type McpServerInput,
  type McpServerView,
  type McpStatus,
  type McpTool,
} from "@/lib/mcp-servers";

/**
 * Settings → MCP servers: outside tools that workflow steps can use.
 *
 * Distinct from Settings → Editor connection, which is about an editor talking
 * to this harness. Here the harness is the client: each server is connected
 * once, with one shared account, and steps name the servers they want.
 */
export function McpServersPage() {
  const servers = useMcpServers();
  const status = useAuthStatus();
  const isAdmin = isAdminOf(status.data);
  const [editing, setEditing] = useState<McpServerView | "new" | null>(null);
  const [deleting, setDeleting] = useState<McpServerView | null>(null);

  // Built-ins first, then the rest in the order the server sent them.
  const list = useMemo(
    () =>
      [...(servers.data ?? [])].sort(
        (a, b) => Number(b.builtin) - Number(a.builtin),
      ),
    [servers.data],
  );

  return (
    <SettingsShell title="MCP servers">
      <div className="mx-auto flex max-w-3xl flex-col gap-5 p-6">
        <div className="flex items-start justify-between gap-4">
          <div>
            <h1 className="flex items-center gap-2 text-lg font-semibold">
              <IconServer2 className="size-5 text-accent-orange" /> MCP servers
            </h1>
            <p className="mt-1 max-w-prose text-sm text-muted-foreground">
              Outside tools that workflow steps can use. Each server is
              connected once for the whole harness with a shared account (a
              service mailbox, say), not per person. Steps get only the tools
              enabled here, and a new server starts with just its read-only
              tools enabled.
            </p>
          </div>
          {isAdmin && (
            <Button onClick={() => setEditing("new")}>
              <IconPlus /> Add server
            </Button>
          )}
        </div>

        <OAuthCallbackBanner />

        {servers.isError && (
          <p className="text-sm text-destructive">{servers.error.message}</p>
        )}
        {servers.isPending && (
          <p className="text-sm text-muted-foreground">Loading…</p>
        )}

        {list.map((s) => (
          <ServerCard
            key={s.name}
            server={s}
            isAdmin={isAdmin}
            onEdit={() => setEditing(s)}
            onDelete={() => setDeleting(s)}
          />
        ))}
        {servers.isSuccess && list.every((s) => s.builtin) && (
          <p className="text-sm text-muted-foreground">
            No servers of your own yet.
            {isAdmin && " Add one to give steps more tools."}
          </p>
        )}
      </div>

      {editing && (
        <ServerDialog
          server={editing === "new" ? null : editing}
          onClose={() => setEditing(null)}
        />
      )}
      {deleting && (
        <DeleteDialog server={deleting} onClose={() => setDeleting(null)} />
      )}
    </SettingsShell>
  );
}

/** The outcome of an OAuth sign-in, read once from the query and then cleared. */
function OAuthCallbackBanner() {
  const [params, setParams] = useSearchParams();
  const [result, setResult] = useState<{
    status: string;
    server: string | null;
    message: string | null;
  } | null>(null);

  useEffect(() => {
    const status = params.get("mcp");
    if (!status) return;
    setResult({
      status,
      server: params.get("server"),
      message: params.get("message"),
    });
    // Drop the params so a refresh doesn't show the banner again.
    const next = new URLSearchParams(params);
    next.delete("mcp");
    next.delete("server");
    next.delete("message");
    setParams(next, { replace: true });
  }, [params, setParams]);

  if (!result) return null;
  const ok = result.status === "connected";
  const who = result.server ? `“${result.server}”` : "The server";
  const text = ok
    ? `${who} is connected.`
    : result.status === "denied"
      ? `${who} was not connected: the sign-in was declined${result.message ? ` (${result.message})` : ""}.`
      : `${who} could not be connected${result.message ? `: ${result.message}` : "."}`;
  return (
    <div
      role="status"
      className={
        ok
          ? "rounded-md border border-status-success/40 bg-status-success/10 px-3 py-2 text-xs"
          : "rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"
      }
    >
      {text}
    </div>
  );
}

const STATUS: Record<
  McpStatus,
  { label: string; variant: "success" | "secondary" | "failed" | "outline" }
> = {
  ok: { label: "Connected", variant: "success" },
  not_connected: { label: "Not connected", variant: "secondary" },
  reconnect: { label: "Reconnect needed", variant: "outline" },
  error: { label: "Error", variant: "failed" },
};

function StatusBadge({ status }: { status: McpStatus }) {
  const s = STATUS[status] ?? STATUS.error;
  return <Badge variant={s.variant}>{s.label}</Badge>;
}

function transportSummary(s: McpServerView): string {
  if (s.transport === "stdio") return [s.command, ...s.args].join(" ");
  if (s.transport === "http") return s.url;
  return "";
}

function ServerCard({
  server: s,
  isAdmin,
  onEdit,
  onDelete,
}: {
  server: McpServerView;
  isAdmin: boolean;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const refresh = useRefreshMcpTools();
  const disconnect = useDisconnectMcpOAuth();
  const canEdit = isAdmin && !s.builtin;
  const summary = transportSummary(s);
  const error = refresh.error ?? disconnect.error;

  return (
    <Card>
      <CardHeader>
        <CardTitle className="flex flex-wrap items-center gap-2">
          {s.label || s.name}
          <code className="font-mono text-xs font-normal text-muted-foreground">
            {s.name}
          </code>
          {s.builtin && <Badge variant="outline">Built-in</Badge>}
          <StatusBadge status={s.status} />
        </CardTitle>
        <CardDescription>
          {s.builtin ? (
            <>
              Set up under{" "}
              <Link to="/settings/integrations">Settings → Integrations</Link>.
              Its tools are fixed and only read.
            </>
          ) : (
            <>
              <span className="mr-1">
                {s.transport === "stdio" ? "Local command:" : "Remote URL:"}
              </span>
              <code className="font-mono text-xs break-all">{summary}</code>
            </>
          )}
        </CardDescription>
        {canEdit && (
          <CardAction className="flex gap-1">
            <Button
              variant="ghost"
              size="icon-sm"
              title="Edit"
              onClick={onEdit}
            >
              <IconPencil />
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              title="Delete"
              onClick={onDelete}
            >
              <IconTrash />
            </Button>
          </CardAction>
        )}
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        {s.status !== "ok" && s.last_error && (
          <p className="text-xs text-destructive">{s.last_error}</p>
        )}

        {canEdit && (
          <div className="flex flex-wrap items-center gap-2">
            {s.auth === "oauth" && <OAuthConnect server={s} />}
            {s.auth === "oauth" && s.status !== "not_connected" && (
              <Button
                variant="outline"
                size="sm"
                disabled={disconnect.isPending}
                onClick={() => disconnect.mutate(s.name)}
              >
                <IconPlugConnectedX /> Disconnect
              </Button>
            )}
            <Button
              variant="outline"
              size="sm"
              disabled={refresh.isPending}
              onClick={() => refresh.mutate(s.name)}
            >
              <IconRefresh />
              {refresh.isPending ? "Refreshing…" : "Refresh tools"}
            </Button>
          </div>
        )}
        {error && <p className="text-xs text-destructive">{error.message}</p>}

        <ToolsSection server={s} editable={canEdit} />
      </CardContent>
    </Card>
  );
}

/**
 * Connect / Reconnect for an OAuth server. When the provider won't let the
 * harness register itself, the admin pastes a client they created by hand.
 */
function OAuthConnect({ server: s }: { server: McpServerView }) {
  const start = useStartMcpOAuth();
  const [own, setOwn] = useState(false);
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  // The server's message for this case asks for a client ID.
  const needsClient = own || /client/i.test(start.error?.message ?? "");
  const label = s.status === "not_connected" ? "Connect" : "Reconnect";

  return (
    <>
      <Button
        size="sm"
        disabled={start.isPending}
        onClick={() =>
          start.mutate({
            name: s.name,
            ...(clientId.trim()
              ? { client_id: clientId, client_secret: clientSecret }
              : {}),
          })
        }
      >
        <IconPlugConnected />
        {start.isPending ? "Opening sign-in…" : label}
      </Button>
      {!needsClient && (
        <Button variant="link" size="sm" onClick={() => setOwn(true)}>
          Use my own OAuth client
        </Button>
      )}
      {(start.isError || needsClient) && (
        <div className="flex w-full flex-col gap-2 rounded-md border border-border p-3">
          {start.isError && (
            <p className="text-xs text-destructive">{start.error.message}</p>
          )}
          {needsClient && (
            <>
              <p className="text-xs text-muted-foreground">
                Create an OAuth client with the provider, paste it here and
                press {label} again. If the provider asks for a redirect URL,
                use{" "}
                <code className="font-mono break-all">
                  {`<public URL>${MCP_OAUTH_CALLBACK_PATH}`}
                </code>{" "}
                (on this address:{" "}
                <code className="font-mono break-all">
                  {window.location.origin + MCP_OAUTH_CALLBACK_PATH}
                </code>
                ).
              </p>
              <div className="grid gap-2 sm:grid-cols-2">
                <Input
                  placeholder="Client ID"
                  aria-label="OAuth client ID"
                  value={clientId}
                  onChange={(e) => setClientId(e.target.value)}
                  autoComplete="off"
                />
                <Input
                  type="password"
                  placeholder="Client secret (if it has one)"
                  aria-label="OAuth client secret"
                  value={clientSecret}
                  onChange={(e) => setClientSecret(e.target.value)}
                  autoComplete="new-password"
                />
              </div>
            </>
          )}
        </div>
      )}
    </>
  );
}

/** A tool that may change things: marked so enabling one is a choice. */
function WriteBadge() {
  return (
    <Badge
      variant="outline"
      className="border-accent-orange/50 text-accent-orange"
    >
      can write
    </Badge>
  );
}

function ToolsSection({
  server: s,
  editable,
}: {
  server: McpServerView;
  editable: boolean;
}) {
  const save = useSetMcpTools();
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState<Set<string>>(
    () => new Set(s.enabled_tools),
  );
  // Follow the server whenever its saved set changes (a save, a refresh).
  const savedKey = s.enabled_tools.join("\n");
  useEffect(() => {
    setDraft(new Set(savedKey ? savedKey.split("\n") : []));
  }, [savedKey]);

  const dirty =
    draft.size !== s.enabled_tools.length ||
    s.enabled_tools.some((t) => !draft.has(t));
  const writeOn = s.tools.filter((t) => !t.read_only && draft.has(t.name));

  if (s.tools.length === 0) {
    return (
      <p className="text-xs text-muted-foreground">
        No tools listed yet.
        {editable && " Connect it, or press Refresh tools."}
      </p>
    );
  }

  const toggle = (t: McpTool, on: boolean) =>
    setDraft((d) => {
      const next = new Set(d);
      if (on) next.add(t.name);
      else next.delete(t.name);
      return next;
    });

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
        <span>
          {draft.size} of {s.tools.length} tools enabled
          {writeOn.length > 0 && `, ${writeOn.length} of them can write`}
          {s.tools_refreshed_at &&
            ` · listed ${new Date(s.tools_refreshed_at).toLocaleString()}`}
        </span>
        <Button variant="link" size="xs" onClick={() => setOpen((o) => !o)}>
          {open ? "Hide tools" : "Show tools"}
        </Button>
      </div>
      {open && (
        <ul className="flex flex-col divide-y divide-border rounded-md border border-border">
          {s.tools.map((t) => {
            const on = draft.has(t.name);
            return (
              <li
                key={t.name}
                className={
                  !t.read_only && on
                    ? "flex items-start gap-3 bg-accent-orange/10 px-3 py-2"
                    : "flex items-start gap-3 px-3 py-2"
                }
              >
                <Switch
                  size="sm"
                  className="mt-0.5"
                  checked={on}
                  disabled={!editable}
                  aria-label={`Enable ${t.name}`}
                  onCheckedChange={(v) => toggle(t, v)}
                />
                <div className="flex min-w-0 flex-col gap-0.5">
                  <div className="flex flex-wrap items-center gap-2">
                    <code className="font-mono text-xs">{t.name}</code>
                    {t.read_only ? (
                      <Badge variant="secondary">read-only</Badge>
                    ) : (
                      <WriteBadge />
                    )}
                  </div>
                  {t.description && (
                    <p className="line-clamp-2 text-xs text-muted-foreground">
                      {t.description}
                    </p>
                  )}
                </div>
              </li>
            );
          })}
        </ul>
      )}
      {editable && dirty && (
        <div className="flex items-center gap-2">
          <Button
            size="sm"
            disabled={save.isPending}
            onClick={() =>
              save.mutate({
                name: s.name,
                // Keep the server's own order.
                enabled: s.tools.map((t) => t.name).filter((n) => draft.has(n)),
              })
            }
          >
            {save.isPending ? "Saving…" : "Save tools"}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            onClick={() => setDraft(new Set(s.enabled_tools))}
          >
            Discard
          </Button>
          {writeOn.some((t) => !s.enabled_tools.includes(t.name)) && (
            <span className="text-xs text-accent-orange">
              Steps using this server will be able to change things through the
              newly enabled write tools.
            </span>
          )}
        </div>
      )}
      {save.isError && (
        <p className="text-xs text-destructive">{save.error.message}</p>
      )}
    </div>
  );
}

/** One secret row in the form. `saved` rows hold no value until replaced. */
interface SecretRow {
  key: string;
  saved: boolean;
  /** For a saved row: replace its value with `value`. */
  replace: boolean;
  value: string;
}

function FormField({
  label,
  htmlFor,
  help,
  children,
}: {
  label: string;
  htmlFor?: string;
  help?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={htmlFor}>{label}</Label>
      {children}
      {help && <p className="text-xs text-muted-foreground">{help}</p>}
    </div>
  );
}

const TRANSPORTS: Record<"stdio" | "http", string> = {
  http: "Remote URL",
  stdio: "Local command",
};
const AUTHS: Record<"none" | "secret" | "oauth", string> = {
  none: "None",
  secret: "Secret values",
  oauth: "OAuth sign-in",
};

function ServerDialog({
  server,
  onClose,
}: {
  server: McpServerView | null;
  onClose: () => void;
}) {
  const creating = server === null;
  const create = useCreateMcpServer();
  const update = useUpdateMcpServer();
  const mutation = creating ? create : update;

  const [name, setName] = useState(server?.name ?? "");
  const [label, setLabel] = useState(server?.label ?? "");
  const [transport, setTransport] = useState<"stdio" | "http">(
    server?.transport === "stdio" ? "stdio" : "http",
  );
  const [command, setCommand] = useState(server?.command ?? "");
  const [args, setArgs] = useState((server?.args ?? []).join("\n"));
  const [url, setUrl] = useState(server?.url ?? "");
  const [auth, setAuth] = useState<"none" | "secret" | "oauth">(
    server?.auth === "secret" || server?.auth === "oauth"
      ? server.auth
      : "none",
  );
  const [secrets, setSecrets] = useState<SecretRow[]>(
    (server?.secret_keys ?? []).map((key) => ({
      key,
      saved: true,
      replace: false,
      value: "",
    })),
  );
  const [touchedName, setTouchedName] = useState(false);

  const nameProblem = creating ? serverNameProblem(name) : null;
  const effectiveAuth =
    transport === "stdio" && auth === "oauth" ? "none" : auth;
  const showSecrets = effectiveAuth === "secret";

  const setRow = (i: number, patch: Partial<SecretRow>) =>
    setSecrets((rows) =>
      rows.map((r, j) => (j === i ? { ...r, ...patch } : r)),
    );
  const removeRow = (i: number) =>
    setSecrets((rows) => rows.filter((_, j) => j !== i));

  const submit = (e: React.FormEvent) => {
    e.preventDefault();
    if (nameProblem) {
      setTouchedName(true);
      return;
    }
    const rows = showSecrets ? secrets : [];
    const input: McpServerInput = {
      name: creating ? name : server.name,
      label: label.trim(),
      transport,
      ...(transport === "stdio"
        ? { command: command.trim(), args: parseArgs(args) }
        : { url: url.trim() }),
      auth: effectiveAuth,
      secret_keys: rows.filter((r) => r.saved).map((r) => r.key),
      secrets: Object.fromEntries(
        rows
          .filter((r) => r.key.trim() && (!r.saved || r.replace))
          .map((r) => [r.key.trim(), r.value]),
      ),
    };
    mutation.mutate(input, { onSuccess: onClose });
  };

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent className="sm:max-w-lg">
        <form onSubmit={submit} className="flex flex-col gap-4">
          <DialogHeader>
            <DialogTitle>
              {creating
                ? "Add MCP server"
                : `Edit ${server.label || server.name}`}
            </DialogTitle>
            <DialogDescription>
              Connected once for the whole harness. After saving, the harness
              connects and lists its tools; only the read-only ones start
              enabled.
            </DialogDescription>
          </DialogHeader>

          <div className="grid gap-3 sm:grid-cols-2">
            <FormField
              label="Name"
              htmlFor="mcp-name"
              help={
                creating
                  ? "What steps write under mcp:. Can't be changed later."
                  : "Can't be changed."
              }
            >
              <Input
                id="mcp-name"
                className="font-mono"
                value={name}
                disabled={!creating}
                aria-invalid={touchedName && !!nameProblem}
                onChange={(e) => setName(e.target.value)}
                onBlur={() => setTouchedName(true)}
                placeholder="notion"
                autoComplete="off"
              />
              {touchedName && nameProblem && (
                <p className="text-xs text-destructive">{nameProblem}</p>
              )}
            </FormField>
            <FormField label="Label" htmlFor="mcp-label">
              <Input
                id="mcp-label"
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                placeholder="Notion"
              />
            </FormField>
          </div>

          <FormField label="How to reach it">
            <Select
              value={transport}
              onValueChange={(v) => v && setTransport(v as "stdio" | "http")}
            >
              <SelectTrigger className="w-full">
                <SelectValue>
                  {(v: string | null) =>
                    TRANSPORTS[v as "stdio" | "http"] ?? v ?? ""
                  }
                </SelectValue>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="http">{TRANSPORTS.http}</SelectItem>
                <SelectItem value="stdio">{TRANSPORTS.stdio}</SelectItem>
              </SelectContent>
            </Select>
          </FormField>

          {transport === "http" ? (
            <FormField label="URL" htmlFor="mcp-url">
              <Input
                id="mcp-url"
                className="font-mono"
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="https://mcp.example.com/mcp"
              />
            </FormField>
          ) : (
            <>
              <FormField
                label="Command"
                htmlFor="mcp-command"
                help="Run inside the harness container, so it must be installed there (npx and uvx usually are)."
              >
                <Input
                  id="mcp-command"
                  className="font-mono"
                  value={command}
                  onChange={(e) => setCommand(e.target.value)}
                  placeholder="npx"
                />
              </FormField>
              <FormField
                label="Arguments"
                htmlFor="mcp-args"
                help="One per line. Spaces inside a line stay part of that argument."
              >
                <Textarea
                  id="mcp-args"
                  className="font-mono"
                  rows={3}
                  value={args}
                  onChange={(e) => setArgs(e.target.value)}
                  placeholder={"-y\n@modelcontextprotocol/server-everything"}
                />
              </FormField>
            </>
          )}

          <FormField
            label="Sign-in"
            help={
              effectiveAuth === "oauth"
                ? "After saving, press Connect on the server and sign in with the shared account."
                : effectiveAuth === "secret"
                  ? transport === "stdio"
                    ? "Passed to the command as environment variables."
                    : "Sent as HTTP headers, e.g. Authorization → Bearer …"
                  : undefined
            }
          >
            <Select
              value={effectiveAuth}
              onValueChange={(v) =>
                v && setAuth(v as "none" | "secret" | "oauth")
              }
            >
              <SelectTrigger className="w-full">
                <SelectValue>
                  {(v: string | null) =>
                    AUTHS[v as "none" | "secret" | "oauth"] ?? v ?? ""
                  }
                </SelectValue>
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="none">{AUTHS.none}</SelectItem>
                <SelectItem value="secret">{AUTHS.secret}</SelectItem>
                {transport === "http" && (
                  <SelectItem value="oauth">{AUTHS.oauth}</SelectItem>
                )}
              </SelectContent>
            </Select>
          </FormField>

          {showSecrets && (
            <div className="flex flex-col gap-2">
              <Label>
                {transport === "stdio" ? "Environment variables" : "Headers"}
              </Label>
              {secrets.map((r, i) => (
                <div key={i} className="flex items-center gap-2">
                  <Input
                    className="font-mono"
                    value={r.key}
                    disabled={r.saved}
                    aria-label="Name"
                    placeholder={
                      transport === "stdio" ? "API_TOKEN" : "Authorization"
                    }
                    onChange={(e) => setRow(i, { key: e.target.value })}
                  />
                  {r.saved && !r.replace ? (
                    <>
                      <span className="w-full text-xs text-muted-foreground">
                        ••• saved
                      </span>
                      <Button
                        type="button"
                        variant="outline"
                        size="sm"
                        onClick={() => setRow(i, { replace: true })}
                      >
                        Replace
                      </Button>
                    </>
                  ) : (
                    <Input
                      type="password"
                      value={r.value}
                      aria-label={`Value for ${r.key || "new entry"}`}
                      placeholder={r.saved ? "New value" : "Value"}
                      autoComplete="new-password"
                      onChange={(e) => setRow(i, { value: e.target.value })}
                    />
                  )}
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-sm"
                    title="Remove"
                    onClick={() => removeRow(i)}
                  >
                    <IconTrash />
                  </Button>
                </div>
              ))}
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="self-start"
                onClick={() =>
                  setSecrets((rows) => [
                    ...rows,
                    { key: "", saved: false, replace: false, value: "" },
                  ])
                }
              >
                <IconPlus /> Add {transport === "stdio" ? "variable" : "header"}
              </Button>
              <p className="text-xs text-muted-foreground">
                Values are stored encrypted and never shown again.
              </p>
            </div>
          )}

          {mutation.isError && (
            <p className="text-xs text-destructive">{mutation.error.message}</p>
          )}

          <DialogFooter>
            <Button type="button" variant="outline" onClick={onClose}>
              Cancel
            </Button>
            <Button type="submit" disabled={mutation.isPending}>
              {mutation.isPending
                ? "Saving and connecting…"
                : creating
                  ? "Add server"
                  : "Save"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function DeleteDialog({
  server,
  onClose,
}: {
  server: McpServerView;
  onClose: () => void;
}) {
  const del = useDeleteMcpServer();
  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Delete {server.label || server.name}?</DialogTitle>
          <DialogDescription>
            Its saved secrets and sign-in go too. Workflows whose steps list{" "}
            <code className="font-mono">{server.name}</code> keep running, but
            without it.
          </DialogDescription>
        </DialogHeader>
        {del.isError && (
          <p className="text-xs text-destructive">{del.error.message}</p>
        )}
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="destructive"
            disabled={del.isPending}
            onClick={() => del.mutate(server.name, { onSuccess: onClose })}
          >
            {del.isPending ? "Deleting…" : "Delete"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
