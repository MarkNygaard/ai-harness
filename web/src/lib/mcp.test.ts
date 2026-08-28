import { describe, expect, it } from "vitest";
import { MCP_CLIENTS, mcpSnippet } from "./mcp";

const URL = "https://harness.example.com/mcp";
const KEY = "hrn_mcp_abc123";

describe("mcpSnippet", () => {
  it("gives Claude Code the CLI form, not a project config file", () => {
    // `.mcp.json` in a project is the file that gets committed, and this
    // snippet carries a secret — `claude mcp add` writes to the user config.
    const out = mcpSnippet("claude-code", URL, KEY);
    expect(out).toContain("claude mcp add --transport http harness");
    expect(out).toContain(URL);
    expect(out).toContain(`Bearer ${KEY}`);
    expect(out).not.toContain("mcpServers");
  });

  it("uses each editor's own config key", () => {
    // VS Code reads `servers`; Cursor and Claude Desktop read `mcpServers`.
    expect(JSON.parse(mcpSnippet("vscode", URL, KEY))).toHaveProperty(
      "servers.harness.url",
      URL,
    );
    for (const client of ["cursor", "claude-desktop"] as const) {
      expect(JSON.parse(mcpSnippet(client, URL, KEY))).toHaveProperty(
        "mcpServers.harness.url",
        URL,
      );
    }
  });

  it("carries the key as a bearer header, never in the URL", () => {
    for (const { id } of MCP_CLIENTS) {
      const out = mcpSnippet(id, URL, KEY);
      expect(out).toContain(`Bearer ${KEY}`);
      // A token in a query string ends up in proxy and access logs.
      expect(out).not.toContain(`${URL}?`);
      expect(out).not.toContain(`token=${KEY}`);
    }
  });

  it("falls back to a placeholder when there is no key", () => {
    for (const { id } of MCP_CLIENTS) {
      expect(mcpSnippet(id, URL, null)).toContain("<your MCP key>");
    }
  });

  it("emits valid JSON for every file-based editor", () => {
    for (const { id } of MCP_CLIENTS) {
      if (id === "claude-code") continue; // a shell command, not JSON
      expect(() => JSON.parse(mcpSnippet(id, URL, KEY))).not.toThrow();
    }
  });
});
