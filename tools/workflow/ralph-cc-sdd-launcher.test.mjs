import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const projectRoot = fileURLToPath(new URL("../../", import.meta.url));
const launcherPath = path.join(projectRoot, "scripts/ralph-cc-sdd.sh");
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
