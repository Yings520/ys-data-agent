import { readFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

import {
  parseTasks,
  validateTasks,
  WorkflowInputError,
} from "./cc-sdd-to-ralph.mjs";
import {
  assertTaskPublished,
  assertValidationPublished,
} from "./cc-sdd-publish.mjs";

const TASK_ID = /^(?:\d+(?:\.\d+)*|VALIDATE)$/;
const COMPLETION_SENTINEL = ["<promise>", "COMPLETE", "</promise>"].join("");

async function readJson(filePath, label) {
  let contents;
  try {
    contents = await readFile(filePath, "utf8");
  } catch (error) {
    throw new WorkflowInputError(`Cannot read ${label}: ${error.message}`);
  }

  try {
    return JSON.parse(contents);
  } catch (error) {
    throw new WorkflowInputError(`Cannot parse ${label}: ${error.message}`);
  }
}

export async function authorizeCompletion(feature, taskId, root = process.cwd()) {
  if (!/^[a-z0-9][a-z0-9._-]*$/.test(feature ?? "")) {
    throw new WorkflowInputError("Invalid feature name");
  }
  if (!TASK_ID.test(taskId ?? "")) {
    throw new WorkflowInputError("Invalid task ID");
  }
  if (
    process.env.CC_SDD_DISPATCH_FEATURE !== feature ||
    process.env.CC_SDD_DISPATCH_TASK_ID !== taskId
  ) {
    throw new WorkflowInputError(
      "Completion request does not match the current Ralph dispatch",
    );
  }

  const featureDirectory = path.join(root, ".kiro", "specs", feature);
  const specPath = path.join(featureDirectory, "spec.json");
  const tasksPath = path.join(featureDirectory, "tasks.md");
  const spec = await readJson(specPath, specPath);
  if (spec?.approvals?.tasks?.approved !== true) {
    throw new WorkflowInputError(`cc-sdd tasks are not approved in ${specPath}`);
  }

  let markdown;
  try {
    markdown = await readFile(tasksPath, "utf8");
  } catch (error) {
    throw new WorkflowInputError(`Cannot read ${tasksPath}: ${error.message}`);
  }
  const tasks = validateTasks(parseTasks(markdown));

  if (taskId === "VALIDATE") {
    const incomplete = tasks.filter((task) => !task.passes).map((task) => task.id);
    if (incomplete.length > 0) {
      throw new WorkflowInputError(
        `Cannot complete VALIDATE; incomplete tasks: ${incomplete.join(", ")}`,
      );
    }
    assertValidationPublished({ feature, root, env: process.env });
    return COMPLETION_SENTINEL;
  }

  const selected = tasks.find((task) => task.id === taskId);
  if (!selected) {
    throw new WorkflowInputError("Unknown task ID");
  }
  if (!selected.passes) {
    throw new WorkflowInputError(
      `Task ${taskId} is not checked in authoritative tasks.md`,
    );
  }

  assertTaskPublished({ feature, taskId, root, env: process.env });
  return COMPLETION_SENTINEL;
}

export async function runCli(args, root = process.cwd()) {
  if (args.length !== 2) {
    throw new WorkflowInputError(
      "Usage: cc-sdd-complete.mjs <feature> <task-id|VALIDATE>",
    );
  }
  const sentinel = await authorizeCompletion(args[0], args[1], root);
  process.stdout.write(`${sentinel}\n`);
}

const isMain =
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href;

if (isMain) {
  runCli(process.argv.slice(2)).catch(() => {
    process.stderr.write("cc-sdd-complete: completion denied\n");
    process.exitCode = 1;
  });
}
