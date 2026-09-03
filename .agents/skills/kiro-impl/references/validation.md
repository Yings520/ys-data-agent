# Feature Integration Validation Protocol

Use this protocol only after the task-state selector returns `VALIDATE` and every
executable task is `[x]`.

## Inputs

Read `docs/PRD.md`, `spec.json`, `requirements.md`, `design.md`, `tasks.md`,
relevant steering, and the complete Feature diff. Discover canonical test,
build, lint, and smoke commands from repository automation before README
examples.

## Checks

1. Run the complete canonical test suite and required static/build checks.
2. Run the lightest trustworthy smoke command for the built artifact.
3. Map every numeric requirement to completed implementation evidence.
4. Verify cross-task interfaces, data shapes, shared state, and error contracts
   agree.
5. Compare the final component graph, dependency direction, and file layout with
   `design.md`.
6. Audit task boundaries for spillover, hidden ownership, undeclared coupling,
   placeholders, and secrets.
7. Confirm no `_Blocked:_` executable task remains.
8. Apply `verify-completion.md` to the Feature-level claim.

## Decision

Return:

- `GO` only when every required check passes and completion is `VERIFIED`;
- `NO-GO` for concrete failures, with exact remediation and ownership;
- `MANUAL_VERIFY_REQUIRED` when mandatory evidence cannot be gathered.

Tests alone never justify `GO`.
