import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, mkdir, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const projectRoot = fileURLToPath(new URL("../../", import.meta.url));
const publisher = path.join(projectRoot, "tools/workflow/cc-sdd-publish.mjs");
const feature = "sample-feature";
const taskId = "1.1";
const tasksPath = ".kiro/specs/sample-feature/tasks.md";
const sourcePath = "src/provider.rs";

const uncheckedTasks = `- [ ] 1.1 Implement provider
  - Provider behavior exists.
  - _Requirements: 1.1_
  - _Boundary: src/provider.rs_
  - _Depends: none_
- [ ] 1.2 Implement registry
  - Registry behavior exists.
  - _Requirements: 1.2_
  - _Boundary: src/registry.rs_
  - _Depends: 1.1_
`;

function run(root, command, args, env = process.env) {
  const result = spawnSync(command, args, {
    cwd: root,
    encoding: "utf8",
    env,
  });
  assert.equal(
    result.status,
    0,
    `${command} ${args.join(" ")} failed:\n${result.stderr}`,
  );
  return result.stdout.trim();
}

async function createRepository() {
  const root = await mkdtemp(path.join(tmpdir(), "cc-sdd-publish-"));
  const remote = `${root}-remote.git`;
  await mkdir(path.join(root, ".kiro/specs/sample-feature"), {
    recursive: true,
  });
  await mkdir(path.join(root, "src"), { recursive: true });

  run(root, "git", ["init", "-b", "master"]);
  run(root, "git", ["config", "user.name", "Ralph Test"]);
  run(root, "git", ["config", "user.email", "ralph@example.test"]);
  await writeFile(path.join(root, "README.md"), "fixture\n", "utf8");
  await writeFile(path.join(root, tasksPath), uncheckedTasks, "utf8");
  await writeFile(
    path.join(root, ".kiro/specs/sample-feature/spec.json"),
    `${JSON.stringify({ approvals: { tasks: { approved: true } } }, null, 2)}\n`,
    "utf8",
  );
  run(root, "git", ["add", "README.md", ".kiro"]);
  run(root, "git", ["commit", "-m", "test: initialize fixture"]);
  run(root, "git", ["init", "--bare", remote]);
  run(root, "git", ["remote", "add", "origin", remote]);
  run(root, "git", ["push", "-u", "origin", "master"]);
  run(root, "git", ["switch", "-c", "feat/sample-feature"]);

  await writeFile(
    path.join(root, tasksPath),
    uncheckedTasks.replace("- [ ] 1.1", "- [x] 1.1"),
    "utf8",
  );
  await writeFile(path.join(root, sourcePath), "pub struct Provider;\n", "utf8");
  run(root, "git", ["add", tasksPath, sourcePath]);
  return { root, remote };
}

function publish(root, options = {}) {
  const paths = options.paths ?? [tasksPath, sourcePath];
  const args = [publisher, feature, taskId];
  for (const stagedPath of paths) {
    args.push("--path", stagedPath);
  }
  return spawnSync(process.execPath, args, {
    cwd: root,
    encoding: "utf8",
    env: {
      ...process.env,
      CC_SDD_DISPATCH_FEATURE: feature,
      CC_SDD_DISPATCH_TASK_ID: taskId,
      ...options.env,
    },
  });
}

function assertDenied(result) {
  assert.notEqual(result.status, 0);
  assert.equal(result.stdout, "");
  assert.equal(result.stderr, "cc-sdd-publish: publication denied\n");
}

test("publishes one atomic task commit to its exact feature branch", async () => {
  const { root, remote } = await createRepository();

  const result = publish(root);

  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout, "");
  const commitBody = run(root, "git", ["log", "-1", "--format=%B"]);
  assert.match(commitBody, /^feat\(sample-feature\): complete task 1\.1/m);
  assert.match(commitBody, /^CC-SDD-Feature: sample-feature$/m);
  assert.match(commitBody, /^CC-SDD-Task: 1\.1$/m);
  const localHead = run(root, "git", ["rev-parse", "HEAD"]);
  const remoteHead = run(root, "git", [
    "--git-dir",
    remote,
    "rev-parse",
    "refs/heads/feat/sample-feature",
  ]);
  assert.equal(remoteHead, localHead);
  assert.deepEqual(
    run(root, "git", ["diff", "--cached", "--name-only"]),
    "",
  );
});

test("denies publication when dispatch identity does not match", async () => {
  const { root } = await createRepository();
  assertDenied(
    publish(root, { env: { CC_SDD_DISPATCH_TASK_ID: "1.2" } }),
  );
});

test("denies publication from any branch except the exact feature branch", async () => {
  const { root } = await createRepository();
  run(root, "git", ["switch", "-c", "feat/wrong-feature"]);
  assertDenied(publish(root));
});

test("denies staged paths that were not declared", async () => {
  const { root } = await createRepository();
  await writeFile(path.join(root, "src/extra.rs"), "extra\n", "utf8");
  run(root, "git", ["add", "src/extra.rs"]);
  assertDenied(publish(root));
});

test("denies Ralph runtime state and dotenv files even when declared", async () => {
  for (const deniedPath of [".ralph-tui/session.json", ".env"]) {
    const { root } = await createRepository();
    await mkdir(path.dirname(path.join(root, deniedPath)), { recursive: true });
    await writeFile(path.join(root, deniedPath), "secret\n", "utf8");
    run(root, "git", ["add", "-f", deniedPath]);
    assertDenied(
      publish(root, { paths: [tasksPath, sourcePath, deniedPath] }),
    );
  }
});

test("denies an empty staged diff", async () => {
  const { root } = await createRepository();
  run(root, "git", ["restore", "--staged", tasksPath, sourcePath]);
  assertDenied(publish(root));
});

test("denies a task commit without the staged authoritative tasks file", async () => {
  const { root } = await createRepository();
  run(root, "git", ["restore", "--staged", tasksPath]);
  assertDenied(publish(root, { paths: [sourcePath] }));
});

test("denies a tasks diff that completes any task other than the dispatch", async () => {
  const { root } = await createRepository();
  await writeFile(
    path.join(root, tasksPath),
    uncheckedTasks
      .replace("- [ ] 1.1", "- [x] 1.1")
      .replace("- [ ] 1.2", "- [x] 1.2"),
    "utf8",
  );
  run(root, "git", ["add", tasksPath]);
  assertDenied(publish(root));
});
