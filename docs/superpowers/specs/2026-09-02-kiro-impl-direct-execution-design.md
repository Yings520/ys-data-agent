# Direct cc-sdd Execution with `kiro-impl`

## Goal

Replace Ralph TUI with a repository-local Codex Skill named `kiro-impl`.
After `kiro-spec-tasks` generates and a human approves a Feature's `tasks.md`, the
user starts implementation with:

```text
$kiro-impl <feature>
```

The Skill consumes the approved cc-sdd documents directly. It introduces no
generated task projection, scheduler database, iteration budget, or second task
state.

## Sources of Truth

- `docs/PRD.md` remains the project-level product and architecture source.
- `.kiro/specs/<feature>/requirements.md` and `design.md` remain the approved
  Feature contract.
- `.kiro/specs/<feature>/tasks.md` remains the sole implementation graph and
  completion state.
- `.kiro/specs/<feature>/spec.json` remains the approval gate.

`kiro-impl` must not redefine task descriptions, dependencies, boundaries, or
acceptance conditions while executing them.

## Architecture

### `kiro-spec-tasks`

`kiro-spec-tasks` continues to generate and review the authoritative task graph.
Its handoff changes from a Ralph launcher to the exact next invocation:

```text
$kiro-impl <feature>
```

### `kiro-impl`

`kiro-impl` is a multi-task cc-sdd execution Skill. In one Codex session it:

1. loads the approved spec, steering, mandatory Rust policy, and code-change/PR
   policy;
2. parses every leaf task and `_Depends:_` edge from `tasks.md`;
3. selects the first unchecked task whose dependencies are complete;
4. completes exactly that task through RED, minimal implementation, GREEN,
   scoped review, and fresh verification;
5. checks only that task in `tasks.md`;
6. creates one task commit, pushes the Feature branch, and creates or reuses one
   Draft Feature PR;
7. repeats from the updated `tasks.md` until no normal task remains;
8. runs the final Feature validation gate and marks the shared PR Ready only
   when every approved requirement and design contract passes.

Normal execution is sequential. `(P)` remains task-authoring metadata and does
not cause concurrent working-tree mutation.

### Publication

Publication remains repository-controlled rather than dependent on prose from
the Agent. A helper validates the exact staged paths, current Feature branch,
single-task checkbox transition, task trailers, push result, and shared PR
identity. Ralph-specific dispatch environment variables and completion
sentinels are removed.

Each normal task produces one commit and one ordinary push. All task commits use
the same `feat/<feature>` branch and the same Draft PR. The final validation gate
may mark that PR Ready but never merges it.

## Resume and Failure Semantics

`tasks.md` is the durable checkpoint. On test, review, publication, or dependency
failure, `kiro-impl` stops without checking the current task. Re-running the same
command re-reads the repository and resumes at the first eligible unchecked
task.

An unchecked task with incomplete dependencies is skipped. If unchecked tasks
remain but none is eligible, execution stops with a dependency-cycle or blocked
graph report. Existing unrelated working-tree changes are preserved and cannot
be staged by the publication helper.

## Ralph Removal

Remove the Ralph launcher, Codex compatibility wrapper, prompt template,
projection generator, TUI configuration/runtime state, Ralph-only completion
sentinel, Ralph-specific tests, and current documentation references. Rename or
replace reusable task parsing and publication code with cc-sdd/`kiro-impl`
terminology.

Historical Ralph runtime logs under `.ralph-tui/` are disposable and are deleted.
The in-progress provider-management task 2.1 implementation is product work and
is explicitly preserved.

## Verification

Automated workflow tests must demonstrate:

- `kiro-spec-tasks` hands off to `$kiro-impl <feature>`;
- execution is denied unless requirements, design, and tasks are approved;
- dependency ordering and resume selection come only from `tasks.md`;
- a failed task remains unchecked and stops the loop;
- each successful task changes exactly one leaf checkbox and produces one
  correctly attributed commit;
- undeclared files, runtime state, dotenv files, and secret-like files cannot be
  published;
- one Draft PR is reused and validation alone can mark it Ready;
- no live project documentation, Skill, script, or test retains a Ralph runtime
  dependency.

Fresh repository tests, formatting, linting, diff review, and a dry-run fixture
Feature close the migration.
