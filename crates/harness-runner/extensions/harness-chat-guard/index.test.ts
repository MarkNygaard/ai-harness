import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import harnessChatGuard, {
  checkPattern,
  codeGlob,
  codeGrep,
  codeRead,
  confine,
  isAllowedTool,
  ROOT_ENV,
} from "./index";

let base: string;
let root: string;

beforeAll(() => {
  base = fs.mkdtempSync(path.join(os.tmpdir(), "chat-guard-"));
  root = path.join(base, "checkout");
  fs.mkdirSync(path.join(root, "src", "nested"), { recursive: true });
  fs.mkdirSync(path.join(root, ".git"));
  fs.writeFileSync(path.join(root, "src", "main.ts"), "const total = price * qty;\nexport {};\n");
  fs.writeFileSync(path.join(root, "src", "nested", "deep.ts"), "// discount applied here\n");
  fs.writeFileSync(path.join(root, ".git", "config"), "secret-ish\n");
  fs.writeFileSync(path.join(base, "secret.txt"), "SECRET-TOKEN-4242\n");
});

afterAll(() => {
  fs.rmSync(base, { recursive: true, force: true });
});

describe("confine", () => {
  test("allows paths inside the checkout", () => {
    expect(confine(root, "src/main.ts")).toBe(fs.realpathSync(path.join(root, "src", "main.ts")));
    expect(confine(root, undefined)).toBe(fs.realpathSync(root));
    expect(confine(root, "./src/../src/main.ts")).toBe(
      fs.realpathSync(path.join(root, "src", "main.ts")),
    );
  });

  test.each([
    ["../secret.txt"],
    ["src/../../secret.txt"],
    ["/etc/passwd"],
    ["/proc/self/environ"],
    ["~/.claude/.credentials.json"],
    ["memory://notes"],
    ["skill://x"],
    ["https://example.com/x"],
    ["src/main.ts:1-5"],
    ["file.zip:inner"],
  ])("refuses %s", (p) => {
    expect(() => confine(root, p)).toThrow();
  });

  test("refuses an absolute path outside, and allows one inside", () => {
    // Refused either way: outside the root, or (a Windows drive) not a plain path.
    expect(() => confine(root, path.join(base, "secret.txt"))).toThrow();
    if (process.platform !== "win32") {
      expect(confine(root, path.join(root, "src"))).toBe(fs.realpathSync(path.join(root, "src")));
    }
  });

  test("follows a symlink before deciding", () => {
    if (process.platform === "win32") return; // symlinks need privileges there
    const link = path.join(root, "escape");
    fs.symlinkSync(path.join(base, "secret.txt"), link);
    try {
      expect(() => confine(root, "escape")).toThrow(/outside/);
    } finally {
      fs.unlinkSync(link);
    }
  });
});

describe("checkPattern", () => {
  test.each([["/etc/*"], ["../*"], ["src/../../*"], ["~/x"], ["C:/x"], [""]])(
    "refuses %s",
    (p) => expect(() => checkPattern(p)).toThrow(),
  );
  test("allows ordinary patterns", () => {
    expect(() => checkPattern("src/**/*.ts")).not.toThrow();
    expect(() => checkPattern("*.md")).not.toThrow();
  });
});

describe("code tools", () => {
  test("code_read numbers lines and lists directories without .git", () => {
    expect(codeRead(root, { path: "src/main.ts" })).toBe(
      "1\tconst total = price * qty;\n2\texport {};\n3\t",
    );
    const listing = codeRead(root, { path: "." });
    expect(listing).toContain("src/");
    expect(listing).not.toContain(".git");
    expect(() => codeRead(root, { path: "../secret.txt" })).toThrow(/outside/);
  });

  test("code_read pages long files", () => {
    const out = codeRead(root, { path: "src/main.ts", offset: 2, limit: 1 });
    expect(out).toBe("2\texport {};\n(lines 2-2 of 3; pass offset to read on)");
  });

  test("code_glob finds files under the root only, never in .git", () => {
    expect(codeGlob(root, { pattern: "**/*.ts" })).toBe("src/main.ts\nsrc/nested/deep.ts");
    expect(codeGlob(root, { pattern: "**/*" })).not.toContain(".git");
    expect(() => codeGlob(root, { pattern: "../*" })).toThrow();
    expect(() => codeGlob(root, { pattern: "*", path: ".." })).toThrow(/outside/);
  });

  test("code_grep reports file:line and stays inside", () => {
    expect(codeGrep(root, { pattern: "discount" })).toBe(
      "src/nested/deep.ts:1: // discount applied here",
    );
    expect(codeGrep(root, { pattern: "SECRET" })).toBe("(no matches)");
    expect(() => codeGrep(root, { pattern: "x", path: "../" })).toThrow(/outside/);
    expect(() => codeGrep(root, { pattern: "(" })).toThrow(/invalid regex/);
  });
});

describe("the extension", () => {
  type Registered = { name: string; execute: (id: string, p: unknown) => Promise<unknown> };
  function load(withRoot: boolean) {
    const tools: Registered[] = [];
    let gate: ((c: { toolName: string }) => unknown) | undefined;
    const z: any = new Proxy(() => z, { get: () => z, apply: () => z });
    const prev = process.env[ROOT_ENV];
    if (withRoot) process.env[ROOT_ENV] = root;
    else delete process.env[ROOT_ENV];
    try {
      harnessChatGuard({
        zod: z,
        on: (_e, h) => {
          gate = h as typeof gate;
        },
        registerTool: (def) => tools.push(def as unknown as Registered),
      });
    } finally {
      if (prev === undefined) delete process.env[ROOT_ENV];
      else process.env[ROOT_ENV] = prev;
    }
    return { tools, gate: gate! };
  }

  test("blocks every tool but the chat's own", () => {
    const { gate } = load(false);
    for (const t of [
      "mcp__harness_run_list",
      "mcp__sentry_search_issues",
      "mcp__grafana_query_prometheus",
      "code_read",
      "think",
      "yield",
    ]) {
      expect(gate({ toolName: t })).toBeUndefined();
      expect(isAllowedTool(t)).toBe(true);
    }
    for (const t of ["read", "bash", "edit", "write", "grep", "learn", "manage_skill", "mcp__other_x"]) {
      expect(gate({ toolName: t })).toEqual({ block: true, reason: expect.any(String) });
    }
  });

  test("registers the code tools only when the turn has a checkout", async () => {
    expect(load(false).tools).toHaveLength(0);
    const { tools } = load(true);
    expect(tools.map((t) => t.name).sort()).toEqual(["code_glob", "code_grep", "code_read"]);
    const read = tools.find((t) => t.name === "code_read")!;
    const ok = (await read.execute("1", { path: "src/main.ts" })) as { isError?: boolean };
    expect(ok.isError).toBeUndefined();
    const refused = (await read.execute("2", { path: "/proc/self/environ" })) as {
      isError?: boolean;
      content: { text: string }[];
    };
    expect(refused.isError).toBe(true);
  });
});
