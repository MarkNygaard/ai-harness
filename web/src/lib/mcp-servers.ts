/**
 * The MCP server registry: outside tools that workflow steps can use.
 *
 * Not to be confused with `./mcp.ts`, which is about an editor connecting *to*
 * this harness. These are servers the harness connects *out* to, once for the
 * whole install, and hands to the steps that ask for them.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { McpEntry } from "@/types/authoring";
import { apiJson, apiFetch } from "./api";

export interface McpTool {
  name: string;
  description: string;
  /** The server marks it as only reading; anything else may change things. */
  read_only: boolean;
}

export type McpTransport = "builtin" | "stdio" | "http";
export type McpAuth = "builtin" | "none" | "secret" | "oauth";
export type McpStatus = "ok" | "not_connected" | "reconnect" | "error";

export interface McpServerView {
  name: string;
  label: string;
  transport: McpTransport;
  command: string;
  args: string[];
  url: string;
  auth: McpAuth;
  /** Names only; values never leave the server. */
  secret_keys: string[];
  tools: McpTool[];
  enabled_tools: string[];
  tools_refreshed_at: string | null;
  status: McpStatus;
  last_error: string;
  builtin: boolean;
}

export interface McpServerInput {
  /** Create only; ignored on update. */
  name: string;
  label: string;
  transport: "stdio" | "http";
  command?: string;
  args?: string[];
  url?: string;
  auth: "none" | "secret" | "oauth";
  /** Existing secret names to keep. A saved name left out is deleted. */
  secret_keys: string[];
  /** New values by name. Write-only. */
  secrets: Record<string, string>;
}

/** Providers whose agents can be handed MCP servers (mirrors `MCP_PROVIDERS`). */
export const MCP_PROVIDERS = ["claude", "pi", "omp", "kimi"] as const;

/** Whether a step on this provider can use MCP servers. No provider = claude. */
export function providerSupportsMcp(provider: string | undefined): boolean {
  return !provider || (MCP_PROVIDERS as readonly string[]).includes(provider);
}

/** Names the server refuses for a new server. */
export const RESERVED_SERVER_NAMES = ["sentry", "grafana", "session", "oauth"];

/** Why a name can't be used for a new server, or `null` when it can. */
export function serverNameProblem(name: string): string | null {
  if (!name) return "Give it a name.";
  if (name.length > 40) return "At most 40 characters.";
  if (!/^[a-z0-9][a-z0-9_-]*$/.test(name))
    return "Lowercase letters, digits, - and _, starting with a letter or digit.";
  if (RESERVED_SERVER_NAMES.includes(name)) return `“${name}” is reserved.`;
  return null;
}

const KEY = ["mcp-servers"] as const;
const base = (name: string) => `/api/mcp-servers/${encodeURIComponent(name)}`;
const json = (body: unknown): RequestInit => ({
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify(body),
});

export function useMcpServers() {
  return useQuery<McpServerView[], Error>({
    queryKey: KEY,
    queryFn: async ({ signal }) =>
      (
        await apiJson<{ servers: McpServerView[] }>("/api/mcp-servers", {
          signal,
        })
      ).servers,
    retry: false,
  });
}

/** Put one server's new view into the cached list, in place or appended. */
function useMerge() {
  const qc = useQueryClient();
  return (view: McpServerView) =>
    qc.setQueryData<McpServerView[]>(KEY, (old) => {
      if (!old) return [view];
      const i = old.findIndex((s) => s.name === view.name);
      return i < 0 ? [...old, view] : old.map((s, j) => (j === i ? view : s));
    });
}

export function useCreateMcpServer() {
  const merge = useMerge();
  return useMutation<McpServerView, Error, McpServerInput>({
    mutationFn: (input) =>
      apiJson<McpServerView>("/api/mcp-servers", {
        method: "POST",
        ...json(input),
      }),
    onSuccess: merge,
  });
}

export function useUpdateMcpServer() {
  const merge = useMerge();
  return useMutation<McpServerView, Error, McpServerInput>({
    mutationFn: (input) =>
      apiJson<McpServerView>(base(input.name), {
        method: "PUT",
        ...json(input),
      }),
    onSuccess: merge,
  });
}

export function useDeleteMcpServer() {
  const qc = useQueryClient();
  return useMutation<void, Error, string>({
    mutationFn: async (name) => {
      await apiFetch(base(name), { method: "DELETE" });
    },
    onSuccess: (_, name) =>
      qc.setQueryData<McpServerView[]>(KEY, (old) =>
        old?.filter((s) => s.name !== name),
      ),
  });
}

export function useSetMcpTools() {
  const merge = useMerge();
  return useMutation<McpServerView, Error, { name: string; enabled: string[] }>(
    {
      mutationFn: ({ name, enabled }) =>
        apiJson<McpServerView>(`${base(name)}/tools`, {
          method: "PUT",
          ...json({ enabled }),
        }),
      onSuccess: merge,
    },
  );
}

export function useRefreshMcpTools() {
  const merge = useMerge();
  return useMutation<McpServerView, Error, string>({
    mutationFn: (name) =>
      apiJson<McpServerView>(`${base(name)}/refresh`, { method: "POST" }),
    onSuccess: merge,
  });
}

export function useStartMcpOAuth() {
  return useMutation<
    { url: string; callback_url: string },
    Error,
    { name: string; client_id?: string; client_secret?: string }
  >({
    mutationFn: ({ name, ...body }) =>
      apiJson(`${base(name)}/oauth/start`, { method: "POST", ...json(body) }),
    onSuccess: ({ url }) => {
      window.location.href = url;
    },
  });
}

export function useDisconnectMcpOAuth() {
  const merge = useMerge();
  return useMutation<McpServerView, Error, string>({
    mutationFn: (name) =>
      apiJson<McpServerView>(`${base(name)}/oauth/disconnect`, {
        method: "POST",
      }),
    onSuccess: merge,
  });
}

/** Where an OAuth provider sends the browser back to, for registering a client. */
export const MCP_OAUTH_CALLBACK_PATH = "/api/mcp-servers/oauth/callback";

export function entryServer(e: McpEntry): string {
  return typeof e === "string" ? e : e.server;
}

/** `null` = all enabled tools. */
export function entryTools(e: McpEntry): string[] | null {
  return typeof e === "string" ? null : e.tools;
}

/**
 * Command-line args, one per line. Spaces inside a line are kept, so an
 * argument with a space in it needs no quoting; blank lines are dropped.
 */
export function parseArgs(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean);
}
