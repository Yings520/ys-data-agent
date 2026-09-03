---
name: kiro-impl
description: Execute every remaining task in one approved cc-sdd Feature directly from tasks.md, with dependency ordering, TDD, review, one task commit and push, one shared Feature PR, resumability, and final validation. Use after kiro-spec-tasks approval.
---

# Direct cc-sdd Implementation

Invoke this Skill as:

```text
$kiro-impl <feature>
```

It is the direct cc-sdd execution entry point. `tasks.md` is the only scheduler
and durable progress state; do not create a projection or a second task list.

## Preflight

1. Require exactly one feature name matching `[a-z0-9][a-z0-9._-]*`.
2. Run `rtk node tools/workflow/cc-sdd-task-state.mjs <feature> --check`.
   Stop if requirements, design, or tasks are not approved, or if the task graph
   is invalid or blocked.
3. Read completely:
   - `docs/PRD.md`;
   - `.kiro/specs/<feature>/spec.json`, `requirements.md`, `design.md`, and
     `tasks.md`;
   - relevant `.kiro/steering/` documents;
   - `.ysda/agents/rust-engineer.md`;
   - `.ysda/agents/code-change-pr-workflow.md`.
4. Stop if either mandatory policy file is missing, unreadable, or truncated.
5. Confirm the current branch is exactly `feat/<feature>`. Record
   `rtk git status --short`, including pre-existing changes, and preserve them.
   Do not use destructive reset, clean, checkout, or force-push commands.
6. Ensure the Git index contains no pre-existing staged paths. Recover a prior
   interrupted publication with
   `rtk node tools/workflow/cc-sdd-publish.mjs --recover <feature>` when needed.

## Automatic Task Loop

Repeat these steps until the selector returns `VALIDATE`:

1. Run:

   ```text
   rtk node tools/workflow/cc-sdd-task-state.mjs <feature> --next
   ```

2. Treat the returned numeric task as the only selected task. Do not choose a
   different task, combine tasks, or change dependencies. `(P)` metadata does
   not authorize concurrent working-tree mutation.
3. Read [references/implementation.md](references/implementation.md),
   [references/review.md](references/review.md), and
   [references/verify-completion.md](references/verify-completion.md)
   completely, then execute every applicable gate for the selected task.
4. After `APPROVED` review and `VERIFIED` fresh evidence, change only the
   selected leaf checkbox to `[x]`. Remove only an obsolete `_Blocked:_`
   annotation owned by that task when its documented blocker has actually been
   resolved.
5. Review the exact diff. Stage only reviewed files belonging to the task plus
   `.kiro/specs/<feature>/tasks.md`.
6. Publish one task commit and one ordinary push:

   ```text
   rtk node tools/workflow/cc-sdd-publish.mjs <feature> <task-id> \
     --path <reviewed-path> [--path <reviewed-path> ...]
   ```

   The helper must create or reuse the single Draft Feature PR. It rejects
   undeclared staged files, wrong checkbox transitions, wrong branches, and
   unsafe paths.
7. Re-read `tasks.md`, rerun the selector, and continue automatically. Never
   report the Feature complete after only one task.

On any failed test, rejected review, missing evidence, publication failure,
unsafe graph, or spec conflict, leave the current task unchecked and stop. Add
`_Blocked: <reason>_` only for a durable blocker requiring human or spec action.
A later `$kiro-impl <feature>` invocation resumes from authoritative state.

## Final Validation

When the selector returns `VALIDATE`:

1. Confirm every executable leaf in `tasks.md` is `[x]` and has a durable task
   commit.
2. Read [references/validation.md](references/validation.md) and
   [references/verify-completion.md](references/verify-completion.md)
   completely.
3. Run the complete Feature validation. `NO-GO` and
   `MANUAL_VERIFY_REQUIRED` stop execution and cannot publish validation.
4. On `GO` with a `VERIFIED` feature claim, require an empty Git index and run:

   ```text
   rtk node tools/workflow/cc-sdd-publish.mjs <feature> VALIDATE
   ```

   The helper publishes an audit commit and marks the shared PR Ready. It never
   merges the PR.
5. Report completed task IDs, fresh verification commands, the Feature branch,
   remote head, and PR URL/state.

## Publication Contract

- One task commit per successfully completed leaf task.
- One ordinary push after each task commit.
- One shared Draft PR for the Feature; only successful final validation marks
  it Ready.
- No automatic merge, force push, secret publication, unrelated staging, or
  second completion state.
