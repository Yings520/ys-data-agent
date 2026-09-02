# Technology Steering

## Stack

- Language: Rust 2024 edition.
- Runtime: Tokio.
- Workspace: Cargo resolver 3.
- Interfaces: local TUI and CLI in `apps/ysda`.
- Persistence: SQLite runtime/artifact state; PostgreSQL is a supported query source.
- Safety: workspace lint `unsafe_code = "forbid"`.

## Development Rules

- Prefix repository shell commands with `rtk`.
- Preserve typed domain boundaries and explicit failure states.
- Do not add credentials, business rows, prompts, SQL results, or other sensitive values to source, tests, logs, telemetry, or committed fixtures.
- Prefer focused tests during a task and the canonical full gates before feature completion.
- Never claim success from Agent prose or a checked task alone; use fresh command evidence.

## Canonical Quality Commands

Task-local selection depends on the changed crate, but the feature-level baseline is:

```bash
rtk cargo fmt --all --check
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk cargo test --workspace
```

The production-like release gate is:

```bash
rtk bash scripts/v0.2-release-gate.sh
```

It additionally exercises PostgreSQL integration, query evals, model protocol behavior, doctor, export, TUI, and renderer dependency/boundary checks.

## Workflow Toolchain

- BMAD owns only `docs/PRD.md`, the whole-project product, stable architecture, and evolution Source of Truth.
- Small Changes go directly to one bounded Code Agent and still require tests, diff review, and fresh verification.
- Features use cc-sdd requirements, design, and tasks; `$kiro-impl` reads the approved `tasks.md` graph directly and executes dependency-ready tasks serially.
- Node.js ESM tooling under `tools/workflow/` validates authoritative cc-sdd task state and enforces atomic task publication without third-party packages.
- Each successful task produces one commit and one ordinary push on `feat/<feature>` and reuses the shared Draft Feature PR. Final validation may mark it Ready but never merges it.
