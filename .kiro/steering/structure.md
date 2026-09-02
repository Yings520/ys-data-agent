# Structure Steering

## Workspace Boundaries

```text
apps/ysda/                 CLI/TUI composition root and user-facing adapters
crates/ys-agent-core/      Domain types, contracts, policies, and pure rules
crates/ys-agent-runtime/   Task/run coordination, loop driving, workflows
crates/ys-agent-store/     Durable runtime and artifact persistence
crates/ys-agent-adapters/  External model and data-source adapters
fixtures/                  Approved deterministic test fixtures
evals/                     Evaluation definitions and guidance
scripts/                   Repository-wide operational and release gates
tools/workflow/            Development-workflow adapters; never product runtime
```

## Change Rules

- A small Change handled directly by a Code Agent must declare one concrete responsibility boundary and remain within it.
- A Feature's cc-sdd task must declare concrete file or component boundaries.
- Keep UI workflow-free: `apps/ysda` communicates through the existing service API rather than embedding domain workflow logic.
- Keep external protocol/database details in adapters; keep domain policy in core; keep lifecycle coordination in runtime; keep persistence mechanics in store.
- Do not move responsibility across crates merely to make one task easier.
- Do not perform unrelated refactors, formatting sweeps, dependency upgrades, or generated-file rewrites.
- New toolchain integration belongs under `tools/workflow/`, `.agents/skills/`, or `.kiro/`, not product crates.

## Revalidation

Changes to public contracts, persistence schema, runtime event/state shapes, adapter protocols, or workspace dependency direction require revalidation of affected downstream crates and the full workspace test suite.
