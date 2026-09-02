# Ralph Task Policy Loading and Publishing Design

## Context

Ralph TUI executes one approved `provider-management` cc-sdd task per
iteration through `$run-cc-sdd-task`. Two repository policies currently exist
only as unreferenced Markdown files:

- `.ysda/agents/rust-engineer.md`
- `.ysda/agents/code-change-pr-workflow.md`

Ralph must load both documents as mandatory execution constraints. The Feature
also needs one shared branch and Draft pull request, with one atomic commit and
push for every completed task. A task must not be reported complete until its
commit is present on the remote branch and the pull request exists.

## Scope

This change governs Ralph-dispatched cc-sdd work only. It does not change the
approved `provider-management` requirements, design, task graph, or completion
meaning. It does not auto-merge pull requests, force-push branches, or authorize
committing unrelated working-tree changes.

## Decisions

1. Use one Feature branch, `feat/provider-management`, based on `master`.
2. Use one Draft pull request for the Feature. Every completed task adds one
   atomic commit to the same branch and pushes it to `origin`.
3. Keep `.kiro/specs/provider-management/tasks.md` authoritative for task
   completion. Git metadata and the pull request are publication gates, not a
   replacement task tracker.
4. Require both `.ysda/agents` policy documents to be read completely before
   implementation. Missing or unreadable policy files fail closed.
5. Never emit Ralph's completion signal until local validation, task review,
   completion verification, commit, push, and pull-request verification all
   succeed.
6. Keep the pull request Draft during task execution. The reserved `VALIDATE`
   item may mark it Ready only after Feature-wide validation returns `GO`.

## Components

### Ralph dispatch prompt

`.ralph-tui-prompt.hbs` names both mandatory policy paths and instructs the
Agent to invoke only `$run-cc-sdd-task`. This makes the requirement visible at
the dispatch boundary without adding another implementation skill.

### cc-sdd task skill

`.agents/skills/run-cc-sdd-task/SKILL.md` reads both policy files completely
after projection and approval preflight, but before implementation context is
applied. The Rust policy governs discovery, design, safety, testing, and Rust
quality gates. The code-change policy governs scope, diff review, explicit
staging, atomic commits, pushes, and pull-request hygiene.

Higher-priority system instructions, repository `AGENTS.md`, the approved
Feature spec, and the selected task boundary remain authoritative. The user's
approval in this design authorizes task-local commit, push, and pull-request
creation, but not force-push, merge, deletion, or unrelated external changes.

### Publication helper

Add `tools/workflow/cc-sdd-publish.mjs` as the single publication boundary. It
accepts `<feature> <task-id>` and must:

1. Bind its arguments to `CC_SDD_DISPATCH_FEATURE` and
   `CC_SDD_DISPATCH_TASK_ID`.
2. Verify the current branch is `feat/<feature>` and the selected task is the
   only newly completed authoritative leaf.
3. Require an explicitly staged, reviewed task diff. Reject generated Ralph
   state, secrets, conflict markers, empty task commits, and unrelated staged
   paths.
4. Create one Conventional Commit with durable trailers:

   ```text
   CC-SDD-Feature: provider-management
   CC-SDD-Task: 1.3
   ```

5. Push the branch to `origin` without force.
6. Create the shared Draft pull request against `master` when absent, or verify
   that the existing open pull request has the expected head and base.
7. Confirm the task commit is reachable from `origin/feat/<feature>` and return
   the pull-request URL.

The helper never emits Ralph's completion signal.

### Completion helper

`tools/workflow/cc-sdd-complete.mjs` additionally verifies the selected task's
commit trailer, remote reachability, and open pull request before producing the
completion signal. Existing dispatch identity, approval, checkbox, and
sanitized-error protections remain in force.

### Launcher recovery

Before regenerating Ralph's disposable JSON projection,
`scripts/ralph-cc-sdd.sh` runs publication recovery for the selected Feature.
Recovery is idempotent:

- a task commit that exists locally but not remotely is pushed;
- a pushed branch without the expected Draft pull request creates or verifies
  that pull request;
- a newly checked task without its matching commit trailer blocks startup;
- ambiguous staged changes or branch identity block startup rather than being
  committed automatically.

This ordering prevents a checked task from being silently skipped after a
transient push or GitHub failure.

## Normal Task Flow

```text
Ralph dispatch
  -> projection and approval preflight
  -> load both mandatory policy files
  -> implement exactly one task using RED/GREEN/REFACTOR when applicable
  -> run narrow then broad Rust validation
  -> task-local review returns APPROVED
  -> mark only the selected task checkbox
  -> explicitly stage and review the selected task diff
  -> publication helper commits, pushes, and creates/verifies Draft PR
  -> completion helper verifies source state plus publication state
  -> Ralph marks only the selected task complete
```

Any failed step ends with `STATUS: BLOCKED` and does not produce the completion
signal.

## VALIDATE Flow

`VALIDATE` still requires all executable tasks to be complete and Feature-wide
validation to return `GO`. It verifies the full branch and pull request, pushes
any task-owned validation artifact commit when one exists, and changes the
shared pull request from Draft to Ready. It does not merge the pull request.

## Existing Working Tree Bootstrap

The repository already contains approved spec changes, workflow fixes, two
completed tasks, partial task `1.3` work, and later-task files. Bootstrap must
preserve all of it while creating reviewable history:

1. Work on `feat/provider-management`; do not stage the entire tree.
2. Commit the workflow enforcement and its tests separately.
3. Create one reconstructed atomic commit for completed task `1.1` and one for
   completed task `1.2`, each with the durable trailers above.
4. Leave partial `1.3` and future-task files unstaged until their owning task is
   freshly validated and reviewed.
5. Push the branch and create the shared Draft pull request before Ralph resumes
   at task `1.3`.

No existing change may be discarded or silently attributed to the wrong task.

## Error Handling and Safety

- Missing policies, stale projection, unapproved tasks, failed Rust checks,
  rejected review, dirty index ambiguity, Git failure, authentication failure,
  push rejection, or pull-request mismatch blocks completion.
- Error output must not contain Ralph's completion signal.
- No `git add .`, force-push, destructive reset, automatic merge, or automatic
  deletion is permitted.
- Secrets and `.ralph-tui` runtime state are never staged.
- A remote commit is not considered published until an open pull request with
  the expected head and base is verified.

## Verification Strategy

Automated tests must cover:

1. The dispatch prompt and task skill reference both mandatory policies.
2. Missing or unreadable policy files fail closed before implementation.
3. Completion-signal leakage remains impossible in prompt, policy, skill, and
   helper output.
4. Publication rejects mismatched dispatch identity, wrong branch, unrelated
   staged files, secrets, generated Ralph state, and invalid task state.
5. Temporary Git repositories with a fake remote prove one atomic commit per
   task, durable trailers, non-force push, and idempotent recovery.
6. A stubbed `gh` boundary proves Draft PR creation, existing-PR reuse, base/head
   verification, failure propagation, and VALIDATE readiness behavior.
7. Completion fails until the commit is remotely reachable and the pull request
   is verified.
8. Existing projection, wrapper, blocked-status, Rust workspace test, fmt,
   check, and Clippy gates continue to pass.

## Success Criteria

- A real Ralph iteration visibly reads both policy files before editing code.
- Each completed task produces exactly one scoped task commit and pushes it to
  `origin/feat/provider-management`.
- All task commits update one Draft pull request.
- Git or GitHub publication failures cannot cause Ralph to mark a task complete
  or continue to the next task.
- After `VALIDATE` returns `GO`, the pull request is Ready for human review but
  remains unmerged.
