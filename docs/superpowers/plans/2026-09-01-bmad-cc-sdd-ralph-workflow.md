# BMAD、cc-sdd 与 Ralph TUI Workflow Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `ys-data-agent` 中安装并集成 BMAD、cc-sdd 与 Ralph TUI，使 Ralph 能从 cc-sdd `tasks.md` 的确定性投影中逐个执行 reviewer-gated 任务。

**Architecture:** BMAD 与 cc-sdd 以项目级 Agent Skills 安装。无依赖的 Node ESM 工具把 `.kiro/specs/<feature>/tasks.md` 编译为 Ralph JSON；Ralph 的自定义 prompt 每轮只调用一个项目级 `run-cc-sdd-task` skill，后者复用 cc-sdd manual implementation、review 与 completion verification 协议。`tasks.md` 保持唯一任务事实源，生成 JSON 和运行日志不进入 Git。

**Tech Stack:** BMAD 6.x、cc-sdd 3.x、Ralph TUI 0.12、Codex Agent Skills、Node.js ESM/Test Runner、Bash、Rust workspace quality gates

---

## File Structure

- `_bmad/` — BMAD 安装、配置与共享脚本；不放实现任务。
- `.agents/skills/bmad-*` — BMAD Codex skills。
- `.agents/skills/kiro-*` — cc-sdd Codex skills。
- `.agents/skills/run-cc-sdd-task/SKILL.md` — Ralph 的单任务入口；只编排当前 task 或最终验证。
- `.kiro/settings/templates/specs/requirements.md` — 强制记录上游 PRD 路径与 commit。
- `.kiro/settings/templates/specs/tasks.md` — 强制每个可执行任务包含 Requirements、Boundary 与 observable completion。
- `.kiro/steering/{product,tech,structure}.md` — 当前仓库的长期项目记忆和质量命令。
- `tools/workflow/cc-sdd-to-ralph.mjs` — Markdown parser、validator 与 Ralph JSON compiler。
- `tools/workflow/cc-sdd-to-ralph.test.mjs` — converter 单元和 CLI 行为测试。
- `.ralph-tui/config.toml` — 串行 Codex、无 auto-commit 的 Ralph 默认配置。
- `.ralph-tui-prompt.hbs` — 强制调用单任务 skill 的项目 prompt。
- `scripts/ralph-cc-sdd.sh` — 编译 spec task projection 并启动 Ralph。
- `.gitignore` — 忽略派生 JSON、progress 和 iteration logs。

### Task 1: Install BMAD and cc-sdd Codex Skills

**Files:**
- Create: `_bmad/**`
- Create: `.agents/skills/bmad-*/**`
- Create: `.agents/skills/kiro-*/**`
- Create: `.codex/agents/spec-reviewer.toml`
- Create: `.kiro/settings/templates/**`
- Create: `AGENTS.md`

- [ ] **Step 1: Run the cc-sdd dry run**

Run:

```bash
rtk npx --yes cc-sdd@latest --codex-skills --dry-run --lang zh
```

Expected: plan targets `.agents/skills`, `.codex/agents`, `.kiro/settings/templates`, and `AGENTS.md`; no project files change.

- [ ] **Step 2: Install cc-sdd for Codex**

Run:

```bash
rtk npx --yes cc-sdd@latest --codex-skills --lang zh
```

Expected: cc-sdd reports all generated files written and recommends `$kiro-steering` / `$kiro-spec-init`.

- [ ] **Step 3: Install only the BMAD Method module for Codex**

Run:

```bash
rtk npx --yes bmad-method install --yes \
  --directory /Users/ysc/Documents/Data_Engineering/projects/ys-data-agent \
  --modules bmm \
  --tools codex \
  --set core.project_name=ys-data-agent \
  --set core.communication_language=Chinese \
  --set core.document_output_language=Chinese \
  --set core.output_folder=_bmad-output \
  --set bmm.user_skill_level=expert \
  --set bmm.project_knowledge=docs
```

Expected: `_bmad` exists and `.agents/skills/bmad-product-brief/SKILL.md` plus `.agents/skills/bmad-prd/SKILL.md` exist.

- [ ] **Step 4: Verify both skill families coexist**

Run:

```bash
rtk test -f .agents/skills/bmad-prd/SKILL.md
rtk test -f .agents/skills/kiro-impl/SKILL.md
rtk test -f .kiro/settings/templates/specs/tasks.md
```

Expected: all commands exit 0.

- [ ] **Step 5: Record an atomic commit point**

```bash
git add _bmad .agents/skills .codex/agents .kiro/settings AGENTS.md
git commit -m "chore(workflow): install BMAD and cc-sdd skills"
```

### Task 2: Configure cc-sdd Contracts and Project Steering

**Files:**
- Modify: `.kiro/settings/templates/specs/requirements.md`
- Modify: `.kiro/settings/templates/specs/tasks.md`
- Create: `.kiro/steering/product.md`
- Create: `.kiro/steering/tech.md`
- Create: `.kiro/steering/structure.md`

- [ ] **Step 1: Add upstream traceability to the requirements template**

Insert after the introduction:

```markdown
## Upstream Product Source
- **BMAD PRD**: {{BMAD_PRD_PATH}}
- **Source commit**: {{BMAD_PRD_COMMIT}}
- **Covered PRD sections**: {{BMAD_PRD_SECTIONS}}

> Product intent changes must be reconciled in the BMAD PRD before this contract is approved.
```

- [ ] **Step 2: Strengthen the task template for Ralph dispatch**

Replace the executable task annotations with:

```markdown
- [ ] {{MAJOR_NUMBER}}.{{SUB_NUMBER}} {{SUB_TASK_DESCRIPTION}}{{SUB_PARALLEL_MARK}}
  - {{DETAIL_ITEM_1}}
  - {{OBSERVABLE_COMPLETION_ITEM}}
  - _Requirements: {{REQUIREMENT_IDS}}_
  - _Boundary: {{FILE_OR_COMPONENT_BOUNDARIES}}_
  - _Depends: {{TASK_IDS_OR_NONE}}_
```

Add these rules:

```markdown
> Every executable task must carry Requirements, Boundary, Depends (`none` when empty), and at least one observable completion bullet. These fields are machine-checked before Ralph starts.
```

- [ ] **Step 3: Create product steering**

Write `.kiro/steering/product.md` with the repository's current v0.2 trustworthy-query scope, explicit exclusions, and a rule that feature requirements reference an approved BMAD PRD rather than copying it into steering.

- [ ] **Step 4: Create technology steering**

Write `.kiro/steering/tech.md` with Rust 2024, workspace crates, `unsafe_code=forbid`, and these canonical commands:

```bash
rtk cargo fmt --all --check
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk cargo test --workspace
rtk bash scripts/v0.2-release-gate.sh
```

- [ ] **Step 5: Create structure steering**

Write `.kiro/steering/structure.md` mapping `apps/ysda`, the four workspace crates, `fixtures`, `evals`, and `scripts`, and forbid unrelated cross-boundary refactors.

- [ ] **Step 6: Verify the required template markers**

Run:

```bash
rtk rg -n "Upstream Product Source|BMAD_PRD_PATH" .kiro/settings/templates/specs/requirements.md
rtk rg -n "Requirements:|Boundary:|Depends:" .kiro/settings/templates/specs/tasks.md
rtk rg -n "v0.2-release-gate" .kiro/steering/tech.md
```

Expected: every command prints at least one matching line.

- [ ] **Step 7: Record an atomic commit point**

```bash
git add .kiro/settings/templates/specs/requirements.md .kiro/settings/templates/specs/tasks.md .kiro/steering
git commit -m "docs(workflow): define cc-sdd contracts and steering"
```

### Task 3: Implement the tasks.md Parser and Validator with TDD

**Files:**
- Create: `tools/workflow/cc-sdd-to-ralph.mjs`
- Create: `tools/workflow/cc-sdd-to-ralph.test.mjs`

- [ ] **Step 1: Write a failing parser test**

Create a Node test that imports `parseTasks` and verifies checked state, `(P)` removal, Requirements, Boundary and Depends extraction:

```javascript
import assert from "node:assert/strict";
import test from "node:test";
import { parseTasks } from "./cc-sdd-to-ralph.mjs";

test("parses cc-sdd executable tasks", () => {
  const tasks = parseTasks(`# Implementation Plan
- [ ] 1. Runtime work
- [x] 1.1 Persist retry state (P)
  - Retry state survives restart.
  - _Requirements: 1.1, 1.2_
  - _Boundary: crates/ys-agent-runtime/src/retry.rs_
  - _Depends: none_
- [ ] 1.2 Resume a retry
  - Resume uses persisted state.
  - _Requirements: 1.3_
  - _Boundary: crates/ys-agent-runtime/src/coordinator.rs_
  - _Depends: 1.1_
`);

  assert.deepEqual(tasks.map(({ id, title, passes, requirements, boundary, dependsOn }) => ({
    id, title, passes, requirements, boundary, dependsOn,
  })), [
    {
      id: "1.1",
      title: "Persist retry state",
      passes: true,
      requirements: ["1.1", "1.2"],
      boundary: "crates/ys-agent-runtime/src/retry.rs",
      dependsOn: [],
    },
    {
      id: "1.2",
      title: "Resume a retry",
      passes: false,
      requirements: ["1.3"],
      boundary: "crates/ys-agent-runtime/src/coordinator.rs",
      dependsOn: ["1.1"],
    },
  ]);
});
```

- [ ] **Step 2: Run the parser test and verify RED**

Run:

```bash
rtk node --test tools/workflow/cc-sdd-to-ralph.test.mjs
```

Expected: FAIL because the module or export does not exist.

- [ ] **Step 3: Implement the minimal parser**

Implement exported `parseTasks(markdown)` using a task-line regular expression, leaf detection, annotation extraction, and `WorkflowInputError`. It must return only leaf tasks and reject missing Requirements, Boundary or Depends fields.

- [ ] **Step 4: Run the parser test and verify GREEN**

Run the Node test command again.

Expected: PASS.

- [ ] **Step 5: Add validator tests**

Add exact tests for:

```javascript
test("expands a dependency on a task group to all leaf tasks", () => { /* assert 1 -> 1.1,1.2 */ });
test("rejects unknown dependencies", () => { /* assert WorkflowInputError */ });
test("rejects dependency cycles", () => { /* assert WorkflowInputError */ });
test("rejects blocked tasks", () => { /* assert WorkflowInputError */ });
test("rejects a completed task with an incomplete prerequisite", () => { /* assert WorkflowInputError */ });
```

- [ ] **Step 6: Run tests and verify RED**

Expected: new validation tests fail until normalization and graph validation exist.

- [ ] **Step 7: Implement graph normalization and validation**

Add exported `validateTasks(tasks)` that expands group dependencies, rejects unknown IDs, performs DFS cycle detection, checks completion ordering, and rejects `_Blocked:_` tasks.

- [ ] **Step 8: Run tests and verify GREEN**

Expected: all parser and validator tests pass.

- [ ] **Step 9: Record an atomic commit point**

```bash
git add tools/workflow/cc-sdd-to-ralph.mjs tools/workflow/cc-sdd-to-ralph.test.mjs
git commit -m "feat(workflow): parse and validate cc-sdd tasks"
```

### Task 4: Compile Deterministic Ralph JSON and Add CLI Checks

**Files:**
- Modify: `tools/workflow/cc-sdd-to-ralph.mjs`
- Modify: `tools/workflow/cc-sdd-to-ralph.test.mjs`

- [ ] **Step 1: Write failing compiler tests**

Test that `compileTracker(feature, sourcePath, tasks)` produces Ralph `userStories`, preserves dependencies and completion state, adds task-local review criteria, and appends `VALIDATE` depending on every leaf task.

- [ ] **Step 2: Run tests and verify RED**

Expected: FAIL because `compileTracker` is missing.

- [ ] **Step 3: Implement `compileTracker`**

The output must be deterministic JSON with:

```javascript
{
  name: feature,
  description: `Derived from ${sourcePath}; do not edit by hand.`,
  userStories: [
    ...tasks.map(/* cc-sdd mapping */),
    {
      id: "VALIDATE",
      title: `Validate ${feature} integration`,
      acceptanceCriteria: [
        "Full repository quality gates pass",
        "Requirements coverage is complete",
        "Design and boundary validation return GO",
      ],
      dependsOn: tasks.map((task) => task.id),
      priority: 999,
      passes: false,
      labels: ["cc-sdd", "validation"],
    },
  ],
}
```

- [ ] **Step 4: Add CLI integration tests**

Use a temporary directory to verify:

- default invocation writes `.ralph-tui/generated/<feature>.json`;
- `--check` exits 0 when current;
- editing the generated file makes `--check` exit non-zero;
- a missing spec or tasks file produces a concise error and non-zero exit.

- [ ] **Step 5: Implement CLI modes**

Support:

```text
node tools/workflow/cc-sdd-to-ralph.mjs <feature>
node tools/workflow/cc-sdd-to-ralph.mjs <feature> --check
node tools/workflow/cc-sdd-to-ralph.mjs <feature> --stdout
```

Use `process.cwd()` as project root, deterministic two-space JSON plus trailing newline, and atomic write through a sibling temporary file followed by rename.

- [ ] **Step 6: Run all converter tests**

Run:

```bash
rtk node --test tools/workflow/cc-sdd-to-ralph.test.mjs
```

Expected: all tests pass.

- [ ] **Step 7: Record an atomic commit point**

```bash
git add tools/workflow/cc-sdd-to-ralph.mjs tools/workflow/cc-sdd-to-ralph.test.mjs
git commit -m "feat(workflow): compile Ralph task projections"
```

### Task 5: Add the Single-Task Skill and Ralph Runtime Configuration

**Files:**
- Create: `.agents/skills/run-cc-sdd-task/SKILL.md`
- Create: `.ralph-tui-prompt.hbs`
- Create: `.ralph-tui/config.toml`
- Create: `scripts/ralph-cc-sdd.sh`
- Modify: `.gitignore`

- [ ] **Step 1: Create `run-cc-sdd-task`**

The skill must parse `<feature> <task-id>`, run projection preflight, delegate normal IDs to `kiro-impl` manual mode, delegate `VALIDATE` to `kiro-validate-impl`, prohibit unscoped autonomous implementation, and emit the exact Ralph promise only after cc-sdd verification succeeds.

- [ ] **Step 2: Create the project Ralph prompt**

Use documented Handlebars variables and require:

```handlebars
## Current cc-sdd Work Item
- ID: {{taskId}}
- Title: {{taskTitle}}

{{taskDescription}}

## Acceptance Criteria
{{acceptanceCriteria}}

Invoke `$run-cc-sdd-task` with the feature named in the task description and `{{taskId}}`.
Execute no other task. Never emit `<promise>COMPLETE</promise>` yourself; only the skill may emit it after reviewer-gated fresh verification.
```

- [ ] **Step 3: Configure Ralph**

Write:

```toml
agent = "codex"
tracker = "json"
maxIterations = 50
iterationDelay = 1000
outputDir = ".ralph-tui/iterations"
progressFile = ".ralph-tui/progress.md"
autoCommit = false
prompt_template = ".ralph-tui-prompt.hbs"
```

- [ ] **Step 4: Create the launcher**

Write a Bash script that validates one feature argument, runs the converter, then executes:

```bash
rtk ralph-tui run --prd ".ralph-tui/generated/${feature}.json" --serial --on-error abort
```

- [ ] **Step 5: Ignore runtime state**

Append:

```gitignore
/.ralph-tui/generated/
/.ralph-tui/iterations/
/.ralph-tui/progress.md
```

- [ ] **Step 6: Verify syntax and active Ralph template**

Run:

```bash
rtk bash -n scripts/ralph-cc-sdd.sh
rtk ralph-tui config show
rtk ralph-tui template show
```

Expected: Bash syntax passes, config reports `autoCommit=false`, and template contains `$run-cc-sdd-task`.

- [ ] **Step 7: Record an atomic commit point**

```bash
git add .agents/skills/run-cc-sdd-task .ralph-tui-prompt.hbs .ralph-tui/config.toml scripts/ralph-cc-sdd.sh .gitignore
git commit -m "feat(workflow): add Ralph single-task execution harness"
```

### Task 6: Add a Fixture Spec and Verify the End-to-End Projection

**Files:**
- Create: `tools/workflow/fixtures/sample-spec/tasks.md`
- Create: `tools/workflow/fixtures/sample-spec/spec.json`
- Modify: `tools/workflow/cc-sdd-to-ralph.test.mjs`

- [ ] **Step 1: Add a representative approved cc-sdd fixture**

Include two executable tasks with dependency ordering, Requirements, Boundary and observable completion, plus `spec.json` with tasks approval true.

- [ ] **Step 2: Add an end-to-end test**

Copy the fixture into a temporary `.kiro/specs/sample-spec`, invoke the CLI, parse the generated JSON, and assert order `1.1`, `1.2`, `VALIDATE` and final dependencies `1.1`, `1.2`.

- [ ] **Step 3: Run the end-to-end test**

```bash
rtk node --test tools/workflow/cc-sdd-to-ralph.test.mjs
```

Expected: all tests pass.

- [ ] **Step 4: Run non-mutating workflow checks**

```bash
rtk bash -n scripts/ralph-cc-sdd.sh
rtk rg -n "autoCommit = false" .ralph-tui/config.toml
rtk rg -n "Never emit.*COMPLETE" .ralph-tui-prompt.hbs
```

Expected: every command exits 0.

- [ ] **Step 5: Record an atomic commit point**

```bash
git add tools/workflow/fixtures tools/workflow/cc-sdd-to-ralph.test.mjs
git commit -m "test(workflow): verify cc-sdd to Ralph projection"
```

### Task 7: Run Repository Verification and Document Usage

**Files:**
- Modify: `README.md`

- [ ] **Step 1: Add a concise development workflow section**

Document only the happy path:

```text
$bmad-product-brief (optional) → $bmad-prd
$kiro-spec-init → requirements → design → tasks approvals
scripts/ralph-cc-sdd.sh <feature>
$kiro-validate-impl <feature> / final human acceptance
```

State that `tasks.md` is authoritative and generated Ralph JSON must not be edited.

- [ ] **Step 2: Run workflow tests**

```bash
rtk node --test tools/workflow/cc-sdd-to-ralph.test.mjs
rtk bash -n scripts/ralph-cc-sdd.sh
```

Expected: PASS.

- [ ] **Step 3: Run Rust fast gates**

```bash
rtk cargo fmt --all --check
rtk cargo clippy --workspace --all-targets --all-features -- -D warnings
rtk cargo test --workspace
```

Expected: all commands exit 0.

- [ ] **Step 4: Run the existing release gate when Docker prerequisites are available**

```bash
rtk bash scripts/v0.2-release-gate.sh
```

Expected: all release-gate checks pass; if Docker is unavailable, report the exact unavailable prerequisite instead of claiming full verification.

- [ ] **Step 5: Inspect the final diff and user-owned changes**

```bash
rtk git status --short
rtk git diff --check
rtk git diff --stat
```

Expected: no whitespace errors; pre-existing `.zed/` remains untouched.

- [ ] **Step 6: Record an atomic commit point**

```bash
git add README.md
git commit -m "docs(workflow): document BMAD cc-sdd Ralph runbook"
```

## Self-Review

- Spec coverage: installation, ownership, source-of-truth, deterministic projection, one-task execution, independent review, feature validation, human acceptance and simplification rules all map to tasks above.
- Placeholder scan: commands, files, test behaviors and expected results are explicit; template metavariables are intentional runtime inputs.
- Type consistency: converter exports remain `parseTasks`, `validateTasks`, and `compileTracker`; Ralph special task ID remains `VALIDATE`; feature paths consistently use `.kiro/specs/<feature>`.
