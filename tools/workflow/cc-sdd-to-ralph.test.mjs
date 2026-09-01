import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  compileTracker,
  parseTasks,
  validateTasks,
  WorkflowInputError,
} from "./cc-sdd-to-ralph.mjs";

const scriptPath = fileURLToPath(
  new URL("./cc-sdd-to-ralph.mjs", import.meta.url),
);

const validTasks = `- [ ] 1.1 Compile the projection
  - The generated JSON contains the current task.
  - _Requirements: 1.1_
  - _Boundary: tools/workflow/cc-sdd-to-ralph.mjs_
  - _Depends: none_
- [ ] 1.2 Verify the projection
  - Stale generated JSON is rejected.
  - _Requirements: 1.2_
  - _Boundary: tools/workflow/cc-sdd-to-ralph.test.mjs_
  - _Depends: 1.1_
`;

const approvedSpec = `${JSON.stringify(
  {
    feature_name: "sample-feature",
    approvals: { tasks: { generated: true, approved: true } },
  },
  null,
  2,
)}\n`;

function runCli(root, ...args) {
  return spawnSync(process.execPath, [scriptPath, ...args], {
    cwd: root,
    encoding: "utf8",
  });
}

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

  assert.deepEqual(
    tasks.map(
      ({ id, title, passes, requirements, boundary, dependsOn }) => ({
        id,
        title,
        passes,
        requirements,
        boundary,
        dependsOn,
      }),
    ),
    [
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
    ],
  );
});

test("expands a dependency on a task group to every leaf", () => {
  const tasks = validateTasks(
    parseTasks(`- [ ] 1. Foundation
- [x] 1.1 Add the contract
  - The contract is available.
  - _Requirements: 1.1_
  - _Boundary: crates/ys-agent-core/src/contract.rs_
  - _Depends: none_
- [x] 1.2 Persist the contract
  - Persistence round-trips the contract.
  - _Requirements: 1.2_
  - _Boundary: crates/ys-agent-store/src/contract.rs_
  - _Depends: 1.1_
- [ ] 2.1 Consume the foundation
  - The runtime consumes the persisted contract.
  - _Requirements: 2.1_
  - _Boundary: crates/ys-agent-runtime/src/contract.rs_
  - _Depends: 1_
`),
  );

  assert.deepEqual(tasks.find((task) => task.id === "2.1").dependsOn, [
    "1.1",
    "1.2",
  ]);
});

test("rejects an unknown dependency", () => {
  assert.throws(
    () =>
      validateTasks(
        parseTasks(`- [ ] 1.1 Use a missing dependency
  - The task cannot start.
  - _Requirements: 1.1_
  - _Boundary: crates/ys-agent-runtime/src/lib.rs_
  - _Depends: 9.1_
`),
      ),
    (error) =>
      error instanceof WorkflowInputError &&
      /unknown dependency 9\.1/.test(error.message),
  );
});

test("rejects dependency cycles", () => {
  assert.throws(
    () =>
      validateTasks(
        parseTasks(`- [ ] 1.1 First task
  - First is complete.
  - _Requirements: 1.1_
  - _Boundary: crates/ys-agent-core/src/first.rs_
  - _Depends: 1.2_
- [ ] 1.2 Second task
  - Second is complete.
  - _Requirements: 1.2_
  - _Boundary: crates/ys-agent-core/src/second.rs_
  - _Depends: 1.1_
`),
      ),
    (error) =>
      error instanceof WorkflowInputError &&
      /dependency cycle/.test(error.message),
  );
});

test("rejects blocked tasks", () => {
  assert.throws(
    () =>
      validateTasks(
        parseTasks(`- [ ] 1.1 Blocked task
  - The task needs a product decision.
  - _Requirements: 1.1_
  - _Boundary: crates/ys-agent-core/src/lib.rs_
  - _Depends: none_
  - _Blocked: PRD conflict_
`),
      ),
    (error) =>
      error instanceof WorkflowInputError && /Task 1\.1 is blocked/.test(error.message),
  );
});

test("rejects a completed task whose prerequisite is incomplete", () => {
  assert.throws(
    () =>
      validateTasks(
        parseTasks(`- [ ] 1.1 Prerequisite
  - The prerequisite is complete.
  - _Requirements: 1.1_
  - _Boundary: crates/ys-agent-core/src/first.rs_
  - _Depends: none_
- [x] 1.2 Dependent task
  - The dependent behavior is complete.
  - _Requirements: 1.2_
  - _Boundary: crates/ys-agent-core/src/second.rs_
  - _Depends: 1.1_
`),
      ),
    (error) =>
      error instanceof WorkflowInputError &&
      /completed but prerequisite 1\.1 is incomplete/.test(error.message),
  );
});

test("compiles deterministic Ralph stories and a final validation task", () => {
  const tasks = validateTasks(
    parseTasks(`- [x] 1.1 Persist retry state
  - Retry state survives restart.
  - _Requirements: 1.1, 1.2_
  - _Boundary: crates/ys-agent-runtime/src/retry.rs_
  - _Depends: none_
- [ ]* 2.1 Resume a retry
  - Resume uses persisted state.
  - _Requirements: 2.1_
  - _Boundary: crates/ys-agent-runtime/src/coordinator.rs_
  - _Depends: 1.1_
`),
  );

  const tracker = compileTracker(
    "query-recovery",
    ".kiro/specs/query-recovery/tasks.md",
    tasks,
  );

  assert.equal(tracker.name, "query-recovery");
  assert.deepEqual(
    tracker.userStories.map(
      ({ id, priority, passes, dependsOn, labels }) => ({
        id,
        priority,
        passes,
        dependsOn,
        labels,
      }),
    ),
    [
      {
        id: "1.1",
        priority: 1,
        passes: true,
        dependsOn: [],
        labels: ["cc-sdd", "feature:query-recovery"],
      },
      {
        id: "2.1",
        priority: 102,
        passes: false,
        dependsOn: ["1.1"],
        labels: ["cc-sdd", "feature:query-recovery", "optional"],
      },
      {
        id: "VALIDATE",
        priority: 999,
        passes: false,
        dependsOn: ["1.1", "2.1"],
        labels: ["cc-sdd", "feature:query-recovery", "validation"],
      },
    ],
  );
  assert.match(
    tracker.userStories[0].description,
    /Feature: query-recovery/,
  );
  assert.deepEqual(tracker.userStories[0].acceptanceCriteria, [
    "Retry state survives restart.",
    "Requirements covered: 1.1, 1.2",
    "Boundary respected: crates/ys-agent-runtime/src/retry.rs",
    "cc-sdd task-local review returns APPROVED",
    "Fresh verification passes",
  ]);
});

test("rejects a task without an observable completion bullet", () => {
  assert.throws(
    () =>
      parseTasks(`- [ ] 1.1 Annotation-only task
  - _Requirements: 1.1_
  - _Boundary: crates/ys-agent-core/src/lib.rs_
  - _Depends: none_
`),
    (error) =>
      error instanceof WorkflowInputError &&
      /observable completion bullet/.test(error.message),
  );
});

test("rejects an empty task plan", () => {
  assert.throws(
    () => validateTasks(parseTasks("# No executable tasks\n")),
    (error) =>
      error instanceof WorkflowInputError && /no executable tasks/i.test(error.message),
  );
});

test("CLI writes and checks a deterministic tracker projection", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "cc-sdd-to-ralph-"));
  const specDirectory = path.join(root, ".kiro/specs/sample-feature");
  await mkdir(specDirectory, { recursive: true });
  await writeFile(path.join(specDirectory, "tasks.md"), validTasks, "utf8");
  await writeFile(path.join(specDirectory, "spec.json"), approvedSpec, "utf8");

  const generate = runCli(root, "sample-feature");
  assert.equal(generate.status, 0, generate.stderr);

  const outputPath = path.join(
    root,
    ".ralph-tui/generated/sample-feature.json",
  );
  const firstOutput = await readFile(outputPath, "utf8");
  assert.deepEqual(
    JSON.parse(firstOutput).userStories.map((story) => story.id),
    ["1.1", "1.2", "VALIDATE"],
  );

  const check = runCli(root, "sample-feature", "--check");
  assert.equal(check.status, 0, check.stderr);

  await writeFile(outputPath, "{}\n", "utf8");
  const stale = runCli(root, "sample-feature", "--check");
  assert.equal(stale.status, 1);
  assert.match(stale.stderr, /projection is stale/i);
});

test("CLI rejects a missing feature spec", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "cc-sdd-to-ralph-"));
  const result = runCli(root, "missing-feature");

  assert.equal(result.status, 1);
  assert.match(result.stderr, /Cannot read .*missing-feature.*tasks\.md/);
});

test("CLI refuses to dispatch tasks before human task approval", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "cc-sdd-to-ralph-"));
  const specDirectory = path.join(root, ".kiro/specs/sample-feature");
  await mkdir(specDirectory, { recursive: true });
  await writeFile(path.join(specDirectory, "tasks.md"), validTasks, "utf8");
  await writeFile(
    path.join(specDirectory, "spec.json"),
    JSON.stringify({ approvals: { tasks: { approved: false } } }),
    "utf8",
  );

  const result = runCli(root, "sample-feature");
  assert.equal(result.status, 1);
  assert.match(result.stderr, /tasks are not approved/i);
});

test("CLI rejects feature names that could escape the specs directory", async () => {
  const root = await mkdtemp(path.join(tmpdir(), "cc-sdd-to-ralph-"));
  const result = runCli(root, "../outside");

  assert.equal(result.status, 1);
  assert.match(result.stderr, /invalid feature name/i);
});
