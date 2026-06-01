# ai-harness

A Rust-native AI coding harness that runs **in Kubernetes**, is driven by **AI
triggers or cron-polled Linear tasks**, executes **user-authored workflow DAGs**,
and provisions project **toolchains from the UI** (no hand-built wrapper images).

It is seeded from [majiayu000/harness](https://github.com/majiayu000/harness)
(MIT) and reuses its agent adapters, worktree isolation, Postgres runtime, and
web shell, while replacing the workflow front-end with an Archon-style DAG engine
and re-targeting execution at a cluster.

## Documentation

- [docs/PLAN.md](docs/PLAN.md) — architecture & design decisions
- [docs/PHASES.md](docs/PHASES.md) — phased build roadmap

## Status

Early. Phase 0 (seed from majiayu-harness, green build) in progress.

## Acknowledgements

Built on [majiayu000/harness](https://github.com/majiayu000/harness) (MIT) — see
[LICENSE](LICENSE).
