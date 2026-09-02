import { readFile } from "node:fs/promises";
import path from "node:path";
import { pathToFileURL } from "node:url";

const FEATURE_NAME = /^[a-z0-9][a-z0-9._-]*$/;
const TASK_LINE = /^- \[([ xX])\](\*)?\s+(\d+(?:\.\d+)*)\.?\s+(.+?)\s*$/;

export class WorkflowInputError extends Error {
  constructor(message) {
    super(message);
    this.name = "WorkflowInputError";
  }
}

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

  function visit(taskId, taskPath) {
    if (visiting.has(taskId)) {
      throw new WorkflowInputError(
        `Task dependency cycle: ${[...taskPath, taskId].join(" -> ")}`,
      );
    }
    if (visited.has(taskId)) {
      return;
    }

    visiting.add(taskId);
    const task = normalizedById.get(taskId);
    for (const dependency of task.dependsOn) {
      visit(dependency, [...taskPath, taskId]);
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

export function selectNextTask(tasks) {
  const completed = new Set(
    tasks.filter((task) => task.passes).map((task) => task.id),
  );
  const next = tasks.find(
    (task) =>
      !task.passes &&
      task.dependsOn.every((dependency) => completed.has(dependency)),
  );
  if (next) {
    return next;
  }
  if (tasks.every((task) => task.passes)) {
    return { id: "VALIDATE" };
  }
  throw new WorkflowInputError("No dependency-ready task remains");
}

async function readApprovedTaskState(feature, root) {
  const featureDirectory = path.join(root, ".kiro", "specs", feature);
  let spec;
  let markdown;
  try {
    spec = JSON.parse(
      await readFile(path.join(featureDirectory, "spec.json"), "utf8"),
    );
    markdown = await readFile(path.join(featureDirectory, "tasks.md"), "utf8");
  } catch {
    throw new WorkflowInputError("Approved cc-sdd task state is unavailable");
  }

  for (const artifact of ["requirements", "design", "tasks"]) {
    if (spec?.approvals?.[artifact]?.approved !== true) {
      throw new WorkflowInputError(`cc-sdd ${artifact} is not approved`);
    }
  }
  return validateTasks(parseTasks(markdown));
}

export async function runCli(args, root = process.cwd()) {
  const [feature, option] = args;
  if (!FEATURE_NAME.test(feature ?? "")) {
    throw new WorkflowInputError("Invalid feature name");
  }
  if (!new Set(["--check", "--next"]).has(option) || args.length !== 2) {
    throw new WorkflowInputError(
      "Usage: cc-sdd-task-state.mjs <feature> <--check|--next>",
    );
  }

  const tasks = await readApprovedTaskState(feature, root);
  if (option === "--next") {
    process.stdout.write(`${JSON.stringify(selectNextTask(tasks))}\n`);
  }
}

const isMain =
  process.argv[1] &&
  import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href;

if (isMain) {
  runCli(process.argv.slice(2)).catch(() => {
    process.stderr.write("cc-sdd-task-state: cc-sdd state denied\n");
    process.exitCode = 1;
  });
}
