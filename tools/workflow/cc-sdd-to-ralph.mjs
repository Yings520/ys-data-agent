import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

export class WorkflowInputError extends Error {
  constructor(message) {
    super(message);
    this.name = "WorkflowInputError";
  }
}

const TASK_LINE = /^- \[([ xX])\](\*)?\s+(\d+(?:\.\d+)*)\.?\s+(.+?)\s*$/;

function splitCsv(value) {
  return value
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
}

function annotation(body, name) {
  const match = body.match(new RegExp(`_${name}:\\s*([^_]+)_`, "i"));
  return match?.[1].trim();
}

export function parseTasks(markdown) {
  const parsed = [];
  let current;

  for (const line of markdown.split(/\r?\n/)) {
    const match = line.match(TASK_LINE);
    if (match) {
      current = {
        id: match[3],
        title: match[4].replace(/\s+\(P\)\s*$/, ""),
        passes: match[1].toLowerCase() === "x",
        optional: Boolean(match[2]),
        lines: [],
      };
      parsed.push(current);
      continue;
    }

    if (current) {
      current.lines.push(line);
    }
  }

  const leaves = parsed.filter(
    (candidate) =>
      !parsed.some(
        (other) =>
          other !== candidate && other.id.startsWith(`${candidate.id}.`),
      ),
  );

  return leaves.map((task) => {
    const body = task.lines.join("\n");
    const requirementsValue = annotation(body, "Requirements");
    const boundary = annotation(body, "Boundary");
    const dependsValue = annotation(body, "Depends");

    if (!requirementsValue) {
      throw new WorkflowInputError(
        `Task ${task.id} is missing _Requirements:_`,
      );
    }
    if (!boundary) {
      throw new WorkflowInputError(`Task ${task.id} is missing _Boundary:_`);
    }
    if (!dependsValue) {
      throw new WorkflowInputError(`Task ${task.id} is missing _Depends:_`);
    }

    const details = task.lines
      .map((line) => line.match(/^\s{2,}-\s+(.+?)\s*$/)?.[1])
      .filter(Boolean)
      .filter((line) => !/^_(Requirements|Boundary|Depends|Blocked):/i.test(line));

    if (details.length === 0) {
      throw new WorkflowInputError(
        `Task ${task.id} is missing an observable completion bullet`,
      );
    }

    return {
      id: task.id,
      title: task.title,
      passes: task.passes,
      optional: task.optional,
      requirements: splitCsv(requirementsValue),
      boundary,
      dependsOn:
        dependsValue.toLowerCase() === "none" ? [] : splitCsv(dependsValue),
      details,
      blocked: /_Blocked:\s*[^_]+_/i.test(body),
    };
  });
}

export function validateTasks(tasks) {
  if (tasks.length === 0) {
    throw new WorkflowInputError("Task plan has no executable tasks");
  }

  const byId = new Map();
  for (const task of tasks) {
    if (byId.has(task.id)) {
      throw new WorkflowInputError(`Duplicate task ID ${task.id}`);
    }
    if (task.blocked) {
      throw new WorkflowInputError(`Task ${task.id} is blocked`);
    }
    byId.set(task.id, task);
  }

  const normalized = tasks.map((task) => {
    const expanded = [];
    for (const dependency of task.dependsOn) {
      if (byId.has(dependency)) {
        expanded.push(dependency);
        continue;
      }

      const groupLeaves = tasks
        .filter((candidate) => candidate.id.startsWith(`${dependency}.`))
        .map((candidate) => candidate.id);
      if (groupLeaves.length === 0) {
        throw new WorkflowInputError(
          `Task ${task.id} has unknown dependency ${dependency}`,
        );
      }
      expanded.push(...groupLeaves);
    }

    return { ...task, dependsOn: [...new Set(expanded)] };
  });

  const normalizedById = new Map(normalized.map((task) => [task.id, task]));
  const visiting = new Set();
  const visited = new Set();

  function visit(taskId, path) {
    if (visiting.has(taskId)) {
      throw new WorkflowInputError(
        `Task dependency cycle: ${[...path, taskId].join(" -> ")}`,
      );
    }
    if (visited.has(taskId)) {
      return;
    }

    visiting.add(taskId);
    const task = normalizedById.get(taskId);
    for (const dependency of task.dependsOn) {
      visit(dependency, [...path, taskId]);
    }
    visiting.delete(taskId);
    visited.add(taskId);
  }

  for (const task of normalized) {
    visit(task.id, []);
    if (task.passes) {
      for (const dependency of task.dependsOn) {
        if (!normalizedById.get(dependency).passes) {
          throw new WorkflowInputError(
            `Task ${task.id} is completed but prerequisite ${dependency} is incomplete`,
          );
        }
      }
    }
  }

  return normalized;
}

export function compileTracker(feature, sourcePath, tasks) {
  const featureLabel = `feature:${feature}`;
  const stories = tasks.map((task) => {
    const major = Number.parseInt(task.id.split(".")[0], 10);
    return {
      id: task.id,
      title: task.title,
      description: [
        `Feature: ${feature}`,
        `Spec: .kiro/specs/${feature}`,
        `Task source: ${sourcePath}`,
        `Boundary: ${task.boundary}`,
      ].join("\n"),
      acceptanceCriteria: [
        ...task.details,
        `Requirements covered: ${task.requirements.join(", ")}`,
        `Boundary respected: ${task.boundary}`,
        "cc-sdd task-local review returns APPROVED",
        "Fresh verification passes",
      ],
      priority: (task.optional ? 100 : 0) + major,
      passes: task.passes,
      labels: ["cc-sdd", featureLabel, ...(task.optional ? ["optional"] : [])],
      dependsOn: task.dependsOn,
      notes: `Derived from ${sourcePath}; edit tasks.md, not this projection.`,
    };
  });

  stories.push({
    id: "VALIDATE",
    title: `Validate ${feature} integration`,
    description: [
      `Feature: ${feature}`,
      `Spec: .kiro/specs/${feature}`,
      `Task source: ${sourcePath}`,
    ].join("\n"),
    acceptanceCriteria: [
      "Full repository quality gates pass",
      "Requirements coverage is complete",
      "Design and boundary validation return GO",
    ],
    priority: 999,
    passes: false,
    labels: ["cc-sdd", featureLabel, "validation"],
    dependsOn: tasks.map((task) => task.id),
    notes: "Run cc-sdd feature-level validation; only GO may complete this item.",
  });

  return {
    name: feature,
    description: `Derived from ${sourcePath}; do not edit by hand.`,
    userStories: stories,
  };
}

function serializeTracker(tracker) {
  return `${JSON.stringify(tracker, null, 2)}\n`;
}

function projectPath(root, feature) {
  return path.join(root, ".ralph-tui", "generated", `${feature}.json`);
}

async function readTasksFile(tasksPath) {
  try {
    return await readFile(tasksPath, "utf8");
  } catch (error) {
    throw new WorkflowInputError(
      `Cannot read ${tasksPath}: ${error.message}`,
    );
  }
}

async function requireApprovedTasks(specPath) {
  let raw;
  try {
    raw = await readFile(specPath, "utf8");
  } catch (error) {
    throw new WorkflowInputError(`Cannot read ${specPath}: ${error.message}`);
  }

  let spec;
  try {
    spec = JSON.parse(raw);
  } catch (error) {
    throw new WorkflowInputError(`Cannot parse ${specPath}: ${error.message}`);
  }

  if (spec?.approvals?.tasks?.approved !== true) {
    throw new WorkflowInputError(
      `cc-sdd tasks are not approved in ${specPath}`,
    );
  }
}

export async function runCli(args, root = process.cwd()) {
  const [feature, ...options] = args;
  if (!feature) {
    throw new WorkflowInputError(
      "Usage: cc-sdd-to-ralph.mjs <feature> [--check|--stdout]",
    );
  }
  if (!/^[a-z0-9][a-z0-9._-]*$/.test(feature)) {
    throw new WorkflowInputError(`Invalid feature name: ${feature}`);
  }

  const allowedOptions = new Set(["--check", "--stdout"]);
  const unknownOption = options.find((option) => !allowedOptions.has(option));
  if (unknownOption) {
    throw new WorkflowInputError(`Unknown option: ${unknownOption}`);
  }
  if (options.includes("--check") && options.includes("--stdout")) {
    throw new WorkflowInputError("Use only one of --check or --stdout");
  }

  const tasksPath = path.join(root, ".kiro", "specs", feature, "tasks.md");
  const specPath = path.join(root, ".kiro", "specs", feature, "spec.json");
  const sourcePath = path.relative(root, tasksPath).split(path.sep).join("/");
  const markdown = await readTasksFile(tasksPath);
  await requireApprovedTasks(specPath);
  const tasks = validateTasks(parseTasks(markdown));
  const serialized = serializeTracker(
    compileTracker(feature, sourcePath, tasks),
  );

  if (options.includes("--stdout")) {
    process.stdout.write(serialized);
    return;
  }

  const outputPath = projectPath(root, feature);
  if (options.includes("--check")) {
    let existing;
    try {
      existing = await readFile(outputPath, "utf8");
    } catch {
      throw new WorkflowInputError(
        `Ralph projection is stale: ${outputPath} does not exist`,
      );
    }
    if (existing !== serialized) {
      throw new WorkflowInputError(
        `Ralph projection is stale: regenerate ${outputPath}`,
      );
    }
    return;
  }

  await mkdir(path.dirname(outputPath), { recursive: true });
  const temporaryPath = `${outputPath}.${process.pid}.tmp`;
  await writeFile(temporaryPath, serialized, "utf8");
  await rename(temporaryPath, outputPath);
  process.stdout.write(`${path.relative(root, outputPath)}\n`);
}

const isMain =
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href;

if (isMain) {
  runCli(process.argv.slice(2)).catch((error) => {
    const message =
      error instanceof Error ? error.message : "Unknown workflow compiler error";
    process.stderr.write(`cc-sdd-to-ralph: ${message}\n`);
    process.exitCode = 1;
  });
}
