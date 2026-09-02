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

export function expectedBase(env = process.env) {
  return env.CC_SDD_PR_BASE || "master";
}

function runRtkGit(root, prefix, args) {
  const result = spawnSync("rtk", [...prefix, ...args], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: 16 * 1024 * 1024,
  });
  if (result.status !== 0) {
    throw new PublicationError("git operation failed");
  }
  return result.stdout;
}

function git(root, args) {
  // Publication consumes exact, machine-readable Git output (including NUL-delimited paths).
  // `rtk git` is intentionally human-oriented and may append compact diff markers, so use
  // RTK's raw tracked proxy for reads.
  return runRtkGit(root, ["proxy", "git"], args);
}

function mutateGit(root, args) {
  // Ralph's workspace permission profile authorizes the dedicated RTK Git command to mutate
  // `.git`; the generic raw proxy is read-only in that sandbox.
  return runRtkGit(root, ["git"], args);
}

function remoteHead(root, feature) {
  return git(root, [
    "ls-remote",
    "origin",
    `refs/heads/${expectedBranch(feature)}`,
  ])
    .trim()
    .split(/\s+/)[0];
}

export function assertRemoteContainsHead(root, feature) {
  const local = git(root, ["rev-parse", "HEAD"]).trim();
  if (remoteHead(root, feature) !== local) {
    throw new PublicationError("remote branch is behind");
  }
}

function gh(root, args, env = process.env, allowMissingPullRequest = false) {
  const result = spawnSync("gh", args, {
    cwd: root,
    encoding: "utf8",
    env,
    maxBuffer: 4 * 1024 * 1024,
  });
  if (result.status === 0) {
    return result.stdout;
  }
  if (
    allowMissingPullRequest &&
    /no pull requests? found/i.test(`${result.stdout}\n${result.stderr}`)
  ) {
    return null;
  }
  throw new PublicationError("GitHub PR operation failed");
}

function inspectPullRequest(root, feature, env = process.env) {
  const output = gh(
    root,
    [
      "pr",
      "view",
      expectedBranch(feature),
      "--json",
      "state,isDraft,baseRefName,headRefName,url",
    ],
    env,
    true,
  );
  if (output === null) {
    return null;
  }
  try {
    return JSON.parse(output);
  } catch {
    throw new PublicationError("GitHub PR metadata is invalid");
  }
}

function requireExpectedPullRequest(
  pullRequest,
  feature,
  env,
  expectedDraft,
) {
  if (
    pullRequest?.state !== "OPEN" ||
    pullRequest?.isDraft !== expectedDraft ||
    pullRequest?.baseRefName !== expectedBase(env) ||
    pullRequest?.headRefName !== expectedBranch(feature) ||
    typeof pullRequest?.url !== "string" ||
    pullRequest.url.length === 0
  ) {
    throw new PublicationError("GitHub PR does not match the feature contract");
  }
  return pullRequest;
}

function requireExpectedDraftPullRequest(pullRequest, feature, env) {
  return requireExpectedPullRequest(pullRequest, feature, env, true);
}

function requireExpectedReadyPullRequest(pullRequest, feature, env) {
  return requireExpectedPullRequest(pullRequest, feature, env, false);
}

export function ensureDraftPullRequest(root, feature, env = process.env) {
  const existing = inspectPullRequest(root, feature, env);
  if (existing !== null) {
    return requireExpectedDraftPullRequest(existing, feature, env);
  }

  gh(
    root,
    [
      "pr",
      "create",
      "--draft",
      "--base",
      expectedBase(env),
      "--head",
      expectedBranch(feature),
      "--title",
      `feat(${feature}): implement approved feature`,
      "--body",
      `Implements the approved cc-sdd feature ${feature}.`,
    ],
    env,
  );
  return requireExpectedDraftPullRequest(
    inspectPullRequest(root, feature, env),
    feature,
    env,
  );
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

async function readAuthoritativeTasks(root, feature) {
  const tasksPath = path.join(root, ".kiro", "specs", feature, "tasks.md");
  try {
    return validateTasks(parseTasks(await readFile(tasksPath, "utf8")));
  } catch {
    throw new PublicationError("authoritative tasks are unavailable");
  }
}

function commitBodies(root) {
  return git(root, ["log", "--format=%B%x1e"])
    .split("\x1e")
    .map((body) => body.trim())
    .filter(Boolean);
}

function hasTaskCommit(bodies, feature, taskId) {
  const featureTrailer = `CC-SDD-Feature: ${feature}`;
  const taskTrailer = `CC-SDD-Task: ${taskId}`;
  return bodies.some((body) => {
    const lines = body.split(/\r?\n/);
    return lines.includes(featureTrailer) && lines.includes(taskTrailer);
  });
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
  mutateGit(root, [
    "commit",
    "-m",
    subject,
    "-m",
    `${featureTrailer}\n${taskTrailer}`,
  ]);
  mutateGit(root, [
    "push",
    "origin",
    `HEAD:refs/heads/${expectedBranch(feature)}`,
  ]);
  assertRemoteContainsHead(root, feature);
  ensureDraftPullRequest(root, feature);
}

export async function recoverFeature(
  feature,
  root = process.cwd(),
  env = process.env,
) {
  if (!FEATURE_NAME.test(feature ?? "")) {
    throw new PublicationError("invalid feature identity");
  }
  const branch = git(root, ["branch", "--show-current"]).trim();
  if (branch !== expectedBranch(feature)) {
    throw new PublicationError("recovery branch does not match feature");
  }

  await requireApprovedSpec(root, feature);
  const checkedTasks = (await readAuthoritativeTasks(root, feature)).filter(
    (task) => task.passes,
  );
  if (checkedTasks.length === 0) {
    return;
  }

  const bodies = commitBodies(root);
  for (const task of checkedTasks) {
    if (!hasTaskCommit(bodies, feature, task.id)) {
      throw new PublicationError("checked task has no durable task commit");
    }
  }

  const local = git(root, ["rev-parse", "HEAD"]).trim();
  if (remoteHead(root, feature) !== local) {
    mutateGit(root, [
      "push",
      "origin",
      `HEAD:refs/heads/${expectedBranch(feature)}`,
    ]);
  }
  assertRemoteContainsHead(root, feature);
  ensureDraftPullRequest(root, feature, env);
}

export function assertTaskPublished({
  feature,
  taskId,
  root = process.cwd(),
  env = process.env,
}) {
  if (!FEATURE_NAME.test(feature ?? "") || !TASK_ID.test(taskId ?? "")) {
    throw new PublicationError("invalid published task identity");
  }
  const branch = git(root, ["branch", "--show-current"]).trim();
  if (branch !== expectedBranch(feature)) {
    throw new PublicationError("published task branch does not match feature");
  }

  const tasksPath = `.kiro/specs/${feature}/tasks.md`;
  const committedTasks = validateTasks(
    parseTasks(git(root, ["show", `HEAD:${tasksPath}`])),
  );
  if (!committedTasks.find((task) => task.id === taskId)?.passes) {
    throw new PublicationError("published task is not committed as complete");
  }
  if (!hasTaskCommit(commitBodies(root), feature, taskId)) {
    throw new PublicationError("published task commit trailers are missing");
  }

  assertRemoteContainsHead(root, feature);
  requireExpectedDraftPullRequest(
    inspectPullRequest(root, feature, env),
    feature,
    env,
  );
}

export async function publishValidation(
  feature,
  root = process.cwd(),
  env = process.env,
) {
  if (!FEATURE_NAME.test(feature ?? "")) {
    throw new PublicationError("invalid validation feature identity");
  }
  if (
    env.CC_SDD_DISPATCH_FEATURE !== feature ||
    env.CC_SDD_DISPATCH_TASK_ID !== "VALIDATE"
  ) {
    throw new PublicationError("validation does not match dispatch");
  }
  const branch = git(root, ["branch", "--show-current"]).trim();
  if (branch !== expectedBranch(feature)) {
    throw new PublicationError("validation branch does not match feature");
  }
  if (
    git(root, ["diff", "--cached", "--name-only", "-z"]).length !== 0
  ) {
    throw new PublicationError("validation cannot include staged changes");
  }

  await requireApprovedSpec(root, feature);
  const tasks = await readAuthoritativeTasks(root, feature);
  if (tasks.some((task) => !task.passes)) {
    throw new PublicationError("validation requires every task to be complete");
  }
  const bodies = commitBodies(root);
  if (tasks.some((task) => !hasTaskCommit(bodies, feature, task.id))) {
    throw new PublicationError("validation requires every task publication");
  }

  const currentHeadBody = git(root, ["log", "-1", "--format=%B"]);
  const auditAlreadyExists = hasTaskCommit(
    [currentHeadBody],
    feature,
    "VALIDATE",
  );
  if (!auditAlreadyExists) {
    assertRemoteContainsHead(root, feature);
    requireExpectedDraftPullRequest(
      inspectPullRequest(root, feature, env),
      feature,
      env,
    );
    mutateGit(root, [
      "commit",
      "--allow-empty",
      "-m",
      `chore(${feature}): validate feature`,
      "-m",
      `CC-SDD-Feature: ${feature}\nCC-SDD-Task: VALIDATE`,
    ]);
  }

  if (remoteHead(root, feature) !== git(root, ["rev-parse", "HEAD"]).trim()) {
    mutateGit(root, [
      "push",
      "origin",
      `HEAD:refs/heads/${expectedBranch(feature)}`,
    ]);
  }
  assertRemoteContainsHead(root, feature);
  const pullRequest = inspectPullRequest(root, feature, env);
  if (auditAlreadyExists && pullRequest?.isDraft === false) {
    requireExpectedReadyPullRequest(pullRequest, feature, env);
    return;
  }
  requireExpectedDraftPullRequest(
    pullRequest,
    feature,
    env,
  );
  gh(root, ["pr", "ready", expectedBranch(feature)], env);
  requireExpectedReadyPullRequest(
    inspectPullRequest(root, feature, env),
    feature,
    env,
  );
}

export function assertValidationPublished({
  feature,
  root = process.cwd(),
  env = process.env,
}) {
  if (!FEATURE_NAME.test(feature ?? "")) {
    throw new PublicationError("invalid validated feature identity");
  }
  const branch = git(root, ["branch", "--show-current"]).trim();
  if (branch !== expectedBranch(feature)) {
    throw new PublicationError("validated feature branch does not match");
  }
  const tasks = validateTasks(
    parseTasks(git(root, ["show", `HEAD:.kiro/specs/${feature}/tasks.md`])),
  );
  if (tasks.some((task) => !task.passes)) {
    throw new PublicationError("validated commit contains incomplete tasks");
  }
  const headBody = git(root, ["log", "-1", "--format=%B"]);
  if (!hasTaskCommit([headBody], feature, "VALIDATE")) {
    throw new PublicationError("validation audit commit is not HEAD");
  }
  assertRemoteContainsHead(root, feature);
  requireExpectedReadyPullRequest(
    inspectPullRequest(root, feature, env),
    feature,
    env,
  );
}

export async function runCli(args, root = process.cwd()) {
  if (args[0] === "--recover") {
    if (args.length !== 2) {
      throw new PublicationError("invalid recovery arguments");
    }
    await recoverFeature(args[1], root);
    return;
  }
  const { feature, taskId, paths } = parsePublishArgs(args);
  if (taskId === "VALIDATE") {
    if (paths.length !== 0) {
      throw new PublicationError("validation does not accept task paths");
    }
    await publishValidation(feature, root);
    return;
  }
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
