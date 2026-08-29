import { describe, expect, it } from "vitest";
import { describeProvider } from "./provider-status";
import type { ProviderHealth } from "./system";

function health(over: Partial<ProviderHealth> = {}): ProviderHealth {
  return {
    provider: "cursor",
    binary: "cursor-agent",
    on_path: true,
    version: "1.2.3",
    latest: null,
    update_available: false,
    error: null,
    ...over,
  };
}

describe("describeProvider", () => {
  it("falls back to the credential when there is no CLI to check", () => {
    // GitHub and Linear are not a CLI at all.
    expect(describeProvider(true)).toEqual({
      status: "ok",
      detail: "Connected.",
    });
    expect(describeProvider(false).status).toBe("off");
  });

  it("does not call a provider connected when its CLI is missing", () => {
    // The case worth having this function for: the credential is fine and the
    // run still fails.
    const s = describeProvider(true, health({ on_path: false }));
    expect(s.status).toBe("bad");
    expect(s.detail).toContain("cursor-agent");
    expect(s.detail).toContain("not on PATH");
  });

  it("names the missing binary when nothing is set up at all", () => {
    const s = describeProvider(false, health({ on_path: false }));
    expect(s.status).toBe("bad");
    expect(s.detail).toContain("`cursor-agent`");
  });

  it("distinguishes an installed CLI with no credential from a missing one", () => {
    const s = describeProvider(false, health());
    expect(s.status).toBe("warn");
    expect(s.detail).toContain("no credential");
  });

  it("mentions an available update without downgrading the status", () => {
    const s = describeProvider(
      true,
      health({ provider: "claude", update_available: true, latest: "2.2.0" }),
    );
    expect(s.status).toBe("ok");
    expect(s.detail).toContain("2.2.0");
  });

  it("stays quiet about updates when there is none", () => {
    expect(describeProvider(true, health()).detail).toBe("Connected.");
  });
});
