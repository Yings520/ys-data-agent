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
- Never claim success from Agent prose or Ralph's completion token alone; use fresh command evidence.

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
- Features use cc-sdd requirements, design, and tasks; Ralph TUI then selects the next generated work item and starts Codex serially.
- `scripts/codex-ralph` is the compatibility command for Ralph TUI 0.12 and Codex CLI 0.151: Ralph full-auto is disabled, the wrapper supplies Codex's current `-a never` global flag, and raw JSONL bypasses RTK so Ralph can parse it. Re-run `rtk ralph-tui doctor` after either tool is upgraded.
- Node.js ESM tooling under `tools/workflow/` compiles cc-sdd tasks into Ralph JSON without third-party packages.
