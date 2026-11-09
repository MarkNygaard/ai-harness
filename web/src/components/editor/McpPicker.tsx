import { Link } from "react-router-dom";
import { TriangleAlert, X } from "lucide-react";
import type { McpEntry } from "@/types/authoring";
import {
  entryServer,
  entryTools,
  providerSupportsMcp,
  useMcpServers,
  type McpServerView,
} from "@/lib/mcp-servers";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";

const ALL = "all";
const SOME = "some";

/**
 * The step's `mcp:` list: which outside servers its agent gets, and per server
 * either every tool the admin enabled (a bare name) or some of them
 * (`{ server, tools }`).
 *
 * A server the registry doesn't know is kept and flagged rather than dropped:
 * it may be one an admin is about to add, and losing it on an unrelated edit
 * would be a silent change to the workflow.
 */
export function McpPicker({
  value,
  provider,
  onChange,
}: {
  value: McpEntry[];
  /** The effective provider (the step's own, else the workflow's). */
  provider: string | undefined;
  onChange: (next: McpEntry[] | undefined) => void;
}) {
  const servers = useMcpServers();
  const byName = new Map((servers.data ?? []).map((s) => [s.name, s]));
  const listed = new Set(value.map(entryServer));
  const addable = (servers.data ?? []).filter((s) => !listed.has(s.name));
  const supported = providerSupportsMcp(provider);

  const set = (next: McpEntry[]) => onChange(next.length ? next : undefined);
  const replace = (i: number, entry: McpEntry) =>
    set(value.map((e, j) => (j === i ? entry : e)));

  return (
    <div className="flex flex-col gap-1.5">
      <span className="text-[11px] font-medium text-muted-foreground">
        MCP servers
      </span>

      {!supported && (
        <p className="flex items-start gap-1.5 text-[11px] text-destructive">
          <TriangleAlert className="mt-px size-3.5 shrink-0" />
          This step&rsquo;s agent ({provider}) can&rsquo;t use MCP servers.
          {value.length > 0 && " Saving with any listed will be refused."}
        </p>
      )}

      {value.map((entry, i) => (
        <EntryRow
          key={`${i}-${entryServer(entry)}`}
          entry={entry}
          server={byName.get(entryServer(entry))}
          loading={servers.isPending}
          onChange={(e) => replace(i, e)}
          onRemove={() => set(value.filter((_, j) => j !== i))}
        />
      ))}

      {servers.isError && (
        <p className="text-[11px] text-destructive">
          Couldn&rsquo;t list servers: {servers.error.message}
        </p>
      )}

      {addable.length > 0 && (
        <Select
          value={null}
          onValueChange={(v) => v && set([...value, v as string])}
        >
          <SelectTrigger className="w-full text-[13px]">
            <SelectValue>{() => "Add a server…"}</SelectValue>
          </SelectTrigger>
          <SelectContent>
            {addable.map((s) => (
              <SelectItem key={s.name} value={s.name}>
                {s.label || s.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
      {servers.isSuccess && addable.length === 0 && value.length === 0 && (
        <p className="text-[11px] text-muted-foreground">
          No servers registered.{" "}
          <Link className="underline" to="/settings/mcp-servers">
            Settings → MCP servers
          </Link>
        </p>
      )}
    </div>
  );
}

function EntryRow({
  entry,
  server,
  loading,
  onChange,
  onRemove,
}: {
  entry: McpEntry;
  server: McpServerView | undefined;
  loading: boolean;
  onChange: (e: McpEntry) => void;
  onRemove: () => void;
}) {
  const name = entryServer(entry);
  const picked = entryTools(entry);
  const enabled = server?.enabled_tools ?? [];
  // Picked tools the admin has since turned off still show, so they can be
  // seen and unticked rather than vanishing from the list.
  const stale = (picked ?? []).filter((t) => !enabled.includes(t));

  const toggle = (tool: string, on: boolean) => {
    const cur = picked ?? [];
    const next = on ? [...cur, tool] : cur.filter((t) => t !== tool);
    // Keep the admin's order for the ones still enabled.
    const ordered = [
      ...enabled.filter((t) => next.includes(t)),
      ...next.filter((t) => !enabled.includes(t)),
    ];
    onChange({ server: name, tools: ordered });
  };

  return (
    <div className="flex flex-col gap-1.5 rounded-md border border-border p-2">
      <div className="flex items-center justify-between gap-2">
        <span className="truncate text-[13px] font-medium">
          {server?.label || name}
          {server && server.label && server.label !== name && (
            <code className="ml-1.5 font-mono text-[11px] font-normal text-muted-foreground">
              {name}
            </code>
          )}
        </span>
        <Button
          variant="ghost"
          size="icon-xs"
          title="Remove server"
          onClick={onRemove}
        >
          <X />
        </Button>
      </div>

      {!server && !loading && (
        <p className="flex items-start gap-1.5 text-[11px] text-accent-orange">
          <TriangleAlert className="mt-px size-3.5 shrink-0" />
          Not registered. An admin can add it under Settings → MCP servers;
          until then the step runs without it.
        </p>
      )}

      {server && (
        <>
          <Select
            value={picked === null ? ALL : SOME}
            onValueChange={(v) =>
              onChange(v === ALL ? name : { server: name, tools: picked ?? [] })
            }
          >
            <SelectTrigger size="sm" className="w-full text-[12px]">
              <SelectValue>
                {(v: string | null) =>
                  v === SOME ? "Only some tools" : "All enabled tools"
                }
              </SelectValue>
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL}>All enabled tools</SelectItem>
              <SelectItem value={SOME}>Only some tools</SelectItem>
            </SelectContent>
          </Select>

          {picked !== null && (
            <div className="flex flex-col gap-1">
              {enabled.length === 0 && stale.length === 0 && (
                <p className="text-[11px] text-muted-foreground">
                  No tools enabled on this server.
                </p>
              )}
              {[...enabled, ...stale].map((t) => {
                const tool = server.tools.find((x) => x.name === t);
                const off = !enabled.includes(t);
                return (
                  <label
                    key={t}
                    className="flex items-center gap-2 text-[12px]"
                    title={tool?.description || undefined}
                  >
                    <Checkbox
                      checked={picked.includes(t)}
                      onCheckedChange={(on) => toggle(t, on)}
                    />
                    <code className="truncate font-mono">{t}</code>
                    {tool && !tool.read_only && (
                      <span className="text-[10px] text-accent-orange">
                        can write
                      </span>
                    )}
                    {off && (
                      <span className="text-[10px] text-destructive">
                        not enabled by the admin
                      </span>
                    )}
                  </label>
                );
              })}
              {picked.length === 0 && enabled.length > 0 && (
                <p className="text-[11px] text-accent-orange">
                  No tools picked: the step gets none from this server.
                </p>
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}
