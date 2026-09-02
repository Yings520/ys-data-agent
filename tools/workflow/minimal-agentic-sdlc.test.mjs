import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const projectRoot = fileURLToPath(new URL("../../", import.meta.url));
const skillsRoot = path.join(projectRoot, ".agents/skills");

async function projectSkillNames() {
  const entries = await readdir(skillsRoot, { withFileTypes: true });
  const names = [];

  for (const entry of entries) {
    if (!entry.isDirectory()) {
      continue;
    }
    try {
      await readFile(path.join(skillsRoot, entry.name, "SKILL.md"), "utf8");
      names.push(entry.name);
    } catch (error) {
      if (error?.code !== "ENOENT") {
        throw error;
      }
    }
  }

  return names.sort();
}

async function readTree(root) {
  const chunks = [];
  const entries = await readdir(root, { withFileTypes: true });

  for (const entry of entries) {
    const entryPath = path.join(root, entry.name);
    if (entry.isDirectory()) {
      chunks.push(await readTree(entryPath));
    } else {
      chunks.push(await readFile(entryPath, "utf8"));
    }
  }

  return chunks.join("\n");
}

test("project exposes only the six workflow entry skills", async () => {
  assert.deepEqual(await projectSkillNames(), [
    "bmad-prd",
    "kiro-impl",
    "kiro-spec-design",
    "kiro-spec-init",
    "kiro-spec-requirements",
    "kiro-spec-tasks",
  ]);

  const implementationSkill = await readFile(
    path.join(skillsRoot, "kiro-impl/SKILL.md"),
    "utf8",
  );
  for (const reference of [
    "implementation.md",
    "review.md",
    "verify-completion.md",
    "validation.md",
  ]) {
    await readFile(
      path.join(skillsRoot, "kiro-impl/references", reference),
      "utf8",
    );
    assert.match(implementationSkill, new RegExp(`references/${reference}`));
  }
  for (const contract of [
    /\$kiro-impl <feature>/,
    /cc-sdd-task-state\.mjs <feature> --next/,
    /one task commit/i,
    /repeat|continue/i,
    /VALIDATE/,
  ]) {
    assert.match(implementationSkill, contract);
  }
});

test("BMAD produces one PRD artifact without a parallel document system", async () => {
  const bmadRoot = path.join(skillsRoot, "bmad-prd");
  const bmad = await readTree(bmadRoot);

  for (const forbidden of [
    ".memlog.md",
    "addendum.md",
    "review-",
    "memlog.py",
    "bmad-product-brief",
    "bmad-party-mode",
    "bmad-architecture",
    "bmad-help",
  ]) {
    assert.doesNotMatch(bmad, new RegExp(forbidden.replaceAll(".", "\\.")));
  }

  assert.doesNotMatch(bmad, /planning-artifacts\/prds/);
  assert.match(bmad, /`docs\/PRD\.md`/);
  const projectDesign = await readFile(
    path.join(projectRoot, "docs/PRD.md"),
    "utf8",
  );
  await assert.rejects(readFile(path.join(projectRoot, "PRD.md"), "utf8"), {
    code: "ENOENT",
  });
  for (const section of ["总体架构", "后续演进顺序", "Change"]) {
    assert.match(projectDesign, new RegExp(section));
  }
  assert.doesNotMatch(projectDesign, /统一 LLM Provider 管理/);
  assert.doesNotMatch(projectDesign, /^## 32\./m);
});

test("ordinary Features skip BMAD while project-boundary changes update the PRD", async () => {
  const bmad = await readFile(
    path.join(skillsRoot, "bmad-prd/SKILL.md"),
    "utf8",
  );
  const guide = await readFile(
    path.join(projectRoot, "docs/BMAD-CC-SDD-RALPH-USAGE.md"),
    "utf8",
  );

  for (const document of [bmad, guide]) {
    assert.match(document, /普通 Feature[^\n]*(?:跳过|不要调用).*BMAD/i);
    assert.match(document, /项目方向[^\n]*稳定架构[^\n]*发布边界[^\n]*演进顺序/);
    assert.match(document, /Feature[^\n]*(?:Requirements|requirements\.md)[^\n]*(?:cc-sdd|\.kiro\/specs)/i);
  }

  assert.match(guide, /provider-management[^]*§26\.5/);
  assert.match(guide, /只修改项目级 Provider 架构、v0\.2 发布边界和演进计划/);
});

test("a new Provider capability starts as a cc-sdd Feature input", async () => {
  const featureRoot = path.join(
    projectRoot,
    ".kiro/specs/provider-management",
  );
  const control = JSON.parse(
    await readFile(path.join(featureRoot, "spec.json"), "utf8"),
  );
  const requirements = await readFile(
    path.join(featureRoot, "requirements.md"),
    "utf8",
  );

  assert.equal(control.feature_name, "provider-management");
  assert.equal(control.phase, "initialized");
  assert.equal(control.approvals.requirements.generated, false);
  assert.equal(control.approvals.requirements.approved, false);
  assert.equal(control.approvals.design.generated, false);
  assert.equal(control.approvals.design.approved, false);
  assert.equal(control.approvals.tasks.generated, false);
  assert.equal(control.approvals.tasks.approved, false);
  assert.equal(control.ready_for_implementation, false);
  assert.match(requirements, /统一 LLM Provider 接入与管理/);
  assert.match(requirements, /FR-18/);
  assert.match(requirements, /NFR-17/);
  assert.match(requirements, /AC-13/);
  assert.match(requirements, /\$kiro-spec-requirements provider-management/);
  await Promise.all(
    ["design.md", "tasks.md"].map((file) =>
      assert.rejects(readFile(path.join(featureRoot, file), "utf8"), {
        code: "ENOENT",
      }),
    ),
  );
});

test("cc-sdd persists only requirements, design, and tasks as feature documents", async () => {
  const ccSdd = await Promise.all(
    [
      "kiro-spec-init",
      "kiro-spec-requirements",
      "kiro-spec-design",
      "kiro-spec-tasks",
    ].map((name) => readTree(path.join(skillsRoot, name))),
  );
  const templates = await readTree(
    path.join(projectRoot, ".kiro/settings/templates/specs"),
  );
  const workflow = `${ccSdd.join("\n")}\n${templates}`;

  for (const forbidden of [
    "brief.md",
    "research.md",
    "$kiro-discovery",
    "$kiro-impl",
    "$kiro-spec-quick",
    "$kiro-spec-batch",
    "$kiro-validate-gap",
    "$kiro-validate-design",
  ]) {
    assert.doesNotMatch(
      workflow,
      new RegExp(forbidden.replaceAll("$", "\\$").replaceAll(".", "\\.")),
    );
  }

  for (const artifact of ["requirements.md", "design.md", "tasks.md"] ) {
    assert.match(workflow, new RegExp(artifact.replaceAll(".", "\\.")));
  }
});

test("repository guidance documents the minimal source-of-truth chain", async () => {
  const guidance = await Promise.all(
    ["AGENTS.md", "README.md", "docs/BMAD-CC-SDD-RALPH-USAGE.md"].map(
      (file) => readFile(path.join(projectRoot, file), "utf8"),
    ),
  );
  guidance.push(await readTree(path.join(projectRoot, ".kiro/steering")));
  guidance.push(
    await readFile(path.join(projectRoot, "docs/PRD.md"), "utf8"),
  );
  const text = guidance.join("\n");

  for (const document of guidance) {
    assert.match(document, /docs\/PRD\.md/);
  }

  for (const forbidden of [
    "$bmad-product-brief",
    "brief.md",
    "research.md",
    "$kiro-discovery",
    "$kiro-impl",
    "$kiro-spec-quick",
    "$kiro-spec-batch",
    "$kiro-spec-status",
    "$kiro-validate-gap",
    "$kiro-validate-design",
    "$kiro-validate-impl",
    "planning-artifacts/prds",
    "root PRD.md",
    "root `PRD.md`",
    "根目录 `PRD.md`",
  ]) {
    assert.doesNotMatch(
      text,
      new RegExp(forbidden.replaceAll("$", "\\$").replaceAll(".", "\\.")),
    );
  }

  for (const required of [
    "$bmad-prd",
    "$kiro-spec-init",
    "$kiro-spec-requirements",
    "$kiro-spec-design",
    "$kiro-spec-tasks",
    "$run-cc-sdd-task",
    "tasks.md",
    ".ralph-tui/generated/",
    "小改动",
    "直接 Code Agent",
    "Feature",
    "docs/PRD.md",
  ]) {
    assert.match(
      text,
      new RegExp(required.replaceAll("$", "\\$").replaceAll(".", "\\.")),
    );
  }
});
