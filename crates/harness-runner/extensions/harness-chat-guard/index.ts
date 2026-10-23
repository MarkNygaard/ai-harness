/**
 * harness-chat-guard — the omp extension every Pi chat turn runs with.
 *
 * Loaded with `--trusted-extension`, which (a) disables discovery of every
 * other extension and (b) fails the turn if this one does not load, so the
 * guard is either in force or nothing runs.
 *
 * It does two things:
 *
 * 1. **Allows only the chat's own tools.** Every tool call is checked: the
 *    harness MCP tools, the `code_*` tools below, and omp's internal
 *    `think`/`yield` pass; anything else is blocked. omp force-activates a few
 *    tools even under an explicit `--tools` list (`learn`, `manage_skill`,
 *    which write skill files that later workflow runs would load), and this is
 *    what stops them.
 * 2. **Reads code inside one directory.** When the turn has a checkout
 *    (`HARNESS_CHAT_CODE_ROOT`), it registers `code_read`, `code_grep` and
 *    `code_glob`, implemented here. omp's own read/grep/glob accept any path
 *    and nothing confines them, so they stay off and these replace them:
 *    every path is resolved — through symlinks — and refused unless it lands
 *    inside the checkout. Anything that can't be understood is refused.
 */
import * as fs from "node:fs";
import * as path from "node:path";

/** The checkout the code tools may read. Set by the harness, never the model. */
export const ROOT_ENV = "HARNESS_CHAT_CODE_ROOT";

const MAX_READ_LINES = 2000;
const MAX_READ_BYTES = 5 * 1024 * 1024;
const MAX_GREP_MATCHES = 200;
const MAX_GREP_FILE_BYTES = 1024 * 1024;
const MAX_GLOB_RESULTS = 500;
const MAX_LINE_CHARS = 500;

/** Tools a chat turn may call besides the MCP servers below. */
const ALLOWED_TOOLS = new Set(["code_read", "code_grep", "code_glob", "think", "yield"]);
/**
 * MCP servers a chat turn may use: the harness's own, and Sentry's and
 * Grafana's — which the harness runs read-only (read-only credentials, and
 * Sentry's `inspect` skill / Grafana's `--disable-write`).
 */
const MCP_PREFIXES = ["mcp__harness_", "mcp__sentry_", "mcp__grafana_"];

export function isAllowedTool(name: string): boolean {
  return MCP_PREFIXES.some((p) => name.startsWith(p)) || ALLOWED_TOOLS.has(name);
}

/** The deepest existing ancestor of `p`, resolved through symlinks, plus the rest. */
function realpathLoose(p: string): string {
  let existing = p;
  const rest: string[] = [];
  while (!fs.existsSync(existing)) {
    const parent = path.dirname(existing);
    if (parent === existing) break;
    rest.unshift(path.basename(existing));
    existing = parent;
  }
  return path.join(fs.realpathSync(existing), ...rest);
}

/**
 * Resolve `requested` against `root` and return its real path, or throw if it
 * is not inside `root`. Deliberately narrow: no URLs or internal URIs, no
 * selectors, no `~` — a plain relative (or in-root absolute) path only.
 */
export function confine(root: string, requested: string | undefined): string {
  const realRoot = fs.realpathSync(root);
  const raw = (requested ?? "").trim() || ".";
  if (raw.includes("\0")) throw new Error("path contains a NUL byte");
  if (raw.includes(":")) {
    throw new Error(`\`${raw}\` is not a plain path (no URLs, schemes or selectors)`);
  }
  if (raw.startsWith("~")) throw new Error(`\`${raw}\`: home-relative paths are not allowed`);
  const candidate = path.isAbsolute(raw) ? raw : path.resolve(realRoot, raw);
  const real = realpathLoose(path.resolve(candidate));
  const rel = path.relative(realRoot, real);
  if (rel === ".." || rel.startsWith(`..${path.sep}`) || path.isAbsolute(rel)) {
    throw new Error(`\`${raw}\` is outside the project checkout`);
  }
  return real;
}

/** A glob pattern must stay relative and never climb. */
export function checkPattern(pattern: string): void {
  const p = pattern.trim();
  if (!p) throw new Error("empty pattern");
  if (p.includes("\0") || p.includes(":")) throw new Error(`\`${p}\` is not a plain pattern`);
  if (path.isAbsolute(p) || p.startsWith("~") || /^[\\/]/.test(p)) {
    throw new Error(`\`${p}\`: patterns are relative to the checkout`);
  }
  if (p.split(/[\\/]/).some((seg) => seg === "..")) {
    throw new Error(`\`${p}\`: \`..\` is not allowed in a pattern`);
  }
}

/** Paths under `dir` matching `pattern`, relative to `root`, `.git` excluded. */
function scan(root: string, dir: string, pattern: string, limit: number): string[] {
  checkPattern(pattern);
  const out: string[] = [];
  const glob = new Bun.Glob(pattern);
  for (const rel of glob.scanSync({ cwd: dir, onlyFiles: true, followSymlinks: false, dot: true })) {
    const abs = path.join(dir, rel);
    const fromRoot = path.relative(root, abs);
    if (fromRoot.startsWith("..") || path.isAbsolute(fromRoot)) continue;
    if (fromRoot.split(path.sep).includes(".git")) continue;
    out.push(fromRoot.split(path.sep).join("/"));
    if (out.length >= limit) break;
  }
  return out.sort();
}

function looksBinary(buf: Uint8Array): boolean {
  const n = Math.min(buf.length, 8000);
  for (let i = 0; i < n; i++) if (buf[i] === 0) return true;
  return false;
}

function clip(line: string): string {
  return line.length > MAX_LINE_CHARS ? `${line.slice(0, MAX_LINE_CHARS)}…` : line;
}

export function codeRead(
  root: string,
  params: { path: string; offset?: number; limit?: number },
): string {
  const abs = confine(root, params.path);
  const realRoot = fs.realpathSync(root);
  const stat = fs.statSync(abs);
  if (stat.isDirectory()) {
    const entries = fs
      .readdirSync(abs, { withFileTypes: true })
      .filter((e) => e.name !== ".git")
      .map((e) => (e.isDirectory() ? `${e.name}/` : e.name))
      .sort();
    const rel = path.relative(realRoot, abs) || ".";
    return `${rel}/ (directory)\n${entries.join("\n")}`;
  }
  if (stat.size > MAX_READ_BYTES) throw new Error(`file is ${stat.size} bytes; too large to read`);
  const buf = fs.readFileSync(abs);
  if (looksBinary(buf)) return "(binary file)";
  const lines = buf.toString("utf8").split("\n");
  const start = Math.max(1, Math.floor(params.offset ?? 1));
  const count = Math.min(MAX_READ_LINES, Math.max(1, Math.floor(params.limit ?? MAX_READ_LINES)));
  const slice = lines.slice(start - 1, start - 1 + count);
  const body = slice.map((l, i) => `${start + i}\t${clip(l)}`).join("\n");
  const end = start - 1 + slice.length;
  return end < lines.length
    ? `${body}\n(lines ${start}-${end} of ${lines.length}; pass offset to read on)`
    : body;
}

export function codeGlob(root: string, params: { pattern: string; path?: string }): string {
  const realRoot = fs.realpathSync(root);
  const dir = confine(root, params.path);
  const found = scan(realRoot, dir, params.pattern, MAX_GLOB_RESULTS);
  if (found.length === 0) return "(no files match)";
  return found.length >= MAX_GLOB_RESULTS
    ? `${found.join("\n")}\n(first ${MAX_GLOB_RESULTS} shown; narrow the pattern)`
    : found.join("\n");
}

export function codeGrep(
  root: string,
  params: { pattern: string; path?: string; glob?: string; ignore_case?: boolean },
): string {
  const realRoot = fs.realpathSync(root);
  const target = confine(root, params.path);
  let re: RegExp;
  try {
    re = new RegExp(params.pattern, params.ignore_case ? "i" : "");
  } catch (e) {
    throw new Error(`invalid regex: ${(e as Error).message}`);
  }
  const files = fs.statSync(target).isFile()
    ? [path.relative(realRoot, target).split(path.sep).join("/")]
    : scan(realRoot, target, params.glob?.trim() || "**/*", Number.MAX_SAFE_INTEGER);
  const out: string[] = [];
  for (const rel of files) {
    const abs = path.join(realRoot, rel);
    let stat: fs.Stats;
    try {
      stat = fs.statSync(abs);
    } catch {
      continue;
    }
    if (stat.size > MAX_GREP_FILE_BYTES) continue;
    const buf = fs.readFileSync(abs);
    if (looksBinary(buf)) continue;
    const lines = buf.toString("utf8").split("\n");
    for (let i = 0; i < lines.length; i++) {
      if (re.test(lines[i])) {
        out.push(`${rel}:${i + 1}: ${clip(lines[i].trim())}`);
        if (out.length >= MAX_GREP_MATCHES) {
          return `${out.join("\n")}\n(first ${MAX_GREP_MATCHES} matches shown; narrow the search)`;
        }
      }
    }
  }
  return out.length ? out.join("\n") : "(no matches)";
}

type ToolText = { content: { type: "text"; text: string }[]; isError?: boolean };

function run(fn: () => string): ToolText {
  try {
    return { content: [{ type: "text", text: fn() }] };
  } catch (e) {
    return { content: [{ type: "text", text: (e as Error).message }], isError: true };
  }
}

// omp's ExtensionAPI, narrowed to what this uses.
interface Api {
  zod: any;
  on(
    event: "tool_call",
    handler: (call: { toolName: string }) => { block: boolean; reason: string } | void,
  ): void;
  registerTool(def: Record<string, unknown>): void;
}

export default function harnessChatGuard(pi: Api): void {
  pi.on("tool_call", (call) => {
    if (!isAllowedTool(call.toolName)) {
      return {
        block: true,
        reason: `\`${call.toolName}\` is not available in the harness chat`,
      };
    }
  });

  const root = process.env[ROOT_ENV];
  if (!root) return;
  const z = pi.zod;
  const common = { approval: "read", loadMode: "essential" };

  pi.registerTool({
    ...common,
    name: "code_read",
    label: "Read code",
    description:
      "Read a file (or list a directory) in the project checkout. Paths are relative to the checkout root. Returns numbered lines; use offset/limit for long files.",
    parameters: z.object({
      path: z.string().describe("File or directory, relative to the checkout root"),
      offset: z.number().optional().describe("First line to return (1-based)"),
      limit: z.number().optional().describe("How many lines (max 2000)"),
    }),
    async execute(_id: string, params: { path: string; offset?: number; limit?: number }) {
      return run(() => codeRead(root, params));
    },
  });

  pi.registerTool({
    ...common,
    name: "code_grep",
    label: "Search code",
    description:
      "Search the project checkout for a regular expression. Returns `file:line: text`, at most 200 matches. Narrow with `path` (a directory or file) and `glob` (e.g. `**/*.ts`).",
    parameters: z.object({
      pattern: z.string().describe("JavaScript regular expression"),
      path: z.string().optional().describe("Directory or file to search, relative to the root"),
      glob: z.string().optional().describe("Only files matching this glob, e.g. **/*.rs"),
      ignore_case: z.boolean().optional(),
    }),
    async execute(
      _id: string,
      params: { pattern: string; path?: string; glob?: string; ignore_case?: boolean },
    ) {
      return run(() => codeGrep(root, params));
    },
  });

  pi.registerTool({
    ...common,
    name: "code_glob",
    label: "Find files",
    description:
      "List files in the project checkout matching a glob, e.g. `src/**/*.ts`. Paths are relative to the root; at most 500.",
    parameters: z.object({
      pattern: z.string().describe("Glob, relative to `path` (or the root)"),
      path: z.string().optional().describe("Directory to search from, relative to the root"),
    }),
    async execute(_id: string, params: { pattern: string; path?: string }) {
      return run(() => codeGlob(root, params));
    },
  });
}
