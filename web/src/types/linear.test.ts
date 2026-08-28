import { describe, expect, it } from "vitest";
import { connectionName } from "./linear";
import type { LinearConnection } from "./linear";

function connection(over: Partial<LinearConnection>): LinearConnection {
  return {
    id: "acme",
    label: null,
    workspace_name: null,
    workspace_url_key: null,
    mode: "none",
    client_configured: false,
    webhook_secret_configured: false,
    agent_scopes_granted: false,
    refresh_error: null,
    projects: [],
    ...over,
  };
}

describe("connectionName", () => {
  it("prefers the workspace's own name once connected", () => {
    // The workspace name comes from Linear, so it is what people recognize —
    // it beats whatever the operator typed when adding the account.
    const c = connection({ workspace_name: "Acme Inc", label: "acme prod" });
    expect(connectionName(c)).toBe("Acme Inc");
  });

  it("falls back to the label before the account is connected", () => {
    expect(connectionName(connection({ label: "Acme prod" }))).toBe(
      "Acme prod",
    );
  });

  it("falls back to the id when nothing else is known", () => {
    // The legacy account has no label: it existed before accounts were named.
    expect(connectionName(connection({ id: "default" }))).toBe("default");
  });
});
