# Single-Task Implementation Protocol

Apply this protocol to exactly the task ID dispatched by Ralph.

## Task Brief

Before editing code, read the relevant project baseline in `docs/PRD.md` and the task's referenced sections in `requirements.md` and `design.md`. Record:

- observable behavior required;
- files or components allowed by `_Boundary:_`;
- completion evidence;
- `_Depends:_` tasks that must already be complete;
- relevant repository test, build, lint, and smoke commands.

Stop if the approved spec conflicts with the codebase or does not determine the required behavior. Add `_Blocked: <reason>_` to the selected task instead of inventing a workaround.

## Execution

For behavioral work, use one RED → GREEN → REFACTOR slice at a time:

1. Write a focused test derived from the selected acceptance criteria.
2. Run it and preserve the expected failing output.
3. Implement only enough code to make that test pass.
4. Refactor without changing scope and rerun the test.
5. Run the task-relevant regression, lint, type, build, and smoke checks.

Documentation or configuration-only work may skip RED when no observable executable behavior changes, but still requires mechanical verification.

Never implement another task, perform unrelated refactoring, use a destructive Git reset, or edit `.ralph-tui/generated/*.json`.

## Review and Completion

1. Read and apply `review.md` against the actual diff. A task cannot proceed with a missing or `REJECTED` verdict.
2. Repair only concrete review findings, with at most two review rounds. If still rejected, mark the task blocked and stop.
3. Read and apply `verify-completion.md` using fresh evidence from the current code state.
4. Only after `APPROVED` and `VERIFIED`, change the selected checkbox to `[x]`.
5. Re-read `tasks.md` and confirm no other executable task was newly checked.

Ralph owns iteration state. Do not claim another task, create a second task list, or emit its completion promise before every gate succeeds.
