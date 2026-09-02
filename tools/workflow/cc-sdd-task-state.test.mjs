import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdir, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  parseTasks,
  selectNextTask,
  validateTasks,
  WorkflowInputError,
} from "./cc-sdd-task-state.mjs";

const scriptPath = fileURLToPath(
  new URL("./cc-sdd-task-state.mjs", import.meta.url),
);

const taskMarkdown = `- [x] 1.1 Establish the contract
  - The contract is available.
  - _Requirements: 1.1_
  - _Boundary: src/contract.rs_
  - _Depends: none_
- [ ] 2.1 Persist the contract
  - The contract round-trips.
  - _Requirements: 2.1_
  - _Boundary: src/store.rs_
  - _Depends: 1.1_
- [ ] 2.2 Consume the contract
  - Runtime uses the contract.
  - _Requirements: 2.2_
  - _Boundary: src/runtime.rs_
  - _Depends: 2.1_
`;

async function createFeature({ approvals, tasks = taskMarkdown }) {
  const root = await mkdtemp(path.join(tmpdir(), "cc-sdd-task-state-"));
  const featureDirectory = path.join(root, ".kiro/specs/sample-feature");
  await mkdir(featureDirectory, { recursive: true });
  await writeFile(path.join(featureDirectory, "tasks.md"), tasks, "utf8");
  await writeFile(
    path.join(featureDirectory, "spec.json"),
    JSON.stringify({ approvals }),
    "utf8",
  );
  return root;
}

test("selects the first incomplete task whose dependencies passed", () => {
  const tasks = validateTasks(parseTasks(taskMarkdown));

  assert.equal(selectNextTask(tasks).id, "2.1");
});

test("returns validation only after every task passed", () => {
  const tasks = validateTasks(
    parseTasks(taskMarkdown.replaceAll("- [ ]", "- [x]")),
  );

  assert.deepEqual(selectNextTask(tasks), { id: "VALIDATE" });
});

test("reports an unsafe graph when no incomplete task is dependency-ready", () => {
  assert.throws(
    () =>
      selectNextTask([
        {
          id: "2.1",
          passes: false,
          dependsOn: ["missing"],
        },
      ]),
    (error) =>
      error instanceof WorkflowInputError &&
      /dependency-ready task/i.test(error.message),
  );
});

test("expands a dependency on a task group to every leaf", () => {
  const tasks = validateTasks(
    parseTasks(`- [ ] 1. Foundation
- [x] 1.1 Define the contract
  - The contract exists.
  - _Requirements: 1.1_
  - _Boundary: src/contract.rs_
  - _Depends: none_
- [x] 1.2 Persist the contract
  - Persistence round-trips.
  - _Requirements: 1.2_
  - _Boundary: src/store.rs_
  - _Depends: 1.1_
- [ ] 2.1 Consume the foundation
  - Runtime consumes persistence.
  - _Requirements: 2.1_
  - _Boundary: src/runtime.rs_
  - _Depends: 1_
`),
  );

  assert.deepEqual(tasks.find((task) => task.id === "2.1").dependsOn, [
    "1.1",
    "1.2",
  ]);
});

test("rejects unknown dependencies and dependency cycles", () => {
  assert.throws(
    () =>
      validateTasks(
        parseTasks(`- [ ] 1.1 Missing dependency
  - Execution is refused.
  - _Requirements: 1.1_
  - _Boundary: src/task.rs_
  - _Depends: 9.1_
`),
      ),
    /unknown dependency 9\.1/,
  );

  assert.throws(
    () =>
      validateTasks(
        parseTasks(`- [ ] 1.1 First task
  - First is complete.
  - _Requirements: 1.1_
  - _Boundary: src/first.rs_
  - _Depends: 1.2_
- [ ] 1.2 Second task
  - Second is complete.
  - _Requirements: 1.2_
  - _Boundary: src/second.rs_
  - _Depends: 1.1_
`),
      ),
    /dependency cycle/,
  );
});

test("rejects blocked tasks and invalid completed dependency state", () => {
  assert.throws(
    () =>
      validateTasks(
        parseTasks(`- [ ] 1.1 Blocked task
  - A decision is required.
  - _Requirements: 1.1_
  - _Boundary: src/task.rs_
  - _Depends: none_
  - _Blocked: unresolved contract_
`),
      ),
    /Task 1\.1 is blocked/,
  );

  assert.throws(
    () => validateTasks(parseTasks(taskMarkdown.replace("- [x] 1.1", "- [ ] 1.1").replace("- [ ] 2.1", "- [x] 2.1"))),
    /completed but prerequisite 1\.1 is incomplete/,
  );
});

test("rejects tasks without observable completion criteria", () => {
  assert.throws(
    () =>
      parseTasks(`- [ ] 1.1 Annotation-only task
  - _Requirements: 1.1_
  - _Boundary: src/task.rs_
  - _Depends: none_
`),
    /observable completion bullet/,
  );
  assert.throws(
    () => validateTasks(parseTasks("# No executable tasks\n")),
    /no executable tasks/i,
  );
});

test("CLI emits the next authoritative task as JSON", async () => {
  const approved = {
    requirements: { approved: true },
    design: { approved: true },
    tasks: { approved: true },
  };
  const root = await createFeature({ approvals: approved });

  const result = spawnSync(
    process.execPath,
    [scriptPath, "sample-feature", "--next"],
    { cwd: root, encoding: "utf8" },
  );

  assert.equal(result.status, 0, result.stderr);
  assert.equal(JSON.parse(result.stdout).id, "2.1");
});

test("CLI denies execution until every cc-sdd artifact is approved", async () => {
  for (const missingApproval of ["requirements", "design", "tasks"]) {
    const approvals = {
      requirements: { approved: true },
      design: { approved: true },
      tasks: { approved: true },
    };
    approvals[missingApproval].approved = false;
    const root = await createFeature({ approvals });

    const result = spawnSync(
      process.execPath,
      [scriptPath, "sample-feature", "--check"],
      { cwd: root, encoding: "utf8" },
    );

    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /cc-sdd state denied/i);
  }
});
