import { describe, expect, it } from "vitest";
import {
  entryServer,
  entryTools,
  parseArgs,
  providerSupportsMcp,
  serverNameProblem,
} from "./mcp-servers";

describe("parseArgs", () => {
  it("takes one argument per line and keeps spaces inside a line", () => {
    expect(parseArgs("-y\n @scope/server \n\n--root\r\n/my dir")).toEqual([
      "-y",
      "@scope/server",
      "--root",
      "/my dir",
    ]);
    expect(parseArgs("")).toEqual([]);
  });
});

describe("serverNameProblem", () => {
  it("accepts what the server accepts", () => {
    expect(serverNameProblem("notion")).toBeNull();
    expect(serverNameProblem("my-server_2")).toBeNull();
  });
  it("refuses the rest", () => {
    expect(serverNameProblem("")).not.toBeNull();
    expect(serverNameProblem("Notion")).not.toBeNull();
    expect(serverNameProblem("-x")).not.toBeNull();
    expect(serverNameProblem("a".repeat(41))).not.toBeNull();
    for (const n of ["sentry", "grafana", "session", "oauth"]) {
      expect(serverNameProblem(n)).not.toBeNull();
    }
  });
});

describe("providerSupportsMcp", () => {
  it("is claude (or nothing), pi, omp and kimi only", () => {
    for (const p of [undefined, "", "claude", "pi", "omp", "kimi"]) {
      expect(providerSupportsMcp(p)).toBe(true);
    }
    expect(providerSupportsMcp("codex")).toBe(false);
    expect(providerSupportsMcp("cursor")).toBe(false);
  });
});

describe("mcp entries", () => {
  it("reads both shapes", () => {
    expect(entryServer("sentry")).toBe("sentry");
    expect(entryTools("sentry")).toBeNull();
    const narrowed = { server: "notion", tools: ["search"] };
    expect(entryServer(narrowed)).toBe("notion");
    expect(entryTools(narrowed)).toEqual(["search"]);
  });
});
