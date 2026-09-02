# Code Change, Validation, Commit, and Pull Request Standard

This standard applies to features, bug fixes, refactoring, documentation, and engineering configuration changes. Its goal is to make every change scoped, verifiable, reviewable, and reversible.

Command examples use the repository's `rtk` prefix. If `rtk` is unavailable, run the underlying command without the prefix.

## 1. Core Principles

- Clarify the requirement, acceptance criteria, and impact before changing code.
- Make only the changes required by the current task. Avoid opportunistic refactoring.
- Follow the repository's existing architecture, coding style, dependency policy, and error-handling conventions.
- Add tests for new behavior. Bug fixes should include regression tests.
- Do not overwrite, delete, or commit another contributor's unfinished work.
- Report only validation results that were actually executed. Never hide or infer failures.

## 2. Standard Workflow

```text
Confirm requirement and scope
  → Create a task branch
  → Modify code
  → Run local validation
  → Review the working-tree diff
  → Stage explicit files
  → Review the staged diff
  → Create an atomic commit
  → Push the branch
  → Create and self-review the Pull Request
```

## 3. Modify Code

### 3.1 Before Starting

Confirm the following:

- [ ] The Issue / Task ID and objective are clear.
- [ ] The acceptance criteria are testable.
- [ ] Affected modules, public APIs, persistence formats, and external protocols are identified.
- [ ] Relevant code, tests, and project standards have been reviewed.
- [ ] The current working-tree state is understood.

```bash
rtk git status --short
rtk git branch --show-current
rtk git diff --stat
```

### 3.2 Change Rules

- Prefer the smallest change that satisfies the acceptance criteria.
- Do not include unrelated renaming, formatting, dependency upgrades, or architectural changes.
- Reuse existing module boundaries and implementation patterns. Do not create parallel abstractions without justification.
- Explain compatibility impact when changing a public API, database schema, serialization format, or dependency version.
- Do not weaken validation, remove assertions, or skip tests to make a build pass.
- Record out-of-scope problems in a separate Issue instead of mixing them into the current change.

## 4. Local Validation

Validate from narrow to broad: run directly affected tests first, then module, crate, or workspace checks.

### 4.1 Default Rust Quality Gates

```bash
rtk cargo fmt --all -- --check
rtk cargo check --workspace
rtk cargo test --workspace
rtk cargo clippy --workspace --all-targets -- -D warnings
```

Use stricter commands when required by CI, the task plan, or module-specific standards. Use `--all-features` only when the project expects every feature combination to be compatible.

### 4.2 Test Requirements

- [ ] New or changed behavior has unit-test coverage.
- [ ] Cross-module flows have appropriate integration tests.
- [ ] Bug fixes include a regression test that reproduces the original defect.
- [ ] Required external-service tests were run.
- [ ] If an external environment is unavailable, the PR is marked Draft, documents the gap, and is not mergeable.
- [ ] Test output contains no errors or unexpected warnings.

### 4.3 Pre-Commit Diff Review

```bash
rtk git status --short
rtk git diff --check
rtk git diff --stat
rtk git diff
```

Confirm the following:

- [ ] Only files related to the current task are included.
- [ ] No debug code, temporary logging, commented-out implementation, or generated debris remains.
- [ ] No `.env` file, key, token, certificate, credential, database file, or real user data is present.
- [ ] Dependency versions, lockfiles, CI configuration, and public protocols were not changed accidentally.
- [ ] Every new file has a clear purpose and is correctly tracked or ignored.

## 5. Git Branches and Commits

### 5.1 Branch Naming

Use this format:

```text
<type>/<task-or-issue>-<short-description>
```

Common branch types:

- `feat/`: new functionality
- `fix/`: bug fix
- `refactor/`: internal restructuring without an external behavior change
- `docs/`: documentation
- `test/`: test improvements
- `chore/`: build, dependency, or maintenance work
- `release/`: release preparation

Use lowercase English words and hyphens. Keep the name concise and outcome-focused. Do not use personal names or vague labels such as `temp` or `test2`.

Recommended examples:

```text
feat/task-10-query-tools
fix/issue-123-query-timeout
refactor/issue-208-connector-registry
docs/task-42-pr-workflow
release/v0.2
```

### 5.2 Commit Messages

Use Conventional Commits:

```text
<type>(<scope>): <imperative summary>
```

Rules:

- Use an English imperative summary that states what the commit does.
- Keep `type` consistent with the branch purpose.
- Use a stable module name for `scope`, such as `tools`, `runtime`, or `store`.
- Be specific. Keep the subject at or below 72 characters when practical.
- Use the body to explain motivation, compatibility, or migration when necessary. Do not restate the diff.
- Each commit must represent one independently understandable and reversible logical change.

Recommended examples:

```text
feat(tools): add governed query capabilities
fix(runtime): enforce tool output byte limits
test(connectors): cover bound timestamp parameters
docs(workflow): define pull request standards
```

Prohibited examples:

```text
update code
fix stuff
changes
wip
final version
```

### 5.3 Stage and Commit

Stage explicit task files. Avoid `git add .` unless the entire working tree has been reviewed.

```bash
rtk git add <file-1> <file-2>
rtk git diff --cached --check
rtk git diff --cached --stat
rtk git diff --cached
rtk git commit -m "feat(tools): add governed query capabilities"
rtk git show --stat --oneline HEAD
rtk git status --short
```

Before committing, confirm that the staged diff is complete and limited to the current task. Split multiple independently reviewable goals into separate atomic commits.

## 6. Create a Pull Request

### 6.1 Preconditions

- [ ] The branch is based on the correct, current target branch.
- [ ] All required local validation has passed.
- [ ] Commit history is clear and contains no temporary commits.
- [ ] No task-related change is left unstaged or uncommitted.
- [ ] The PR contains no unrelated files or sensitive information.

```bash
rtk git status --short
rtk git log --oneline <base-branch>..HEAD
rtk git diff --stat <base-branch>...HEAD
rtk git push -u origin <branch-name>
```

### 6.2 PR Title

Use the same semantic format as a commit message:

```text
<type>(<scope>): <clear outcome>
```

Examples:

```text
feat(tools): add governed query capabilities
fix(runtime): reject oversized tool output
```

The title must describe a reviewable outcome. Do not use vague titles such as `WIP`, `misc changes`, `update`, or `fix`. Use the platform's Draft PR state for unfinished collaboration.

### 6.3 PR Description Template

```markdown
## Background

Describe the problem, the business or technical context, and why this change is needed.

Closes #<issue-id>
Task: <task-id-or-link>

## Changes

- Change one
- Change two
- Explicitly excluded scope

## Testing

- `command actually executed` — Passed
- `command actually executed` — Passed
- Not run and reason: `None`

## Risks

- Compatibility, data, performance, security, or release risk
- Mitigation
- Rollback approach

## Self-Review

- [ ] Scope matches the Issue / Task
- [ ] Required Format / Lint / Build / Test checks passed
- [ ] Full diff and commit history were reviewed
- [ ] No sensitive information or unrelated files are included
- [ ] Compatibility, migration, and unverified areas are documented
- [ ] Documentation and tests were updated where required
```

### 6.4 PR State Rules

- Mark a PR Ready for review / mergeable only after every required test passes.
- Use a Draft PR when tests fail, an external integration is unverified, or implementation is incomplete. State each blocker in the description.
- Re-run affected validation after addressing review feedback and update the `Testing` section.
- Before merging, confirm CI success, required approvals, the target branch, and the absence of unexpected diff changes.

## 7. Prohibited Actions

- Do not commit keys, tokens, passwords, certificates, connection strings, production configuration, or sensitive data.
- Do not mix unrelated code, formatting, dependency, or documentation changes into the current Issue / Task.
- Do not create or keep a PR mergeable while required tests are failing.
- Do not use vague commit or PR descriptions such as `update`, `fix stuff`, `changes`, or `WIP`.
- Do not hide failures by deleting tests, weakening assertions, disabling lint rules, or broadening ignore patterns.
- Do not silently change public APIs, persistence formats, database schemas, dependency versions, or CI gates.
- Do not commit local state directories, build artifacts, database snapshots, or fixtures containing real data.
- Do not overwrite, discard, or commit another contributor's working-tree changes without authorization.

## 8. Definition of Done

A change is complete only when all conditions are met:

- [ ] Requirements and acceptance criteria are satisfied.
- [ ] The implementation is minimal and follows the project architecture.
- [ ] Required Format, Lint, Build, Unit Test, and Integration Test checks passed.
- [ ] Working-tree, staged, and final PR diffs were reviewed.
- [ ] Commits are atomic and clearly described.
- [ ] The PR links its Issue / Task and documents changes, testing, and risks.
- [ ] No sensitive information, unrelated files, or undisclosed compatibility risks are present.
- [ ] CI passes and project approval requirements are satisfied.
