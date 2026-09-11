import { describe, expect, it } from "vitest";
import { offersCliUpdate } from "./system";
import type { CliUpdateStatus, ProviderHealth } from "./system";

const IDLE: CliUpdateStatus = {
  active_runs: 0,
  installing: false,
  pending: [],
  completed: [],
  error: null,
};

function health(over: Partial<ProviderHealth> = {}): ProviderHealth {
  return {
    provider: "claude",
    binary: "claude",
    on_path: true,
    version: "2.1.268",
    latest: "2.1.268",
    update_available: false,
    reinstallable: true,
    error: null,
    ...over,
  } as ProviderHealth;
}

describe("offersCliUpdate", () => {
  /**
   * The reported bug. Claude Code and Codex sat at the latest version with an
   * "Update to 2.1.268" button beside `v2.1.268`, and pressing it changed
   * nothing because there was nothing to change.
   */
  it("offers nothing for an npm CLI that is already current", () => {
    expect(offersCliUpdate(health(), IDLE, "claude")).toBe(false);
  });

  it("offers an update when one is genuinely due", () => {
    const behind = health({ version: "2.1.187", update_available: true });
    expect(offersCliUpdate(behind, IDLE, "claude")).toBe(true);
  });

  /**
   * Cursor installs from a vendor script and publishes no version endpoint, so
   * `latest` is null and no comparison is possible. Without this it would be
   * stuck on whatever the image baked in until someone rebuilt it.
   */
  it("offers a reinstall when there is no latest to compare against", () => {
    const script = health({
      provider: "cursor",
      binary: "cursor-agent",
      version: "2026.09.08-6caf4ff",
      latest: null,
    });
    expect(offersCliUpdate(script, IDLE, "cursor")).toBe(true);
  });

  /** Pi has no installer here, so there is nothing to press. */
  it("offers nothing for a CLI the harness cannot install", () => {
    const pi = health({
      provider: "pi",
      binary: "omp",
      version: null,
      latest: null,
      reinstallable: false,
    });
    expect(offersCliUpdate(pi, IDLE, "pi")).toBe(false);
  });

  /**
   * A queued update outranks everything: the button becomes the Cancel path,
   * and hiding it would strand an update nobody could call off.
   */
  it("keeps the control while an update is queued, current or not", () => {
    const queued = { ...IDLE, pending: ["claude"], active_runs: 1 };
    expect(offersCliUpdate(health(), queued, "claude")).toBe(true);
  });

  /** Health can be absent while the first request is in flight. */
  it("offers nothing when health is unknown", () => {
    expect(offersCliUpdate(undefined, IDLE, "claude")).toBe(false);
    expect(offersCliUpdate(undefined, undefined, "claude")).toBe(false);
  });
});
