import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdtemp, mkdir, readFile, writeFile } from "node:fs/promises";
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
  const fakeBin = path.join(root, ".test-bin");
  const ghCallLog = path.join(root, ".gh-calls");
  const ghPrState = path.join(root, ".gh-pr-state");
  await mkdir(path.join(root, ".kiro/specs/sample-feature"), {
    recursive: true,
  });
  await mkdir(path.join(root, "src"), { recursive: true });
  await mkdir(fakeBin, { recursive: true });
  await writeFile(
    path.join(fakeBin, "gh"),
    `#!/bin/sh
printf '%s\\n' "$*" >> "$GH_CALL_LOG"
if [ "\${GH_FAIL:-}" = "1" ]; then
  printf '%s\\n' 'gh failed' >&2
  exit 1
fi
case "$1 $2" in
  "pr view")
    if [ -n "\${GH_PR_JSON:-}" ]; then
      printf '%s\\n' "$GH_PR_JSON"
    elif [ -f "$GH_PR_STATE" ]; then
      if grep -q '^ready$' "$GH_PR_STATE"; then
        printf '%s\\n' '{"state":"OPEN","isDraft":false,"baseRefName":"master","headRefName":"feat/sample-feature","url":"https://github.example/pull/1"}'
      else
        printf '%s\\n' '{"state":"OPEN","isDraft":true,"baseRefName":"master","headRefName":"feat/sample-feature","url":"https://github.example/pull/1"}'
      fi
    else
      printf '%s\\n' 'no pull requests found' >&2
      exit 1
    fi
    ;;
  "pr create")
    : > "$GH_PR_STATE"
    printf '%s\\n' 'https://github.example/pull/1'
    ;;
  "pr ready")
    printf '%s\\n' 'ready' > "$GH_PR_STATE"
    printf '%s\\n' 'ready'
    ;;
  *) exit 64 ;;
esac
`,
    "utf8",
  );
  await writeFile(
    path.join(fakeBin, "git"),
    "#!/bin/sh\nprintf '%s\\n' 'direct git is unavailable' >&2\nexit 126\n",
    "utf8",
  );
  await writeFile(
    path.join(fakeBin, "rtk"),
    `#!/bin/sh
printf '%s\\n' "$*" >> "$RTK_CALL_LOG"
if [ "$1 $2" = "proxy git" ]; then
  shift 2
  if [ "\${RTK_PROXY_WRITES_DENIED:-}" = "1" ] && { [ "$1" = "commit" ] || [ "$1" = "push" ]; }; then
    printf '%s\\n' 'proxy git writes are denied' >&2
    exit 126
  fi
  exec "$REAL_GIT" "$@"
fi
if [ "$1" = "git" ]; then
  shift
  if [ "\${RTK_COMPACT_MARKERS:-}" = "1" ] && [ "$1 $2 $3 $4" = "diff --cached --name-only -z" ]; then
    "$REAL_GIT" "$@"
    printf '\\n\\n--- Changes ---\\n\\n'
    exit 0
  fi
  exec "$REAL_GIT" "$@"
fi
exit 64
`,
    "utf8",
  );
  await chmod(path.join(fakeBin, "gh"), 0o755);
  await chmod(path.join(fakeBin, "git"), 0o755);
  await chmod(path.join(fakeBin, "rtk"), 0o755);

  run(root, "git", ["init", "-b", "master"]);
  run(root, "git", ["config", "user.name", "cc-sdd Test"]);
  run(root, "git", ["config", "user.email", "cc-sdd@example.test"]);
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
  return {
    root,
    remote,
    ghCallLog,
    ghPrState,
    env: {
      PATH: `${fakeBin}${path.delimiter}${process.env.PATH}`,
      GH_CALL_LOG: ghCallLog,
      GH_PR_STATE: ghPrState,
      REAL_GIT: "/usr/bin/git",
      RTK_CALL_LOG: path.join(root, ".rtk-calls"),
    },
  };
}

function publish(root, options = {}) {
  const paths = options.paths ?? [tasksPath, sourcePath];
  const selectedTaskId = options.taskId ?? taskId;
  const args = [publisher, feature, selectedTaskId];
  for (const stagedPath of paths) {
    args.push("--path", stagedPath);
  }
  return spawnSync(process.execPath, args, {
    cwd: root,
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${path.join(root, ".test-bin")}${path.delimiter}${process.env.PATH}`,
      GH_CALL_LOG: path.join(root, ".gh-calls"),
      GH_PR_STATE: path.join(root, ".gh-pr-state"),
      REAL_GIT: "/usr/bin/git",
      RTK_CALL_LOG: path.join(root, ".rtk-calls"),
      ...options.env,
    },
  });
}

function validateFeature(root, options = {}) {
  return spawnSync(process.execPath, [publisher, feature, "VALIDATE"], {
    cwd: root,
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${path.join(root, ".test-bin")}${path.delimiter}${process.env.PATH}`,
      GH_CALL_LOG: path.join(root, ".gh-calls"),
      GH_PR_STATE: path.join(root, ".gh-pr-state"),
      REAL_GIT: "/usr/bin/git",
      RTK_CALL_LOG: path.join(root, ".rtk-calls"),
      ...options.env,
    },
  });
}

async function publishSecondTask(root) {
  const currentTasks = uncheckedTasks
    .replace("- [ ] 1.1", "- [x] 1.1")
    .replace("- [ ] 1.2", "- [x] 1.2");
  await writeFile(path.join(root, tasksPath), currentTasks, "utf8");
  await writeFile(path.join(root, "src/registry.rs"), "pub struct Registry;\n", "utf8");
  run(root, "git", ["add", tasksPath, "src/registry.rs"]);
  const result = publish(root, {
    taskId: "1.2",
    paths: [tasksPath, "src/registry.rs"],
  });
  assert.equal(result.status, 0, result.stderr);
}

function recover(root, options = {}) {
  return spawnSync(process.execPath, [publisher, "--recover", feature], {
    cwd: root,
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${path.join(root, ".test-bin")}${path.delimiter}${process.env.PATH}`,
      GH_CALL_LOG: path.join(root, ".gh-calls"),
      GH_PR_STATE: path.join(root, ".gh-pr-state"),
      REAL_GIT: "/usr/bin/git",
      RTK_CALL_LOG: path.join(root, ".rtk-calls"),
      ...options.env,
    },
  });
}

function commitFixtureTask(root, withTrailers = true) {
  const args = ["commit", "-m", "feat(sample-feature): complete task 1.1"];
  if (withTrailers) {
    args.push(
      "-m",
      "CC-SDD-Feature: sample-feature\nCC-SDD-Task: 1.1",
    );
  }
  run(root, "git", args);
}

function assertDenied(result) {
  assert.notEqual(result.status, 0);
  assert.equal(result.stdout, "");
  assert.equal(result.stderr, "cc-sdd-publish: publication denied\n");
}

test("publishes one atomic task commit to its exact feature branch", async () => {
  const { root, remote, ghCallLog } = await createRepository();

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
  assert.match(await readFile(ghCallLog, "utf8"), /pr create .*--draft/);
});

test("uses raw RTK proxy output for machine-readable Git commands", async () => {
  const { root } = await createRepository();

  const result = publish(root, { env: { RTK_COMPACT_MARKERS: "1" } });

  assert.equal(result.status, 0, result.stderr);
  assert.match(
    await readFile(path.join(root, ".rtk-calls"), "utf8"),
    /^proxy git diff --cached --name-only -z$/m,
  );
});

test("routes Git mutations through the writable RTK command", async () => {
  const { root } = await createRepository();

  const result = publish(root, {
    env: { RTK_PROXY_WRITES_DENIED: "1" },
  });

  assert.equal(result.status, 0, result.stderr);
  const calls = await readFile(path.join(root, ".rtk-calls"), "utf8");
  assert.match(calls, /^git commit /m);
  assert.match(calls, /^git push /m);
  assert.doesNotMatch(calls, /^proxy git (?:commit|push) /m);
});

test("reuses the one open Draft PR for the expected head and base", async () => {
  const { root, ghCallLog } = await createRepository();
  const result = publish(root, {
    env: {
      GH_PR_JSON: JSON.stringify({
        state: "OPEN",
        isDraft: true,
        baseRefName: "master",
        headRefName: "feat/sample-feature",
        url: "https://github.example/pull/1",
      }),
    },
  });

  assert.equal(result.status, 0, result.stderr);
  const calls = await readFile(ghCallLog, "utf8");
  assert.match(calls, /pr view/);
  assert.doesNotMatch(calls, /pr create/);
});

test("denies a PR with the wrong lifecycle or branch identity", async () => {
  const invalidPullRequests = [
    {
      state: "OPEN",
      isDraft: true,
      baseRefName: "master",
      headRefName: "feat/wrong-feature",
    },
    {
      state: "OPEN",
      isDraft: true,
      baseRefName: "develop",
      headRefName: "feat/sample-feature",
    },
    {
      state: "CLOSED",
      isDraft: true,
      baseRefName: "master",
      headRefName: "feat/sample-feature",
    },
    {
      state: "OPEN",
      isDraft: false,
      baseRefName: "master",
      headRefName: "feat/sample-feature",
    },
  ];

  for (const pullRequest of invalidPullRequests) {
    const { root } = await createRepository();
    assertDenied(
      publish(root, {
        env: {
          GH_PR_JSON: JSON.stringify({
            ...pullRequest,
            url: "https://github.example/pull/1",
          }),
        },
      }),
    );
  }
});

test("denies GitHub CLI failures without leaking their output", async () => {
  const { root } = await createRepository();
  assertDenied(publish(root, { env: { GH_FAIL: "1" } }));
});

test("recovery pushes a trailer-backed task commit when remote is behind", async () => {
  const { root, remote } = await createRepository();
  commitFixtureTask(root);

  const result = recover(root, {
    env: {
      GH_PR_JSON: JSON.stringify({
        state: "OPEN",
        isDraft: true,
        baseRefName: "master",
        headRefName: "feat/sample-feature",
        url: "https://github.example/pull/1",
      }),
    },
  });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(
    run(root, "git", ["rev-parse", "HEAD"]),
    run(root, "git", [
      "--git-dir",
      remote,
      "rev-parse",
      "refs/heads/feat/sample-feature",
    ]),
  );
});

test("recovery creates the Draft PR after an earlier PR interruption", async () => {
  const { root, ghCallLog } = await createRepository();
  commitFixtureTask(root);
  run(root, "git", [
    "push",
    "origin",
    "HEAD:refs/heads/feat/sample-feature",
  ]);

  const result = recover(root);

  assert.equal(result.status, 0, result.stderr);
  assert.match(await readFile(ghCallLog, "utf8"), /pr create .*--draft/);
});

test("recovery rejects a checked task without its durable commit trailers", async () => {
  const { root } = await createRepository();
  commitFixtureTask(root, false);

  assertDenied(recover(root));
});

test("VALIDATE publishes an audit commit and marks the shared PR ready", async () => {
  const { root, ghCallLog } = await createRepository();
  const first = publish(root);
  assert.equal(first.status, 0, first.stderr);
  await publishSecondTask(root);

  const result = validateFeature(root);

  assert.equal(result.status, 0, result.stderr);
  const calls = await readFile(ghCallLog, "utf8");
  assert.match(calls, /pr ready feat\/sample-feature/);
  assert.doesNotMatch(calls, /pr merge/);
  const validationCommit = run(root, "git", ["log", "-1", "--format=%B"]);
  assert.match(validationCommit, /^chore\(sample-feature\): validate feature/m);
  assert.match(validationCommit, /^CC-SDD-Feature: sample-feature$/m);
  assert.match(validationCommit, /^CC-SDD-Task: VALIDATE$/m);
});

test("VALIDATE refuses an incomplete feature without marking its PR ready", async () => {
  const { root, ghCallLog } = await createRepository();
  const first = publish(root);
  assert.equal(first.status, 0, first.stderr);

  assertDenied(validateFeature(root));
  assert.doesNotMatch(await readFile(ghCallLog, "utf8"), /pr ready/);
});

test("VALIDATE retry is idempotent after the PR is already Ready", async () => {
  const { root } = await createRepository();
  const first = publish(root);
  assert.equal(first.status, 0, first.stderr);
  await publishSecondTask(root);
  const validated = validateFeature(root);
  assert.equal(validated.status, 0, validated.stderr);
  const validationHead = run(root, "git", ["rev-parse", "HEAD"]);

  const retried = validateFeature(root);

  assert.equal(retried.status, 0, retried.stderr);
  assert.equal(run(root, "git", ["rev-parse", "HEAD"]), validationHead);
});

test("denies a task ID that does not match the staged checkbox delta", async () => {
  const { root } = await createRepository();
  assertDenied(publish(root, { taskId: "1.2" }));
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

test("denies dotenv and private-key files even when declared", async () => {
  for (const deniedPath of [".env", "secrets/id_rsa"]) {
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

test("denies a tasks diff that completes more than the selected task", async () => {
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
