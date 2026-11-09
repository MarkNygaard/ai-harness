#!/usr/bin/env node
// Record what an agent CLI really prints, as a test fixture.
//
//   node scripts/record-cli-fixture.mjs <claude|omp|cursor> <simple|tool-call> [--model <id>]
//
// Runs the CLI with the flags its adapter passes, on a fixed prompt, in a
// scratch directory, and writes stdout to testdata/cli/<provider>/<scenario>.jsonl.
// Tests replay those files through the real parsers. See testdata/cli/README.md.
//
// Plain JavaScript with no dependencies, so it runs under `node` or `bun`.

import { spawnSync } from "node:child_process";
import { mkdtempSync, mkdirSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

// The text the tool-call scenario's file holds. Tests assert the agent's reply
// contains it, which proves the tool ran and its result reached the answer.
const MARKER = "fixture-marker-7f3a";

const SCENARIOS = {
  simple: {
    prompt: "Reply with exactly the word pong and nothing else.",
    setup: () => {},
  },
  "tool-call": {
    prompt:
      "Use your file-reading tool to read notes.txt in the current directory, " +
      "then reply with exactly its contents and nothing else.",
    setup: (dir) => writeFileSync(join(dir, "notes.txt"), `${MARKER}\n`),
  },
};

// Each provider's binary, default model and flags. Keep these in step with the
// adapters: `base_args` in harness-agents/src/claude.rs, `build_args` in
// harness-runner/src/pi.rs and harness-runner/src/cursor.rs.
const PROVIDERS = {
  claude: {
    bin: process.env.CLAUDE_CLI ?? "claude",
    model: "haiku",
    args: (prompt, model) => [
      "-p", prompt,
      "--output-format", "stream-json",
      "--model", model,
      "--verbose",
      // The adapter passes --dangerously-skip-permissions. The scenarios only
      // read a file, so the recording allows just that; the stream format is
      // the same either way.
      "--allowedTools", "Read",
      // Not passed by the adapter. They keep the recording's own user settings,
      // hooks and MCP servers out of the fixture without changing the event format.
      "--setting-sources", "project",
      "--strict-mcp-config",
    ],
  },
  omp: {
    bin: process.env.OMP_CLI ?? process.env.OMP_PATH ?? "omp",
    model: "kimi-code/kimi-for-coding",
    args: (prompt, model) => ["-p", prompt, "--mode", "json", "--model", model],
  },
  cursor: {
    bin: process.env.CURSOR_AGENT_CLI ?? "cursor-agent",
    model: "composer-2.5",
    args: (prompt, model) => [
      "-p", prompt,
      "--output-format", "json",
      "--model", model,
      "--force",
      "--trust",
    ],
  },
};

// Fields of Claude's `system`/`init` line worth keeping. The rest lists the
// recording machine's tools, slash commands, skills, plugins and agents.
const CLAUDE_INIT_KEYS = ["type", "subtype", "session_id", "model", "cwd", "permissionMode", "uuid"];

function usage(message) {
  if (message) console.error(message);
  console.error(
    "usage: node scripts/record-cli-fixture.mjs <claude|omp|cursor> <simple|tool-call> [--model <id>]",
  );
  process.exit(2);
}

function parseArgs(argv) {
  const [provider, scenario, ...rest] = argv;
  if (!PROVIDERS[provider]) usage(`unknown provider: ${provider ?? "(none)"}`);
  if (!SCENARIOS[scenario]) usage(`unknown scenario: ${scenario ?? "(none)"}`);
  let model = PROVIDERS[provider].model;
  for (let i = 0; i < rest.length; i++) {
    if (rest[i] === "--model" && rest[i + 1]) model = rest[++i];
    else usage(`unexpected argument: ${rest[i]}`);
  }
  return { provider, scenario, model };
}

// The adapters strip these before spawning (`strip_claude_env`,
// `strip_control_plane_env`), so the recording does too.
function childEnv() {
  const env = { ...process.env };
  for (const key of Object.keys(env)) {
    if (key.startsWith("CLAUDE") || key.startsWith("HARNESS_") || key === "DATABASE_URL") {
      delete env[key];
    }
  }
  return env;
}

// Every spelling a path can take in the output: as is, with forward slashes,
// and JSON-escaped (doubled backslashes on Windows).
function spellings(path) {
  const forward = path.replaceAll("\\", "/");
  const escaped = path.replaceAll("\\", "\\\\");
  return [...new Set([escaped, path, forward])].sort((a, b) => b.length - a.length);
}

function scrub(stdout, provider, workDir) {
  const replacements = [
    ...spellings(workDir).map((s) => [s, "/work"]),
    ...spellings(homedir()).map((s) => [s, "/home/user"]),
  ];
  return stdout
    .split(/\r?\n/)
    .filter((line) => line.trim() !== "")
    .map((line) => {
      let out = line;
      for (const [from, to] of replacements) out = out.replaceAll(from, to);
      // A Windows recording joins the rest of a path with `\`: `/work\\notes.txt`.
      out = out.replaceAll("/work\\\\", "/work/").replaceAll("/home/user\\\\", "/home/user/");
      if (provider === "claude") out = pruneClaudeInit(out);
      return out;
    })
    .join("\n") + "\n";
}

function pruneClaudeInit(line) {
  let v;
  try {
    v = JSON.parse(line);
  } catch {
    return line;
  }
  if (v.type !== "system" || v.subtype !== "init") return line;
  const kept = {};
  for (const key of CLAUDE_INIT_KEYS) if (key in v) kept[key] = v[key];
  return JSON.stringify(kept);
}

// npm installs CLIs as `.cmd` shims on Windows, which only run through a shell,
// and a shell joins arguments unquoted. Quote them there; pass them as is elsewhere.
const WINDOWS = process.platform === "win32";
function shellArgs(args) {
  return WINDOWS ? args.map((a) => `"${a.replaceAll('"', '\\"')}"`) : args;
}

function version(bin) {
  const r = spawnSync(bin, ["--version"], { encoding: "utf8", shell: WINDOWS });
  return (r.stdout || r.stderr || "unknown").trim().split(/\r?\n/)[0];
}

function main() {
  const { provider, scenario, model } = parseArgs(process.argv.slice(2));
  const p = PROVIDERS[provider];
  const s = SCENARIOS[scenario];

  // realpath: on macOS the temp dir is a symlink, and the CLI reports the real path.
  const workDir = realpathSync(mkdtempSync(join(tmpdir(), "cli-fixture-")));
  try {
    s.setup(workDir);
    console.error(`recording ${provider}/${scenario} with model ${model} in ${workDir}`);
    const r = spawnSync(p.bin, shellArgs(p.args(s.prompt, model)), {
      cwd: workDir,
      env: childEnv(),
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      maxBuffer: 64 * 1024 * 1024,
      shell: WINDOWS,
    });
    if (r.error) throw r.error;
    if (r.status !== 0) {
      console.error(r.stderr);
      console.error(`${provider} exited with ${r.status}; nothing written`);
      process.exit(1);
    }

    const outDir = join(ROOT, "testdata", "cli", provider);
    mkdirSync(outDir, { recursive: true });
    const outFile = join(outDir, `${scenario}.jsonl`);
    writeFileSync(outFile, scrub(r.stdout, provider, workDir));
    writeFileSync(
      join(outDir, `${scenario}.version`),
      `${version(p.bin)}\nmodel: ${model}\nrecorded: ${new Date().toISOString().slice(0, 10)}\n`,
    );
    console.error(`wrote ${outFile}`);
  } finally {
    rmSync(workDir, { recursive: true, force: true });
  }
}

main();
