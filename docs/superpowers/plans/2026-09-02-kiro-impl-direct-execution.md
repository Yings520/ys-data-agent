# Direct cc-sdd `kiro-impl` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Ralph TUI with `$kiro-impl <feature>`, a Codex Skill that executes an approved cc-sdd `tasks.md` graph directly and publishes one commit per task to one Feature PR.

**Architecture:** Extract the reusable task parser into a Ralph-free state helper that validates approvals and selects the next dependency-ready task. `kiro-impl` owns the sequential Agent loop, while the existing publication helper remains the deterministic Git/PR boundary after removing Ralph dispatch and completion-sentinel assumptions. The authoritative checkpoint remains `.kiro/specs/<feature>/tasks.md`.

**Tech Stack:** Codex project Skills (Markdown/YAML), Node.js ESM and `node:test`, shell/Git/GitHub CLI, cc-sdd Markdown/JSON.

---

## File Map

- Create `tools/workflow/cc-sdd-task-state.mjs`: parse, validate, approval-gate, and select the next task from authoritative cc-sdd state.
- Create `tools/workflow/cc-sdd-task-state.test.mjs`: public parser/selector/CLI behavior.
- Create `.agents/skills/kiro-impl/SKILL.md`: multi-task direct execution loop.
- Create `.agents/skills/kiro-impl/agents/openai.yaml`: Skill discovery metadata.
- Move and edit `.agents/skills/kiro-impl/references/*.md`: task implementation, review, verification, and Feature validation protocols.
- Modify `.agents/skills/kiro-spec-tasks/SKILL.md` and `agents/openai.yaml`: hand off to `$kiro-impl`.
- Modify `tools/workflow/cc-sdd-publish.mjs` and tests: remove Ralph dispatch coupling while retaining staged-diff, commit, push, and PR enforcement.
- Modify `tools/workflow/minimal-agentic-sdlc.test.mjs`: assert the new six-Skill workflow and absence of Ralph.
- Rename `docs/BMAD-CC-SDD-RALPH-USAGE.md` to `docs/BMAD-CC-SDD-USAGE.md` and update `AGENTS.md`, `README.md`, `docs/PRD.md`, `.kiro/steering/*.md`, BMAD Skill guidance, and `.gitignore`.
- Delete Ralph-only scripts, prompt, projection/completion helpers, tests, old Skill path, and `.ralph-tui/` runtime data.
- Preserve `.kiro/specs/provider-management/tasks.md` and all current task 2.1 product files.

### Task 1: Authoritative task-state helper

**Files:**
- Create: `tools/workflow/cc-sdd-task-state.mjs`
- Create: `tools/workflow/cc-sdd-task-state.test.mjs`
- Delete after migration: `tools/workflow/cc-sdd-to-ralph.mjs`
- Delete after migration: `tools/workflow/cc-sdd-to-ralph.test.mjs`
- Modify: `tools/workflow/fixtures/sample-spec/tasks.md`

- [ ] **Step 1: Write the failing next-task tests**

Add tests that call a public `selectNextTask(tasks)` interface:

```js
test("selects the first incomplete task whose dependencies passed", () => {
  const tasks = validateTasks(parseTasks(taskMarkdown));
  assert.equal(selectNextTask(tasks).id, "2.1");
});

test("returns validation only after every task passed", () => {
  const tasks = validateTasks(parseTasks(completedTaskMarkdown));
  assert.deepEqual(selectNextTask(tasks), { id: "VALIDATE" });
});
```

Add CLI fixture tests for `node cc-sdd-task-state.mjs <feature> --next` that require `requirements`, `design`, and `tasks` approvals and emit exactly one JSON object.

- [ ] **Step 2: Run the tests and verify RED**

Run:

```bash
rtk node --test tools/workflow/cc-sdd-task-state.test.mjs
```

Expected: FAIL because the module and selector do not exist.

- [ ] **Step 3: Implement the minimal Ralph-free state API**

Move `WorkflowInputError`, `parseTasks`, and `validateTasks` from the projection generator. Add:

```js
export function selectNextTask(tasks) {
  const completed = new Set(tasks.filter((task) => task.passes).map((task) => task.id));
  const next = tasks.find(
    (task) => !task.passes && task.dependsOn.every((id) => completed.has(id)),
  );
  if (next) return next;
  if (tasks.every((task) => task.passes)) return { id: "VALIDATE" };
  throw new WorkflowInputError("No dependency-ready task remains");
}
```

The CLI loads `.kiro/specs/<feature>/spec.json` and refuses execution unless all three approvals are `true`; `--next` prints the selected task as JSON and `--check` validates silently.

- [ ] **Step 4: Run GREEN and compatibility tests**

Run:

```bash
rtk node --test tools/workflow/cc-sdd-task-state.test.mjs
rtk node --test tools/workflow/cc-sdd-publish.test.mjs
```

Expected: all tests pass after publisher imports the new module.

- [ ] **Step 5: Commit the state helper**

```bash
rtk git add tools/workflow/cc-sdd-task-state.mjs tools/workflow/cc-sdd-task-state.test.mjs tools/workflow/cc-sdd-publish.mjs tools/workflow/fixtures/sample-spec/tasks.md tools/workflow/cc-sdd-to-ralph.mjs tools/workflow/cc-sdd-to-ralph.test.mjs
rtk git commit -m "refactor(workflow): read cc-sdd task state directly"
```

### Task 2: Direct `kiro-impl` Skill

**Files:**
- Create: `.agents/skills/kiro-impl/SKILL.md`
- Create: `.agents/skills/kiro-impl/agents/openai.yaml`
- Create: `.agents/skills/kiro-impl/references/implementation.md`
- Create: `.agents/skills/kiro-impl/references/review.md`
- Create: `.agents/skills/kiro-impl/references/verify-completion.md`
- Create: `.agents/skills/kiro-impl/references/validation.md`
- Delete: `.agents/skills/run-cc-sdd-task/`
- Modify: `tools/workflow/minimal-agentic-sdlc.test.mjs`

- [ ] **Step 1: Write the failing Skill contract test**

Change the expected Skill list from `run-cc-sdd-task` to `kiro-impl` and assert that `SKILL.md` contains:

```js
for (const contract of [
  /\$kiro-impl <feature>/,
  /cc-sdd-task-state\.mjs <feature> --next/,
  /one task commit/i,
  /repeat|continue/i,
  /VALIDATE/,
]) {
  assert.match(implementationSkill, contract);
}
```

- [ ] **Step 2: Run the contract test and verify RED**

Run:

```bash
rtk node --test --test-name-pattern="six workflow entry skills" tools/workflow/minimal-agentic-sdlc.test.mjs
```

Expected: FAIL because `kiro-impl` does not exist.

- [ ] **Step 3: Create the multi-task Skill**

The Skill accepts exactly one feature argument. Its loop is:

```text
preflight approvals and clean staged index
while next task is numeric:
  read exact task boundary and mandatory policies
  RED -> minimal GREEN -> refactor
  scoped review -> fresh verification
  check only this task
  stage reviewed paths including tasks.md
  cc-sdd-publish <feature> <task-id> --path ...
  re-read authoritative task state
when next is VALIDATE:
  full Feature validation
  cc-sdd-publish <feature> VALIDATE
  stop after reporting PR state
```

It must preserve pre-existing changes, never merge, stop on the first blocker, and resume exclusively from `tasks.md`. Copy the four existing reference protocols and remove Ralph, projection, dispatched-ID, sentinel, and iteration language.

- [ ] **Step 4: Run the Skill contract test and metadata checks**

Run:

```bash
rtk node --test tools/workflow/minimal-agentic-sdlc.test.mjs
```

Expected: Skill discovery and reference-link tests pass.

- [ ] **Step 5: Commit the Skill migration**

```bash
rtk git add .agents/skills/kiro-impl .agents/skills/run-cc-sdd-task tools/workflow/minimal-agentic-sdlc.test.mjs
rtk git commit -m "feat(workflow): add direct kiro implementation skill"
```

### Task 3: Ralph-free task publication

**Files:**
- Modify: `tools/workflow/cc-sdd-publish.mjs`
- Modify: `tools/workflow/cc-sdd-publish.test.mjs`
- Delete: `tools/workflow/cc-sdd-complete.mjs`
- Delete: `tools/workflow/cc-sdd-complete.test.mjs`
- Delete: `tools/workflow/cc-sdd-completion.test.mjs`

- [ ] **Step 1: Write failing direct-publication tests**

Remove `CC_SDD_DISPATCH_*` from the successful fixture and add:

```js
test("publishes the one task identified by its staged checkbox delta", async () => {
  const fixture = await createFixture();
  await publishTask("sample-feature", "1.1", fixture.paths, fixture.root);
  assert.equal(taskTrailerAtHead(fixture.root), "CC-SDD-Task: 1.1");
});
```

Retain mismatch coverage by passing task `1.2` while only `1.1` changes; the helper must deny it based on `tasks.md`, not environment variables.

- [ ] **Step 2: Run tests and verify RED**

Run:

```bash
rtk node --test tools/workflow/cc-sdd-publish.test.mjs
```

Expected: direct success tests fail with `publication does not match dispatch`.

- [ ] **Step 3: Remove dispatch and sentinel coupling**

Delete the `CC_SDD_DISPATCH_FEATURE`/`CC_SDD_DISPATCH_TASK_ID` checks from normal and validation publication. Keep all branch, approval, staged-path, single-checkbox-delta, commit-trailer, push, remote-head, Draft PR, and Ready transition checks. Rename Ralph-specific comments/tests to direct cc-sdd terminology. Delete the completion helper because no scheduler sentinel consumes it.

- [ ] **Step 4: Run GREEN and publication recovery tests**

Run:

```bash
rtk node --test tools/workflow/cc-sdd-publish.test.mjs
```

Expected: all publication, recovery, rejection, and validation tests pass.

- [ ] **Step 5: Commit publication changes**

```bash
rtk git add tools/workflow/cc-sdd-publish.mjs tools/workflow/cc-sdd-publish.test.mjs tools/workflow/cc-sdd-complete.mjs tools/workflow/cc-sdd-complete.test.mjs tools/workflow/cc-sdd-completion.test.mjs
rtk git commit -m "refactor(workflow): publish cc-sdd tasks without Ralph"
```

### Task 4: Replace the documented Feature execution chain

**Files:**
- Modify: `.agents/skills/kiro-spec-tasks/SKILL.md`
- Modify: `.agents/skills/kiro-spec-tasks/agents/openai.yaml`
- Modify: `.agents/skills/bmad-prd/SKILL.md`
- Modify: `.agents/skills/bmad-prd/assets/prd-template.md`
- Modify: `AGENTS.md`
- Modify: `README.md`
- Modify: `docs/PRD.md`
- Create: `docs/BMAD-CC-SDD-USAGE.md`
- Delete: `docs/BMAD-CC-SDD-RALPH-USAGE.md`
- Modify: `.kiro/steering/tech.md`
- Modify: `.kiro/steering/structure.md`
- Modify: `tools/workflow/minimal-agentic-sdlc.test.mjs`

- [ ] **Step 1: Write failing documentation-chain assertions**

Require the live guidance set to contain `$kiro-impl`, exclude `$run-cc-sdd-task`, and exclude case-insensitive `Ralph` and `.ralph-tui`:

```js
assert.match(text, /\$kiro-impl/);
assert.doesNotMatch(text, /Ralph|\.ralph-tui|run-cc-sdd-task/i);
```

- [ ] **Step 2: Run tests and verify RED**

Run:

```bash
rtk node --test tools/workflow/minimal-agentic-sdlc.test.mjs
```

Expected: FAIL on the existing Ralph handoff and forbidden `$kiro-impl` rules.

- [ ] **Step 3: Update the source-of-truth workflow**

Replace the execution portion everywhere with:

```text
docs/PRD.md -> cc-sdd requirements.md -> design.md -> tasks.md
                                                -> $kiro-impl <feature>
```

`kiro-spec-tasks` must report `$kiro-impl $1` as its only next command after approval. Preserve the small-change/Feature routing, human approvals, task-local TDD/review/fresh verification, one task commit/push, shared Draft PR, and final human acceptance.

- [ ] **Step 4: Run GREEN and scan live guidance**

Run:

```bash
rtk node --test tools/workflow/minimal-agentic-sdlc.test.mjs
rtk rg -n -i "ralph|ralph-tui|run-cc-sdd-task" AGENTS.md README.md docs/PRD.md docs/BMAD-CC-SDD-USAGE.md .kiro/steering .agents/skills
```

Expected: tests pass and the scan returns no live Ralph dependency.

- [ ] **Step 5: Commit guidance changes**

```bash
rtk git add AGENTS.md README.md docs/PRD.md docs/BMAD-CC-SDD-USAGE.md docs/BMAD-CC-SDD-RALPH-USAGE.md .kiro/steering .agents/skills/kiro-spec-tasks .agents/skills/bmad-prd tools/workflow/minimal-agentic-sdlc.test.mjs
rtk git commit -m "docs(workflow): hand approved tasks to kiro impl"
```

### Task 5: Delete Ralph runtime and compatibility assets

**Files:**
- Delete: `.ralph-tui/`
- Delete: `.ralph-tui-prompt.hbs`
- Delete: `scripts/ralph-cc-sdd.sh`
- Delete: `scripts/codex-ralph`
- Delete: `tools/workflow/ralph-cc-sdd-launcher.test.mjs`
- Modify: `.gitignore`
- Modify: `tools/workflow/minimal-agentic-sdlc.test.mjs`

- [ ] **Step 1: Add a failing repository absence test**

Assert each Ralph-only path is absent and `.gitignore` no longer contains Ralph runtime rules:

```js
for (const relativePath of ralphOnlyPaths) {
  await assert.rejects(readFile(path.join(projectRoot, relativePath)), {
    code: "ENOENT",
  });
}
```

- [ ] **Step 2: Run the test and verify RED**

Run:

```bash
rtk node --test tools/workflow/minimal-agentic-sdlc.test.mjs
```

Expected: FAIL because the Ralph files still exist.

- [ ] **Step 3: Delete only Ralph-owned assets**

Remove the listed files and the complete `.ralph-tui/` directory, including disposable historical logs. Do not delete or rewrite `.kiro/specs/provider-management/` or the current task 2.1 product files.

- [ ] **Step 4: Verify deletion and workflow tests**

Run:

```bash
rtk rg -n -uuu -i "ralph|ralph-tui|run-cc-sdd-task" --glob '!.git/**' --glob '!target/**' .
rtk node --test tools/workflow/*.test.mjs
```

Expected: no live dependency remains; only explicitly retained historical design/plan records may mention the migration, and every workflow test passes.

- [ ] **Step 5: Commit the deletion**

```bash
rtk git add .gitignore .ralph-tui .ralph-tui-prompt.hbs scripts/ralph-cc-sdd.sh scripts/codex-ralph tools/workflow/ralph-cc-sdd-launcher.test.mjs tools/workflow/minimal-agentic-sdlc.test.mjs
rtk git commit -m "chore(workflow): remove Ralph runtime"
```

### Task 6: Recover and publish provider-management task 2.1

**Files:**
- Modify: `.kiro/specs/provider-management/tasks.md`
- Modify: `crates/ys-agent-store/src/sqlite.rs`
- Create: `crates/ys-agent-store/migrations/0002_provider_management.sql`
- Modify: `crates/ys-agent-store/tests/sqlite_store_test.rs`

- [ ] **Step 1: Re-run the existing RED/GREEN evidence as fresh verification**

Run:

```bash
rtk cargo fmt --all -- --check
rtk cargo test -p ys-agent-store --test sqlite_store_test
rtk cargo test -p ys-agent-store
rtk cargo check --workspace --all-targets
rtk cargo clippy --workspace --all-targets -- -D warnings
```

Expected: formatting, 8 directed migration tests, 13 store tests, workspace check, and Clippy all pass.

- [ ] **Step 2: Remove the obsolete Ralph blocker and check only 2.1**

Change the authoritative line to `- [x] 2.1 ...` and delete only its `_Blocked: Git index permission denied ..._` annotation. Confirm no other checkbox changes.

- [ ] **Step 3: Review and stage the exact task paths**

Run:

```bash
rtk git diff --check
rtk git add .kiro/specs/provider-management/tasks.md crates/ys-agent-store/src/sqlite.rs crates/ys-agent-store/migrations/0002_provider_management.sql crates/ys-agent-store/tests/sqlite_store_test.rs
```

Expected: staged paths exactly match the reviewed task boundary.

- [ ] **Step 4: Publish through the direct helper**

Run:

```bash
rtk node tools/workflow/cc-sdd-publish.mjs provider-management 2.1 \
  --path .kiro/specs/provider-management/tasks.md \
  --path crates/ys-agent-store/src/sqlite.rs \
  --path crates/ys-agent-store/migrations/0002_provider_management.sql \
  --path crates/ys-agent-store/tests/sqlite_store_test.rs
```

Expected: one task commit with cc-sdd trailers is pushed to `feat/provider-management` and the existing Draft PR is reused.

- [ ] **Step 5: Verify durable publication**

```bash
rtk proxy git log -1 --format='%H%n%B'
rtk proxy git ls-remote origin refs/heads/feat/provider-management
rtk gh pr view feat/provider-management --json state,isDraft,headRefName,baseRefName,url
```

Expected: local and remote heads match; PR is OPEN and Draft.

### Task 7: Final migration verification

**Files:**
- Test only; no planned source changes.

- [ ] **Step 1: Run all workflow tests**

```bash
rtk node --test tools/workflow/*.test.mjs
```

Expected: all tests pass.

- [ ] **Step 2: Run repository quality gates**

```bash
rtk cargo fmt --all -- --check
rtk cargo test --workspace
rtk cargo clippy --workspace --all-targets -- -D warnings
rtk git diff --check
```

Expected: every command passes.

- [ ] **Step 3: Verify the user-facing handoff**

```bash
rtk rg -n '\$kiro-impl' .agents/skills/kiro-spec-tasks/SKILL.md .agents/skills/kiro-impl/SKILL.md AGENTS.md README.md docs/BMAD-CC-SDD-USAGE.md
rtk node tools/workflow/cc-sdd-task-state.mjs provider-management --next
```

Expected: all handoffs name `$kiro-impl`; the selector prints task 2.2 after published 2.1.

- [ ] **Step 4: Inspect the complete migration diff**

```bash
rtk proxy git status --short
rtk proxy git diff --stat origin/master...HEAD
rtk proxy git log --oneline origin/master..HEAD
```

Expected: no Ralph runtime state remains, no product work was lost, commits are atomic, and unrelated user changes are preserved.
