---
name: kiro-spec-tasks
description: Decompose approved cc-sdd requirements and design into the sole authoritative executable task list consumed directly by kiro-impl.
metadata:
  shared-rules: "tasks-generation.md, tasks-parallel-analysis.md"
---

# cc-sdd Task Decomposition

Create `.kiro/specs/$1/tasks.md`, the only authoritative implementation graph and completion state. `$kiro-impl` consumes this file directly and must never redefine it.

## Inputs and Approval Gate

Read completely:

- `.kiro/specs/$1/spec.json`, `requirements.md`, and `design.md`;
- the existing `tasks.md`, when updating;
- relevant `.kiro/steering/` files;
- `rules/tasks-generation.md` and, unless `--sequential` is requested, `rules/tasks-parallel-analysis.md`;
- `.kiro/settings/templates/specs/tasks.md`.

Requirements and design must be human-approved in `spec.json`. An explicit `-y` may record intentional fast-track approvals; otherwise stop when either approval is absent.

## Procedure

1. Build a draft task graph in working context. Do not write it yet.
2. Map every numeric requirement ID and every design component, contract, runtime prerequisite, integration point, and verification concern to at least one task.
3. Keep executable leaf tasks small enough for one bounded Agent run, normally 1–3 hours:
   - one responsibility boundary per normal task;
   - cross-boundary work becomes an explicit integration task;
   - state a concrete observable done condition;
   - declare non-obvious `_Depends:_` edges;
   - use `_Boundary:_` and `(P)` only when ownership and parallel safety are clear.
4. Include implementation and automated-test work. Exclude product planning, sprint ceremony, marketing, manual user testing, and unrelated documentation.
5. Run the Task Plan Review Gate from `rules/tasks-generation.md`. Repair local issues for at most two passes.
6. Perform one fresh task-graph sanity pass, using a separate review context when available. Check only hidden prerequisites, ordering, boundary overlap, task size, verifiability, and contradictions with requirements/design.
7. If the graph reveals a real spec gap, stop and return to requirements or design. Do not hide the gap in a vague task.
8. Write `.kiro/specs/$1/tasks.md` only after both checks pass.
9. Update `spec.json`:
   - `phase: "tasks-generated"`;
   - requirements and design approvals remain `true`;
   - `approvals.tasks.generated: true` and `approved: false`;
   - refresh `updated_at`.
10. Present the task summary and ask the user to approve `tasks.md`. On explicit approval, set `approvals.tasks.approved: true`.

## Direct Implementation Handoff

After task approval, report this exact next invocation:

```text
$kiro-impl $1
```

Never instruct the user or an Agent to bypass the approved task graph with an unscoped implementation command. During execution, `$kiro-impl` selects tasks by `_Depends:_`, writes completion state only to `tasks.md`, and resumes from that authoritative state.

## Output

Report the task count, requirement/design coverage, dependency and boundary review verdicts, approval state, and the exact next command. Keep the response concise.
