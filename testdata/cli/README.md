# Recorded agent CLI output

Each file here is what an agent CLI printed on a real run, recorded by
`scripts/record-cli-fixture.mjs`. The adapter tests replay them through the
real parsers. A frame written by hand only shows what we think a CLI prints,
and a test built on one keeps passing after the CLI changes its format.

| Folder | CLI | Replayed by |
| --- | --- | --- |
| `claude/` | Claude Code (`claude -p --output-format stream-json`) | `harness-agents/src/claude_stream.rs`, module `recorded_fixtures` |
| `omp/` | omp (`omp -p --mode json`) | `harness-runner/src/pi.rs`, module `recorded_fixtures` |

cursor-agent has no recordings yet. The script can record it
(`node scripts/record-cli-fixture.mjs cursor simple`, with `CURSOR_API_KEY`
set); its replay tests would go in `harness-runner/src/cursor.rs`.

## Scenarios

- `simple`: the agent is asked to reply with the word `pong`.
- `tool-call`: the agent reads `notes.txt` with its file tool and replies with
  the file's contents, `fixture-marker-7f3a`.

Each `<scenario>.jsonl` has a `<scenario>.version` next to it, giving the CLI
version, the model and the date of the recording.

## Re-recording

Re-record when a CLI update changes its output, or to add a scenario. Run from
the repository root, on a machine where the CLI is installed and signed in:

```bash
node scripts/record-cli-fixture.mjs claude simple
node scripts/record-cli-fixture.mjs claude tool-call
node scripts/record-cli-fixture.mjs omp tool-call --model kimi-code/kimi-for-coding
```

omp signs in through `/login` in its interactive screen; run `omp` once and
log in before recording. `bun` works in place of `node`. The script runs the CLI with the flags its
adapter passes, in a scratch directory, and overwrites the fixture. `--model`
picks a model; each provider defaults to the adapter's default, except Claude,
which records with `haiku` to keep it cheap. `CLAUDE_CLI`, `OMP_CLI` and
`CURSOR_AGENT_CLI` point the script at a binary that is not on `PATH`.

The recording differs from a real run in two ways:

- For Claude it allows only the `Read` tool, where the adapter passes
  `--dangerously-skip-permissions`. It also loads no user settings or MCP
  servers. The stream format is the same either way.
- It replaces the scratch directory with `/work` and the home directory with
  `/home/user`, and trims Claude's `init` line to a few fields. Read a new
  recording before committing it, in case the CLI prints something else that
  belongs to the machine or the account.

Then run the replay tests. A failure after re-recording means the CLI's output
changed in a way the parser has to handle.

```bash
cargo test -p harness-agents recorded_fixtures
cargo test -p harness-runner recorded_fixtures
```
