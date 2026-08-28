/**
 * The MCP connection: what an editor needs to talk to this harness.
 */
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { apiJson } from "./api";

export interface McpConnection {
  /** `<public url>/mcp`, or `null` when no public URL is configured. */
  endpoint: string | null;
  /** The MCP key, or `null` where this install cannot hold one. */
  token: string | null;
  /** True when `/mcp` is reachable with no credential at all. */
  unauthenticated: boolean;
  run_tools: string[];
  authoring_tools: string[];
}

export function useMcpConnection() {
  return useQuery<McpConnection, Error>({
    queryKey: ["mcp", "connection"],
    queryFn: ({ signal }) =>
      apiJson<McpConnection>("/api/mcp/connection", { signal }),
    retry: false,
    // The key doesn't change on its own, and polling it would put a secret on
    // the wire every few seconds for no reason.
    refetchInterval: false,
    staleTime: Infinity,
  });
}

/** Replace the key. Every editor configured with the old one stops working. */
export function useRegenerateMcpKey() {
  const qc = useQueryClient();
  return useMutation<McpConnection, Error, void>({
    mutationFn: () =>
      apiJson<McpConnection>("/api/mcp/connection", { method: "POST" }),
    onSuccess: (data) => {
      qc.setQueryData(["mcp", "connection"], data);
    },
  });
}

/** The editors we render a ready-made snippet for. */
export type McpClient = "claude-code" | "cursor" | "vscode" | "claude-desktop";

export const MCP_CLIENTS: { id: McpClient; label: string; file: string }[] = [
  { id: "claude-code", label: "Claude Code", file: "CLI, or ~/.claude.json" },
  { id: "cursor", label: "Cursor", file: "~/.cursor/mcp.json" },
  { id: "vscode", label: "VS Code", file: ".vscode/mcp.json" },
  {
    id: "claude-desktop",
    label: "Claude Desktop",
    file: "claude_desktop_config.json",
  },
];

/**
 * The configuration to paste, per editor.
 *
 * Claude Code gets the CLI form rather than a config file: `claude mcp add`
 * writes to your **user** config, whereas a project's `.mcp.json` is the file
 * that ends up committed — and this snippet contains a secret.
 */
export function mcpSnippet(
  client: McpClient,
  endpoint: string,
  token: string | null,
): string {
  const auth = token ?? "<your MCP key>";
  const server = (key: string) =>
    JSON.stringify(
      {
        [key]: {
          harness: {
            type: "http",
            url: endpoint,
            headers: { Authorization: `Bearer ${auth}` },
          },
        },
      },
      null,
      2,
    );

  switch (client) {
    case "claude-code":
      return [
        `claude mcp add --transport http harness ${endpoint} \\`,
        `  --header "Authorization: Bearer ${auth}"`,
      ].join("\n");
    case "vscode":
      return server("servers");
    case "cursor":
    case "claude-desktop":
      return server("mcpServers");
  }
}
