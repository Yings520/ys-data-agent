import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const projectRoot = fileURLToPath(new URL("../../", import.meta.url));
const launcherPath = path.join(projectRoot, "scripts/ralph-cc-sdd.sh");
const codexWrapperPath = path.join(projectRoot, "scripts/codex-ralph");
const configPath = path.join(projectRoot, ".ralph-tui/config.toml");

test("launcher uses arguments supported by the current Ralph CLI", async () => {
  const stubDirectory = await mkdtemp(path.join(tmpdir(), "ralph-launcher-"));
  const logPath = path.join(stubDirectory, "calls.log");
  const rtkPath = path.join(stubDirectory, "rtk");

  await writeFile(
    rtkPath,
    `#!/usr/bin/env bash
printf '%s\\n' "$*" >> "$RALPH_LAUNCH_LOG"
for argument in "$@"; do
  if [[ "$argument" == "--on-error" ]]; then
    exit 64
  fi
done
`,
    "utf8",
  );
  await chmod(rtkPath, 0o755);

  const result = spawnSync("/bin/bash", [launcherPath, "sample-feature"], {
    cwd: projectRoot,
    encoding: "utf8",
    env: {
      ...process.env,
      PATH: `${stubDirectory}:${process.env.PATH ?? ""}`,
      RALPH_LAUNCH_LOG: logPath,
    },
  });

  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual((await readFile(logPath, "utf8")).trim().split("\n"), [
    "node tools/workflow/cc-sdd-publish.mjs --recover sample-feature",
    "node tools/workflow/cc-sdd-to-ralph.mjs sample-feature",
    "ralph-tui run --prd .ralph-tui/generated/sample-feature.json --serial",
  ]);
});

test("project config aborts the loop after a failed iteration", async () => {
  const config = await readFile(configPath, "utf8");

  assert.match(
    config,
    /\[errorHandling\][\s\S]*?strategy\s*=\s*"abort"/,
  );
  assert.match(config, /continueOnNonZeroExit\s*=\s*false/);
});

test("Codex wrapper binds the completion helper to the Ralph dispatch", async () => {
  const stubDirectory = await mkdtemp(path.join(tmpdir(), "codex-wrapper-"));
  const environmentLog = path.join(stubDirectory, "environment.log");
  const argumentLog = path.join(stubDirectory, "arguments.log");
  const promptLog = path.join(stubDirectory, "prompt.log");
  const codexPath = path.join(stubDirectory, "codex");
  const policyRoot = path.join(stubDirectory, "policy-root");
  const policyDirectory = path.join(policyRoot, ".ysda/agents");
  await mkdir(policyDirectory, { recursive: true });
  await writeFile(
    path.join(policyDirectory, "rust-engineer.md"),
    "RUST_POLICY_SENTINEL\n",
    "utf8",
  );
  await writeFile(
    path.join(policyDirectory, "code-change-pr-workflow.md"),
    "CHANGE_POLICY_SENTINEL\n",
    "utf8",
  );
  const prompt = `## Current cc-sdd Work Item

- ID: 6.1
- Title: Implement provider screen

## Dispatch Context

Feature: provider-management
Spec: .kiro/specs/provider-management
`;

  await writeFile(
    codexPath,
    `#!/usr/bin/env bash
printf '%s\n' "$@" > "$CODEX_ARGUMENT_LOG"
printf '%s\n%s\n' "$CC_SDD_DISPATCH_FEATURE" "$CC_SDD_DISPATCH_TASK_ID" > "$CODEX_ENV_LOG"
cat > "$CODEX_PROMPT_LOG"
`,
    "utf8",
  );
  await chmod(codexPath, 0o755);

  const result = spawnSync(
    "/bin/bash",
    [codexWrapperPath, "exec", "--json", "--sandbox", "workspace-write", "-"],
    {
    cwd: projectRoot,
    encoding: "utf8",
    input: prompt,
    env: {
      ...process.env,
      PATH: `${stubDirectory}:${process.env.PATH ?? ""}`,
      CC_SDD_POLICY_ROOT: policyRoot,
      CODEX_ARGUMENT_LOG: argumentLog,
      CODEX_ENV_LOG: environmentLog,
      CODEX_PROMPT_LOG: promptLog,
    },
    },
  );

  assert.equal(result.status, 0, result.stderr);
  assert.equal(
    await readFile(environmentLog, "utf8"),
    "provider-management\n6.1\n",
  );
  const codexArguments = (await readFile(argumentLog, "utf8")).trim().split("\n");
  assert.equal(codexArguments.includes("--sandbox"), false);
  assert.equal(codexArguments.includes("workspace-write"), false);
  assert.ok(codexArguments.includes('default_permissions="ralph_local_tests"'));
  assert.ok(
    codexArguments.includes(
      'permissions.ralph_local_tests.extends=":workspace"',
    ),
  );
  assert.ok(
    codexArguments.includes(
      "permissions.ralph_local_tests.network.enabled=true",
    ),
  );
  assert.ok(
    codexArguments.includes(
      "permissions.ralph_local_tests.network.allow_local_binding=true",
    ),
  );
  const loggedPrompt = await readFile(promptLog, "utf8");
  assert.ok(loggedPrompt.startsWith(prompt));
  assert.match(loggedPrompt, /RUST_POLICY_SENTINEL/);
  assert.match(loggedPrompt, /CHANGE_POLICY_SENTINEL/);
});

test("Codex wrapper fails closed before launch when a policy is missing", async () => {
  const stubDirectory = await mkdtemp(path.join(tmpdir(), "codex-policy-"));
  const codexPath = path.join(stubDirectory, "codex");
  const invocationMarker = path.join(stubDirectory, "codex-invoked");
  const policyRoot = path.join(stubDirectory, "empty-policy-root");
  await mkdir(policyRoot, { recursive: true });
  await writeFile(
    codexPath,
    `#!/usr/bin/env bash
printf '%s\n' invoked > "$CODEX_INVOCATION_MARKER"
`,
    "utf8",
  );
  await chmod(codexPath, 0o755);

  const prompt = `## Current cc-sdd Work Item

- ID: 1.3
- Title: Define provider ports

## Dispatch Context

Feature: provider-management
Spec: .kiro/specs/provider-management
`;
  const result = spawnSync(
    "/bin/bash",
    [codexWrapperPath, "exec", "--json", "--sandbox", "workspace-write", "-"],
    {
      cwd: projectRoot,
      encoding: "utf8",
      input: prompt,
      env: {
        ...process.env,
        PATH: `${stubDirectory}:${process.env.PATH ?? ""}`,
        CC_SDD_POLICY_ROOT: policyRoot,
        CODEX_INVOCATION_MARKER: invocationMarker,
      },
    },
  );

  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /mandatory policy unavailable/i);
  await assert.rejects(readFile(invocationMarker, "utf8"));
});

test("Codex wrapper passes Ralph's availability probe through unchanged", async () => {
  const stubDirectory = await mkdtemp(path.join(tmpdir(), "codex-probe-"));
  const environmentLog = path.join(stubDirectory, "environment.log");
  const promptLog = path.join(stubDirectory, "prompt.log");
  const codexPath = path.join(stubDirectory, "codex");
  const prompt = 'Reply with just the word "ok".';

  await writeFile(
    codexPath,
    `#!/usr/bin/env bash
printf '%s\n%s\n' "\${CC_SDD_DISPATCH_FEATURE:-}" "\${CC_SDD_DISPATCH_TASK_ID:-}" > "$CODEX_ENV_LOG"
cat > "$CODEX_PROMPT_LOG"
`,
    "utf8",
  );
  await chmod(codexPath, 0o755);

  const result = spawnSync("/bin/bash", [codexWrapperPath, "exec", "--json", "-"], {
    cwd: projectRoot,
    encoding: "utf8",
    input: prompt,
    env: {
      ...process.env,
      PATH: `${stubDirectory}:${process.env.PATH ?? ""}`,
      CODEX_ENV_LOG: environmentLog,
      CODEX_PROMPT_LOG: promptLog,
    },
  });

  assert.equal(result.status, 0, result.stderr);
  assert.equal(await readFile(environmentLog, "utf8"), "\n\n");
  assert.equal(await readFile(promptLog, "utf8"), prompt);
});

test("Codex wrapper aborts Ralph when the final task result is blocked", async () => {
  const stubDirectory = await mkdtemp(path.join(tmpdir(), "codex-blocked-"));
  const codexPath = path.join(stubDirectory, "codex");
  const prompt = `## Current cc-sdd Work Item

- ID: 1.1
- Title: Implement dependency surface

## Dispatch Context

Feature: provider-management
Spec: .kiro/specs/provider-management
`;
  const rawJsonl = `${JSON.stringify({
    type: "item.completed",
    item: {
      type: "command_execution",
      aggregated_output: "The skill documentation mentions STATUS: BLOCKED.",
    },
  })}\n`;

  await writeFile(
    codexPath,
    `#!/usr/bin/env bash
last_message_file=""
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    -o|--output-last-message)
      last_message_file="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
printf '%s' "$CODEX_RAW_JSONL"
if [[ -n "$last_message_file" ]]; then
  printf '%s\n' 'STATUS: BLOCKED' > "$last_message_file"
fi
`,
    "utf8",
  );
  await chmod(codexPath, 0o755);

  const result = spawnSync("/bin/bash", [codexWrapperPath, "exec", "--json", "-"], {
    cwd: projectRoot,
    encoding: "utf8",
    input: prompt,
    env: {
      ...process.env,
      PATH: `${stubDirectory}:${process.env.PATH ?? ""}`,
      CODEX_RAW_JSONL: rawJsonl,
    },
  });

  assert.notEqual(result.status, 0);
  assert.equal(result.stdout, rawJsonl);
});
