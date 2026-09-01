---
name: run-cc-sdd-task
description: Execute exactly one approved cc-sdd task, or the final VALIDATE gate, when Ralph TUI dispatches a generated work item. Enforces projection freshness, selected-task scope, cc-sdd review, and fresh completion evidence.
---

# Run One cc-sdd Task

This is the only implementation entry point Ralph may invoke. The arguments are:

```text
<feature> <task-id>
```

`<feature>` must be the feature named in the Ralph task description. `<task-id>` must be one exact cc-sdd task ID or the reserved ID `VALIDATE`.

## Preflight

1. Reject missing arguments, feature names outside `[a-z0-9._-]`, and task IDs other than a numeric cc-sdd ID or `VALIDATE`.
2. Run `rtk node tools/workflow/cc-sdd-to-ralph.mjs <feature> --check` before reading implementation context. A failure means the Ralph projection is stale, the spec is unapproved, or the task graph is unsafe. Stop without a completion promise.
3. Read `.kiro/specs/<feature>/spec.json` and confirm `approvals.tasks.approved` is `true`.
4. Read `requirements.md`, `design.md`, `tasks.md`, and only the steering documents relevant to the selected boundary.
5. Record `git status --short` before execution. Preserve all pre-existing changes and never use a destructive reset.

## Normal Task IDs

1. Locate exactly `<task-id>` in `tasks.md`. It must be incomplete and all `_Depends:_` tasks must be complete.
2. Read `.agents/skills/kiro-impl/SKILL.md` completely and apply its **Manual Mode** to exactly `<feature> <task-id>`.
3. Do not invoke unscoped `$kiro-impl <feature>` and do not implement, review, or mark any other executable leaf task complete.
4. The selected task is not complete until:
   - the required RED → GREEN → REFACTOR cycle has evidence when behavior changes;
   - task-relevant mechanical checks pass;
   - `kiro-review` returns a parseable `APPROVED` verdict, using a fresh reviewer when the host supports it;
   - `kiro-verify-completion` verifies the completion claim from fresh evidence;
   - the selected checkbox is `[x]` in `tasks.md`.
5. Re-read `tasks.md`. Confirm the selected task is checked and no other executable leaf task was newly checked by this run.
6. Do not run the projection `--check` after marking the task complete: Ralph updates its JSON only after consuming the completion promise. The next iteration performs the consistency check.

## Reserved `VALIDATE` ID

1. Confirm every executable task in `tasks.md` is complete.
2. Read `.agents/skills/kiro-validate-impl/SKILL.md` completely and validate the entire `<feature>`.
3. `NO-GO` or `MANUAL_VERIFY_REQUIRED` is not completion. Record the exact finding and stop.
4. On `GO`, read `.agents/skills/kiro-verify-completion/SKILL.md` completely and verify the feature-level claim with fresh test, build, smoke, traceability, and boundary evidence.

## Ralph Result Protocol

- Only after every applicable gate above succeeds, finish the response with the exact token `<promise>COMPLETE</promise>`.
- On a blocker, spec conflict, stale projection, rejected review, failed command, `NO-GO`, or missing manual evidence, do not print that token anywhere in the response.
- For a blocker, update `tasks.md` with cc-sdd's `_Blocked: <reason>_` annotation when the owning cc-sdd protocol calls for it, then report a concise `STATUS: BLOCKED`, the evidence, and the required human action.
- Never edit `.ralph-tui/generated/*.json`; Ralph owns the scheduling projection during a run.
