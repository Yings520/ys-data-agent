import { spawnSync } from "node:child_process";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { parseTasks, validateTasks } from "./cc-sdd-to-ralph.mjs";

const FEATURE_NAME = /^[a-z0-9][a-z0-9._-]*$/;
const TASK_ID = /^\d+(?:\.\d+)*$/;
const DENIED_STAGED_PATH =
  /^(?:\.ralph-tui\/|\.env(?:\.|$))|(?:^|\/)(?:id_rsa|[^/]+\.(?:pem|key|p12))$/i;

export class PublicationError extends Error {
  constructor(message) {
    super(message);
    this.name = "PublicationError";
  }
}

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

export function expectedBranch(feature) {
  return `feat/${feature}`;
}

function git(root, args) {
  const result = spawnSync("git", args, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new PublicationError("git operation failed");
  }
  return result.stdout;
}

function validateRelativePath(candidate) {
  if (
    typeof candidate !== "string" ||
    candidate.length === 0 ||
    candidate.includes("\0") ||
    /[\r\n]/.test(candidate) ||
    path.posix.isAbsolute(candidate) ||
    path.posix.normalize(candidate) !== candidate ||
    candidate === "." ||
    candidate.startsWith("../")
  ) {
    throw new PublicationError("invalid publication path");
  }
}

function samePathSet(left, right) {
  if (left.length !== right.length) {
    return false;
  }
  const sortedLeft = [...left].sort();
  const sortedRight = [...right].sort();
  return sortedLeft.every((value, index) => value === sortedRight[index]);
}

async function requireApprovedSpec(root, feature) {
  const specPath = path.join(root, ".kiro", "specs", feature, "spec.json");
  let spec;
  try {
    spec = JSON.parse(await readFile(specPath, "utf8"));
  } catch {
    throw new PublicationError("approved task spec unavailable");
  }
  if (spec?.approvals?.tasks?.approved !== true) {
    throw new PublicationError("task spec is not approved");
  }
}

function requireSelectedTaskDelta(root, taskId, tasksPath) {
  const before = validateTasks(parseTasks(git(root, ["show", `HEAD:${tasksPath}`])));
  const staged = validateTasks(parseTasks(git(root, ["show", `:${tasksPath}`])));
  const beforeById = new Map(before.map((task) => [task.id, task]));
  const stagedById = new Map(staged.map((task) => [task.id, task]));

  if (!samePathSet([...beforeById.keys()], [...stagedById.keys()])) {
    throw new PublicationError("task identities changed during publication");
  }

  const selected = stagedById.get(taskId);
  if (!selected?.passes) {
    throw new PublicationError("selected task is not complete");
  }

  const passStateDelta = [];
  for (const [id, previous] of beforeById) {
    const current = stagedById.get(id);
    if (previous.passes !== current.passes) {
      if (previous.passes || !current.passes) {
        throw new PublicationError("task completion was reversed");
      }
      passStateDelta.push(id);
    }
  }

  if (passStateDelta.length !== 1 || passStateDelta[0] !== taskId) {
    throw new PublicationError("task completion delta does not match dispatch");
  }
}

export async function publishTask(feature, taskId, paths, root = process.cwd()) {
  if (!FEATURE_NAME.test(feature ?? "") || !TASK_ID.test(taskId ?? "")) {
    throw new PublicationError("invalid feature or task identity");
  }
  if (
    process.env.CC_SDD_DISPATCH_FEATURE !== feature ||
    process.env.CC_SDD_DISPATCH_TASK_ID !== taskId
  ) {
    throw new PublicationError("publication does not match dispatch");
  }
  if (paths.length === 0 || new Set(paths).size !== paths.length) {
    throw new PublicationError("publication paths must be unique and nonempty");
  }
  for (const stagedPath of paths) {
    validateRelativePath(stagedPath);
    if (DENIED_STAGED_PATH.test(stagedPath)) {
      throw new PublicationError("publication path is denied");
    }
  }

  const branch = git(root, ["branch", "--show-current"]).trim();
  if (branch !== expectedBranch(feature)) {
    throw new PublicationError("publication branch does not match feature");
  }

  const stagedPaths = git(root, ["diff", "--cached", "--name-only", "-z"])
    .split("\0")
    .filter(Boolean);
  if (stagedPaths.length === 0 || !samePathSet(stagedPaths, paths)) {
    throw new PublicationError("staged paths do not match publication boundary");
  }

  const tasksPath = `.kiro/specs/${feature}/tasks.md`;
  if (!stagedPaths.includes(tasksPath)) {
    throw new PublicationError("authoritative tasks file is not staged");
  }

  await requireApprovedSpec(root, feature);
  requireSelectedTaskDelta(root, taskId, tasksPath);

  const [subject, featureTrailer, taskTrailer] = taskCommitMessage(
    feature,
    taskId,
  );
  git(root, [
    "commit",
    "-m",
    subject,
    "-m",
    `${featureTrailer}\n${taskTrailer}`,
  ]);
  git(root, [
    "push",
    "origin",
    `HEAD:refs/heads/${expectedBranch(feature)}`,
  ]);
}

export async function runCli(args, root = process.cwd()) {
  const { feature, taskId, paths } = parsePublishArgs(args);
  await publishTask(feature, taskId, paths, root);
}

const isMain =
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href;

if (isMain) {
  runCli(process.argv.slice(2)).catch(() => {
    process.stderr.write("cc-sdd-publish: publication denied\n");
    process.exitCode = 1;
  });
}
