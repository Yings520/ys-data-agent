# Ralph Task Policy Loading and Publishing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every Ralph-dispatched cc-sdd task load the two repository engineering policies, then create one scoped task commit, push it to a shared Feature branch, and update one Draft pull request before Ralph can report completion.

**Architecture:** Keep `$run-cc-sdd-task` as the only implementation entry point. Add policy references at the repository, dispatch, and skill layers; centralize Git/GitHub side effects in one Node publication helper; make the completion helper verify durable publication; and run idempotent recovery before rebuilding Ralph's disposable projection.

**Tech Stack:** Bash, Node.js ESM, `node:test`, Git CLI, GitHub CLI (`gh`), Ralph TUI 0.12, Codex CLI 0.151, Rust workspace quality gates.

---

## File Map

- Modify `AGENTS.md`: document the Ralph-only scope of both mandatory policies and the authorized Git boundary.
- Track `.ysda/agents/rust-engineer.md` and `.ysda/agents/code-change-pr-workflow.md` explicitly even though the rest of `.ysda/` remains ignored.
- Modify `.ralph-tui-prompt.hbs`: require both policies before invoking the task skill.
- Modify `.agents/skills/run-cc-sdd-task/SKILL.md`: load both policies completely, fail closed, stage explicit paths, publish, then complete.
- Modify `scripts/codex-ralph`: fail closed when a policy is unreadable and inject both complete policy documents into every formal Ralph task prompt.
- Create `tools/workflow/cc-sdd-publish.mjs`: validate staged paths, commit, push, create/reuse the Draft PR, recover incomplete publication, and verify publication state.
- Create `tools/workflow/cc-sdd-publish.test.mjs`: exercise the publication boundary with temporary Git repositories, a bare remote, and a fake `gh` executable.
- Modify `tools/workflow/cc-sdd-complete.mjs`: require remote commit reachability and the expected PR before authorizing completion.
- Modify `tools/workflow/cc-sdd-completion.test.mjs`: enforce policy references and publication-gated completion without leaking Ralph's completion signal.
- Modify `scripts/ralph-cc-sdd.sh`: recover publication before compiling the JSON projection.
- Modify `tools/workflow/ralph-cc-sdd-launcher.test.mjs`: verify recovery runs before projection generation and Ralph launch.
- Modify `.ralph-tui/config.toml` only if the existing merged config needs an explicit PR base; prefer `CC_SDD_PR_BASE=master` in the wrapper/helper contract to avoid adding unsupported Ralph keys.

### Task 1: Enforce Mandatory Policy Loading at Three Boundaries

**Files:**
- Modify: `tools/workflow/cc-sdd-completion.test.mjs`
- Track: `.ysda/agents/rust-engineer.md`
- Track: `.ysda/agents/code-change-pr-workflow.md`
- Modify: `AGENTS.md`
- Modify: `.ralph-tui-prompt.hbs`
- Modify: `.agents/skills/run-cc-sdd-task/SKILL.md`
- Modify: `scripts/codex-ralph`
- Modify: `tools/workflow/ralph-cc-sdd-launcher.test.mjs`

- [ ] **Step 1: Write the failing policy-contract test**

Add these constants and assertions to `tools/workflow/cc-sdd-completion.test.mjs`:

```js
const mandatoryPolicyPaths = [
  ".ysda/agents/rust-engineer.md",
  ".ysda/agents/code-change-pr-workflow.md",
];

test("Ralph instructions require both repository execution policies", async () => {
  const enforcementPaths = [
    "AGENTS.md",
    ".ralph-tui-prompt.hbs",
    ".agents/skills/run-cc-sdd-task/SKILL.md",
  ];

  for (const enforcementPath of enforcementPaths) {
    const contents = await readFile(
      path.join(projectRoot, enforcementPath),
      "utf8",
    );
    for (const policyPath of mandatoryPolicyPaths) {
      assert.match(contents, new RegExp(policyPath.replaceAll(".", "\\.")));
    }
    assert.match(contents, /read completely|完整读取/i);
    assert.match(contents, /blocked|阻塞/i);
  }
});
```

Also add both policy files to the existing `instructionPaths` array so the completion-signal leakage test scans them.

In `ralph-cc-sdd-launcher.test.mjs`, make the formal-dispatch wrapper fixture assert that the prompt logged by the fake Codex executable contains the original dispatch followed by the complete contents of both policy files. Add a missing-policy test that sets `CC_SDD_POLICY_ROOT` to an empty temporary directory and asserts the wrapper exits nonzero before invoking Codex. Keep the availability-probe test byte-for-byte unchanged and policy-free.

- [ ] **Step 2: Run the test and verify RED**

Run:

```bash
rtk node --test tools/workflow/cc-sdd-completion.test.mjs
```

Expected: FAIL because `AGENTS.md`, the prompt, and the task skill do not all name both policy paths.

- [ ] **Step 3: Add the minimal repository-level rule**

Add this section to `AGENTS.md` after `Allowed Project Skills`:

```markdown
## Ralph Task Execution Constraints

When and only when Ralph dispatches `$run-cc-sdd-task`, the Agent must read
`.ysda/agents/rust-engineer.md` and
`.ysda/agents/code-change-pr-workflow.md` completely before implementation.
Missing or unreadable policy files block the task. Their commit, push, and PR
authority is limited to the selected approved Feature task on its Feature
branch; they never authorize force-push, automatic merge, or unrelated changes.
```

Do not add the files to `Allowed Project Skills`; they are policies, not skills.
Use `git add -f` for exactly these two policy files because `/.ysda/` remains
ignored. Do not weaken or remove the existing `.gitignore` rule.

- [ ] **Step 4: Add dispatch and skill enforcement**

Insert before the existing skill invocation in `.ralph-tui-prompt.hbs`:

```markdown
2. Read `.ysda/agents/rust-engineer.md` and
   `.ysda/agents/code-change-pr-workflow.md` completely. If either read fails,
   report `STATUS: BLOCKED` and stop.
3. Invoke `$run-cc-sdd-task <feature> {{taskId}}`.
```

Renumber the remaining prompt steps. Add this `Mandatory Policies` section to `.agents/skills/run-cc-sdd-task/SKILL.md` after approval preflight:

```markdown
## Mandatory Policies

1. Read `.ysda/agents/rust-engineer.md` completely.
2. Read `.ysda/agents/code-change-pr-workflow.md` completely.
3. If either file is missing, unreadable, or truncated, stop with
   `STATUS: BLOCKED`. Do not implement or run the completion helper.
4. Apply both policies inside the selected task boundary. Git publication is
   authorized only through the repository publication helper described below.
```

In `scripts/codex-ralph`, after a formal dispatch identity is validated, resolve
the policy root from `${CC_SDD_POLICY_ROOT:-$PWD}`, require both files to be
readable, and append each file in full to `prompt_file` under an explicit policy
heading. Use a shell read loop so no policy contents are interpreted as shell
syntax:

```bash
mandatory_policies=(
  ".ysda/agents/rust-engineer.md"
  ".ysda/agents/code-change-pr-workflow.md"
)
policy_root="${CC_SDD_POLICY_ROOT:-$PWD}"
for policy in "${mandatory_policies[@]}"; do
  policy_path="${policy_root}/${policy}"
  if [[ ! -r "$policy_path" ]]; then
    printf '%s\n' 'codex-ralph: mandatory policy unavailable' >&2
    exit 2
  fi
  printf '\n## Mandatory Execution Policy: %s\n\n' "$policy" >> "$prompt_file"
  while IFS= read -r policy_line || [[ -n "$policy_line" ]]; do
    printf '%s\n' "$policy_line" >> "$prompt_file"
  done < "$policy_path"
done
```

- [ ] **Step 5: Run the test and verify GREEN**

Run:

```bash
rtk node --test tools/workflow/cc-sdd-completion.test.mjs \
  tools/workflow/ralph-cc-sdd-launcher.test.mjs
```

Expected: all completion tests pass and no scanned instruction contains the full completion signal.

- [ ] **Step 6: Commit the policy contract**

```bash
rtk git add -f .ysda/agents/rust-engineer.md \
  .ysda/agents/code-change-pr-workflow.md
rtk git add AGENTS.md .ralph-tui-prompt.hbs \
  .agents/skills/run-cc-sdd-task/SKILL.md \
  scripts/codex-ralph \
  tools/workflow/cc-sdd-completion.test.mjs \
  tools/workflow/ralph-cc-sdd-launcher.test.mjs
rtk git diff --cached --check
rtk git commit -m "fix(workflow): require Ralph execution policies"
```

### Task 2: Implement Atomic Task Commit and Push

**Files:**
- Create: `tools/workflow/cc-sdd-publish.mjs`
- Create: `tools/workflow/cc-sdd-publish.test.mjs`

- [ ] **Step 1: Build a real temporary Git fixture in the test**

Create a test helper that initializes a work repository and bare remote, configures a local author, creates `master`, creates `feat/sample-feature`, and writes an approved checked task:

```js
function run(root, command, args, env = process.env) {
  const result = spawnSync(command, args, { cwd: root, encoding: "utf8", env });
  assert.equal(result.status, 0, `${command}: ${result.stderr}`);
  return result.stdout.trim();
}

async function createRepository() {
  const root = await mkdtemp(path.join(tmpdir(), "cc-sdd-publish-"));
  const remote = `${root}-remote.git`;
  run(root, "git", ["init", "-b", "master"]);
  run(root, "git", ["config", "user.name", "Ralph Test"]);
  run(root, "git", ["config", "user.email", "ralph@example.test"]);
  await writeFile(path.join(root, "README.md"), "fixture\n");
  run(root, "git", ["add", "README.md"]);
  run(root, "git", ["commit", "-m", "test: initialize fixture"]);
  run(root, "git", ["init", "--bare", remote]);
  run(root, "git", ["remote", "add", "origin", remote]);
  run(root, "git", ["push", "-u", "origin", "master"]);
  run(root, "git", ["switch", "-c", "feat/sample-feature"]);
  return { root, remote };
}
```

The fixture must create `.kiro/specs/sample-feature/tasks.md` and `spec.json` with task `1.1` checked, then stage exactly the paths passed through repeated `--path` arguments.

- [ ] **Step 2: Write failing publication tests**

Add tests that invoke:

```bash
node tools/workflow/cc-sdd-publish.mjs sample-feature 1.1 \
  --path .kiro/specs/sample-feature/tasks.md \
  --path src/provider.rs
```

Assert:

```js
assert.match(commitBody, /CC-SDD-Feature: sample-feature/);
assert.match(commitBody, /CC-SDD-Task: 1\.1/);
assert.equal(remoteHead, localHead);
assert.deepEqual(stagedAfterPublish, []);
```

Add negative cases for a dispatch mismatch, branch other than `feat/sample-feature`, a staged file not declared by `--path`, `.ralph-tui/session.json`, `.env`, an empty staged diff, a missing staged `tasks.md`, and a `tasks.md` diff that checks any leaf other than the dispatched task. Every failure must print only `cc-sdd-publish: publication denied` to stderr.

- [ ] **Step 3: Run the publication test and verify RED**

Run:

```bash
rtk node --test tools/workflow/cc-sdd-publish.test.mjs
```

Expected: FAIL because `cc-sdd-publish.mjs` does not exist.

- [ ] **Step 4: Implement CLI validation and task commit**

Create `tools/workflow/cc-sdd-publish.mjs` with these exported boundaries:

```js
export class PublicationError extends Error {}

export function parsePublishArgs(args) {
  const [feature, taskId, ...rest] = args;
  const paths = [];
  for (let index = 0; index < rest.length; index += 2) {
    if (rest[index] !== "--path" || !rest[index + 1]) {
      throw new PublicationError("invalid publication arguments");
    }
    paths.push(rest[index + 1]);
  }
  return { feature, taskId, paths };
}

export function taskCommitMessage(feature, taskId) {
  return [
    `feat(${feature}): complete task ${taskId}`,
    `CC-SDD-Feature: ${feature}`,
    `CC-SDD-Task: ${taskId}`,
  ];
}
```

The implementation must use `spawnSync` with argument arrays, never a shell string. It must validate feature/task syntax, dispatch environment identity, exact branch name, approved checked task state through `parseTasks`/`validateTasks`, exact equality between staged paths and repeated `--path` values, and the denylist below:

```js
const DENIED_STAGED_PATH =
  /^(?:\.ralph-tui\/|\.env(?:\.|$))|(?:^|\/)(?:id_rsa|[^/]+\.(?:pem|key|p12))$/i;
```

Read the staged `tasks.md` with `git show :<path>` and the last committed copy
with `git show HEAD:<path>`. Parse both and require the pass-state delta to equal
exactly `[taskId]`; this is the mechanical proof that the iteration did not
complete another leaf. The staged path set must include that `tasks.md` path.

Create the commit with separate message arguments:

```js
git(root, [
  "commit",
  "-m", subject,
  "-m", `${featureTrailer}\n${taskTrailer}`,
]);
git(root, ["push", "origin", `HEAD:refs/heads/feat/${feature}`]);
```

- [ ] **Step 5: Run the test and verify commit/push GREEN**

Run:

```bash
rtk node --test tools/workflow/cc-sdd-publish.test.mjs
```

Expected: task commit, trailer, denylist, branch, dispatch, and remote-head tests pass.

- [ ] **Step 6: Commit the Git publication core**

```bash
rtk git add tools/workflow/cc-sdd-publish.mjs \
  tools/workflow/cc-sdd-publish.test.mjs
rtk git diff --cached --check
rtk git commit -m "feat(workflow): publish atomic Ralph task commits"
```

### Task 3: Add Draft PR Lifecycle and Idempotent Recovery

**Files:**
- Modify: `tools/workflow/cc-sdd-publish.mjs`
- Modify: `tools/workflow/cc-sdd-publish.test.mjs`

- [ ] **Step 1: Add a fake `gh` executable to the fixture**

The fake executable must log arguments to `GH_CALL_LOG`, return `GH_PR_JSON` for `pr view`, print a stable URL for `pr create`, and fail when `GH_FAIL=1`. Tests must prepend its directory to `PATH`; no real GitHub call occurs.

```bash
case "$1 $2" in
  "pr view") printf '%s\n' "$GH_PR_JSON" ;;
  "pr create") printf '%s\n' 'https://github.example/pull/1' ;;
  "pr ready") printf '%s\n' 'ready' ;;
  *) exit 64 ;;
esac
```

- [ ] **Step 2: Write failing PR and recovery tests**

Cover these observable cases:

- no PR: `gh pr create --draft --base master --head feat/sample-feature` runs;
- existing open Draft PR with the same head/base: it is reused;
- wrong head, wrong base, closed PR, or `gh` failure: publication is denied;
- local task commit exists but the remote head is behind: `--recover sample-feature` pushes it;
- remote branch exists but PR creation previously failed: recovery creates the Draft PR;
- a checked task without a matching commit trailer: recovery refuses to regenerate Ralph state.

- [ ] **Step 3: Run the tests and verify RED**

```bash
rtk node --test tools/workflow/cc-sdd-publish.test.mjs
```

Expected: FAIL on missing PR inspection and missing `--recover` support.

- [ ] **Step 4: Implement PR verification and recovery**

Add:

```js
export function expectedBranch(feature) {
  return `feat/${feature}`;
}

export function expectedBase(env = process.env) {
  return env.CC_SDD_PR_BASE || "master";
}

export function assertRemoteContainsHead(root, feature) {
  const local = git(root, ["rev-parse", "HEAD"]);
  const remoteLine = git(root, [
    "ls-remote", "origin", `refs/heads/${expectedBranch(feature)}`,
  ]);
  if (remoteLine.split(/\s+/)[0] !== local) {
    throw new PublicationError("remote branch is behind");
  }
}
```

Use `gh pr view <branch> --json state,isDraft,baseRefName,headRefName,url`; if it reports no PR, call `gh pr create --draft` and inspect again. Implement `recoverFeature(feature)` by validating every checked task has one commit containing both durable trailers, pushing the current branch when its remote head differs, then ensuring the Draft PR.

- [ ] **Step 5: Verify GREEN and sanitized failures**

```bash
rtk node --test tools/workflow/cc-sdd-publish.test.mjs
```

Expected: PR creation/reuse, remote recovery, missing-trailer rejection, and sanitized CLI-output tests pass.

- [ ] **Step 6: Commit PR lifecycle support**

```bash
rtk git add tools/workflow/cc-sdd-publish.mjs \
  tools/workflow/cc-sdd-publish.test.mjs
rtk git commit -m "feat(workflow): recover Ralph task publication"
```

### Task 4: Gate Ralph Completion and Launcher Startup on Publication

**Files:**
- Modify: `tools/workflow/cc-sdd-complete.mjs`
- Modify: `tools/workflow/cc-sdd-completion.test.mjs`
- Modify: `scripts/ralph-cc-sdd.sh`
- Modify: `tools/workflow/ralph-cc-sdd-launcher.test.mjs`
- Modify: `.agents/skills/run-cc-sdd-task/SKILL.md`

- [ ] **Step 1: Write failing completion publication tests**

Extend the completion fixture into a temporary Git repository. Add two tests:

```js
test("completion rejects a checked task before publication", async () => {
  const result = runHelper(root, "1.1");
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /completion denied/i);
});

test("completion accepts a remotely reachable task commit with an open PR", async () => {
  publishFixtureTask(root, "1.1");
  const result = runHelper(root, "1.1", publishedEnvironment);
  assert.equal(result.status, 0, result.stderr);
  assert.equal(result.stdout.trim(), expectedSentinel);
});
```

The fake `gh` response must identify `feat/sample-feature`, base `master`, state `OPEN`, and Draft status `true`.

- [ ] **Step 2: Write the failing launcher-order test**

Change the expected call order in `ralph-cc-sdd-launcher.test.mjs` to:

```js
[
  "node tools/workflow/cc-sdd-publish.mjs --recover sample-feature",
  "node tools/workflow/cc-sdd-to-ralph.mjs sample-feature",
  "ralph-tui run --prd .ralph-tui/generated/sample-feature.json --serial",
]
```

- [ ] **Step 3: Verify RED**

```bash
rtk node --test tools/workflow/cc-sdd-completion.test.mjs \
  tools/workflow/ralph-cc-sdd-launcher.test.mjs
```

Expected: completion still succeeds without publication and the launcher omits recovery.

- [ ] **Step 4: Add publication to the completion gate**

Import `assertTaskPublished` from `cc-sdd-publish.mjs` and call it after authoritative checkbox validation but before returning the completion signal:

```js
await assertTaskPublished({ feature, taskId, root, env: process.env });
return COMPLETION_SENTINEL;
```

Keep the existing CLI catch fixed to `cc-sdd-complete: completion denied`; never surface Git, task metadata, or `gh` output.

- [ ] **Step 5: Add recovery before projection compilation**

In `scripts/ralph-cc-sdd.sh`, insert:

```bash
CC_SDD_PR_BASE="${CC_SDD_PR_BASE:-master}" \
  rtk node tools/workflow/cc-sdd-publish.mjs --recover "${feature}"
rtk node tools/workflow/cc-sdd-to-ralph.mjs "${feature}"
```

Update the task skill's final sequence to require explicit `--path` values, publication, and then completion:

```text
rtk node tools/workflow/cc-sdd-publish.mjs <feature> <task-id> \
  --path <reviewed-path> [--path <reviewed-path> ...]
rtk node tools/workflow/cc-sdd-complete.mjs <feature> <task-id>
```

The completion helper remains the final shell action.

- [ ] **Step 6: Verify GREEN**

```bash
rtk node --test tools/workflow/cc-sdd-completion.test.mjs \
  tools/workflow/cc-sdd-publish.test.mjs \
  tools/workflow/ralph-cc-sdd-launcher.test.mjs \
  tools/workflow/cc-sdd-to-ralph.test.mjs
```

Expected: all workflow tests pass; blocked publication produces no completion signal.

- [ ] **Step 7: Commit completion and launcher integration**

```bash
rtk git add tools/workflow/cc-sdd-complete.mjs \
  tools/workflow/cc-sdd-completion.test.mjs \
  scripts/ralph-cc-sdd.sh \
  tools/workflow/ralph-cc-sdd-launcher.test.mjs \
  .agents/skills/run-cc-sdd-task/SKILL.md
rtk git commit -m "fix(workflow): gate Ralph completion on publication"
```

### Task 5: Make VALIDATE Mark the Shared PR Ready

**Files:**
- Modify: `tools/workflow/cc-sdd-publish.mjs`
- Modify: `tools/workflow/cc-sdd-publish.test.mjs`
- Modify: `.agents/skills/run-cc-sdd-task/SKILL.md`

- [ ] **Step 1: Write failing VALIDATE tests**

Create a fixture where every task has a published trailer commit and the PR is Draft. Invoke publication with `VALIDATE` and assert:

```js
assert.match(ghCalls, /pr ready/);
assert.match(validationCommit, /CC-SDD-Task: VALIDATE/);
```

Add a negative case where one task is incomplete; `gh pr ready` must not run.

- [ ] **Step 2: Verify RED**

```bash
rtk node --test tools/workflow/cc-sdd-publish.test.mjs
```

Expected: FAIL because VALIDATE is not implemented.

- [ ] **Step 3: Implement VALIDATE publication**

After Feature-wide `GO`, create an audit commit even when no files changed:

```js
git(root, [
  "commit", "--allow-empty",
  "-m", `chore(${feature}): validate feature`,
  "-m", `CC-SDD-Feature: ${feature}\nCC-SDD-Task: VALIDATE`,
]);
git(root, ["push", "origin", `HEAD:refs/heads/${expectedBranch(feature)}`]);
gh(root, ["pr", "ready", expectedBranch(feature)]);
```

Completion for `VALIDATE` must verify the PR is open and no longer Draft. It must never merge the PR.

- [ ] **Step 4: Verify GREEN**

```bash
rtk node --test tools/workflow/cc-sdd-publish.test.mjs \
  tools/workflow/cc-sdd-completion.test.mjs
```

Expected: VALIDATE readiness and incomplete-task rejection pass.

- [ ] **Step 5: Commit VALIDATE publication**

```bash
rtk git add tools/workflow/cc-sdd-publish.mjs \
  tools/workflow/cc-sdd-publish.test.mjs \
  .agents/skills/run-cc-sdd-task/SKILL.md
rtk git commit -m "feat(workflow): ready validated Feature PRs"
```

### Task 6: Reconcile Existing Tasks 1.1 and 1.2 Without Capturing Future Work

**Files:**
- Stage approved spec files and only task-owned code listed below.
- Preserve unstaged: `crates/ys-agent-core/src/ports.rs`, `crates/ys-agent-core/tests/provider_ports_test.rs`, and any future-task implementation not owned by 1.1 or 1.2.

- [ ] **Step 1: Audit the dirty tree before staging**

```bash
rtk git status --short
rtk git diff --check
rtk git diff --stat
rtk node tools/workflow/cc-sdd-to-ralph.mjs provider-management --check
```

Expected: tasks `1.1` and `1.2` are checked, `1.3` is unchecked, and no `_Blocked:` annotation exists.

- [ ] **Step 2: Commit approved Feature specification state**

Stage only:

```bash
rtk git add docs/PRD.md \
  .kiro/specs/provider-management/spec.json \
  .kiro/specs/provider-management/requirements.md \
  .kiro/specs/provider-management/design.md \
  .kiro/specs/provider-management/tasks.md
rtk git diff --cached --check
rtk git commit -m "docs(provider-management): approve Feature specification"
```

- [ ] **Step 3: Reconstruct the task 1.1 commit**

Stage the dependency and module-skeleton surface only:

```bash
rtk git add Cargo.toml Cargo.lock \
  crates/ys-agent-adapters/Cargo.toml \
  crates/ys-agent-adapters/src/lib.rs \
  crates/ys-agent-adapters/src/model/mod.rs \
  crates/ys-agent-adapters/src/model/discovery.rs \
  crates/ys-agent-adapters/src/model/liter.rs \
  crates/ys-agent-adapters/src/model/liter_chat.rs \
  crates/ys-agent-adapters/src/model/liter_responses.rs \
  crates/ys-agent-adapters/src/credential/mod.rs \
  crates/ys-agent-adapters/src/credential/keyring.rs \
  crates/ys-agent-adapters/src/oauth/mod.rs \
  crates/ys-agent-adapters/src/oauth/chatgpt.rs \
  crates/ys-agent-runtime/src/lib.rs \
  crates/ys-agent-runtime/src/provider/mod.rs \
  crates/ys-agent-runtime/src/provider/catalog.rs \
  crates/ys-agent-runtime/src/provider/evidence.rs \
  crates/ys-agent-runtime/src/provider/resolver.rs \
  crates/ys-agent-runtime/src/provider/service.rs \
  crates/ys-agent-runtime/src/provider/validation.rs \
  apps/ysda/src/tui/mod.rs \
  apps/ysda/src/tui/provider_management.rs
rtk git diff --cached --check
rtk git commit -m "feat(provider-management): complete task 1.1" \
  -m $'CC-SDD-Feature: provider-management\nCC-SDD-Task: 1.1'
```

Run `rtk cargo check --workspace --locked` before the commit; it must pass with these module skeletons present.

- [ ] **Step 4: Reconstruct the task 1.2 commit**

Stage only:

```bash
rtk git add crates/ys-agent-core/Cargo.toml \
  crates/ys-agent-core/src/ids.rs \
  crates/ys-agent-core/src/lib.rs \
  crates/ys-agent-core/src/provider.rs \
  crates/ys-agent-core/tests/provider_domain_test.rs
rtk cargo test -p ys-agent-core --test provider_domain_test
rtk cargo clippy -p ys-agent-core --all-targets -- -D warnings
rtk git diff --cached --check
rtk git commit -m "feat(provider-management): complete task 1.2" \
  -m $'CC-SDD-Feature: provider-management\nCC-SDD-Task: 1.2'
```

- [ ] **Step 5: Prove task 1.3 remains uncommitted**

```bash
rtk git status --short -- \
  crates/ys-agent-core/src/ports.rs \
  crates/ys-agent-core/tests/provider_ports_test.rs
```

Expected: both paths remain modified/untracked for Ralph task `1.3`; neither appears in `git show --name-only HEAD`.

- [ ] **Step 6: Push and create the shared Draft PR**

```bash
rtk git push -u origin feat/provider-management
rtk gh pr create \
  --draft \
  --base master \
  --head feat/provider-management \
  --title "feat(provider-management): add governed Provider lifecycle" \
  --body-file .kiro/specs/provider-management/requirements.md
```

Then verify:

```bash
rtk gh pr view feat/provider-management \
  --json state,isDraft,baseRefName,headRefName,url
```

Expected: state `OPEN`, Draft `true`, base `master`, head `feat/provider-management`.

### Task 7: Run Full Verification and Resume at Task 1.3

**Files:**
- No new source files unless a verification failure identifies an in-scope defect.

- [ ] **Step 1: Run workflow tests**

```bash
rtk node --test \
  tools/workflow/cc-sdd-publish.test.mjs \
  tools/workflow/cc-sdd-completion.test.mjs \
  tools/workflow/ralph-cc-sdd-launcher.test.mjs \
  tools/workflow/cc-sdd-to-ralph.test.mjs
```

Expected: every test passes with zero failures.

- [ ] **Step 2: Run repository quality gates**

```bash
rtk cargo fmt --all --check
rtk cargo check --workspace --locked
rtk cargo test --workspace --locked
rtk cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
rtk git diff --check
```

Expected: fmt/check/test/Clippy/diff checks pass.

- [ ] **Step 3: Verify publication recovery is clean**

```bash
CC_SDD_PR_BASE=master \
  rtk node tools/workflow/cc-sdd-publish.mjs --recover provider-management
rtk node tools/workflow/cc-sdd-to-ralph.mjs provider-management
```

Expected: recovery makes no duplicate commit or PR; regenerated projection reports tasks `1.1` and `1.2` complete and `1.3` open.

- [ ] **Step 4: Run a one-iteration real Ralph verification**

```bash
rtk ralph-tui run \
  --prd .ralph-tui/generated/provider-management.json \
  --serial --iterations 1 --no-tui --no-setup
```

Expected: task `1.3` reads both policy files, completes fresh Rust checks, creates exactly one `1.3` trailer commit, pushes it, updates the existing Draft PR, and only then reports completion.

- [ ] **Step 5: Archive the one-iteration runtime state and regenerate**

Move only the just-created Ralph session files into a named directory below `.ralph-tui/iterations/`, then run:

```bash
rtk node tools/workflow/cc-sdd-to-ralph.mjs provider-management
rtk node tools/workflow/cc-sdd-to-ralph.mjs provider-management --check
```

Expected: no active lock remains and the normal launcher can continue at the next open task.

- [ ] **Step 6: Report the handoff**

Report the Draft PR URL, branch, commit IDs for tasks `1.1` through `1.3`, exact test counts, and the next open task. Do not claim CI success until GitHub reports it.
