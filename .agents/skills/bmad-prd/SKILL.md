---
name: bmad-prd
description: Create, update, or validate docs/PRD.md, the single project-wide product, architecture, and evolution design for ys-data-agent. Use only for project-level changes; ordinary Feature requirements go directly to cc-sdd.
---

# BMAD Project Design PRD

BMAD maintains the project's durable design thinking in one place: product intent, stable architecture, current release boundaries, architecture invariants, and future evolution. It must not absorb Feature-specific Requirements/Design/Tasks, sprint state, implementation tasks, or code.

## Artifact Contract

- The only persistent artifact produced by this skill is `docs/PRD.md`.
- `docs/PRD.md` covers the whole `ys-data-agent` project: product reasoning, stable system design, release boundaries, architecture invariants, and evolution roadmap. Do not place a Feature requirements baseline in it or create one PRD per Change or Feature.
- If `docs/PRD.md` exists, update it in place. Do not create a second project-design document, run directory, or dated copy.
- Use `assets/prd-template.md` as the starting structure, then remove sections that do not earn their place.
- Write in the user's requested language. Keep the document coherent and navigable; preserve architecture detail that has earned project-wide authority instead of shortening it merely for brevity.

## Activation

Apply this routing gate before choosing a mode:

- **普通 Feature 默认跳过 BMAD：** Feature 的 Requirements 直接进入 cc-sdd；不要为了新增一个 Feature 而更新 `docs/PRD.md`。
- **只有项目级事实变化才使用 BMAD：** 项目方向、稳定架构、发布边界或演进顺序至少有一项发生变化时，才创建或更新 `docs/PRD.md`。
- Feature 的用户行为、FR/NFR、验收标准、接口、数据结构和任务仍属于 `.kiro/specs/<feature>/`。即使需要先更新项目总纲，也不得把这些详细内容复制进 `docs/PRD.md`。

If an ordinary Feature fits the approved project design without changing those four project-level facts, stop this skill and route directly to cc-sdd. If a real conflict exists, update only the affected project-level sections before returning to the Feature specification.

Infer one intent from the request:

- **Create** — no PRD exists yet.
- **Update** — revise an existing PRD in place.
- **Validate** — inspect the PRD against the quality gate below; apply fixes only when the user asks for them.

Before writing, gather only decisions that materially change product intent, durable architecture, current release boundaries, or the roadmap. Read supplied sources and reconcile their lasting conclusions into `docs/PRD.md`; do not preserve parallel sources after an explicit merge.

## Create or Update

1. Read the existing `docs/PRD.md` completely before changing it.
2. Preserve the project kernel:
   - target user and current problem;
   - desired outcome and why it matters;
   - MVP scope and explicit non-goals;
   - user-visible functional requirements;
   - measurable success criteria;
   - constraints, risks, assumptions, and unresolved product decisions.
3. Preserve stable system design when it matters across Features: architecture principles, core domain model, runtime/control boundaries, security and authority rules, repository dependency direction, release boundaries, evolution sequence, and explicit invariants.
4. Keep Feature-local engineering detail in cc-sdd: exact interfaces, schemas, file edits, dependency selections, migration mechanics, and implementation sequencing belong in that Feature's `requirements.md`, `design.md`, and `tasks.md` unless they become a durable project-wide decision.
5. Use stable identifiers for project-level requirements and explicit numbering for roadmap phases and architecture invariants.
6. Mark inferred facts inline as `[ASSUMPTION: ...]`. Resolve blocking assumptions before declaring the document ready.
7. Preserve approved project decisions that the Change does not explicitly supersede; state supersession scope when decisions conflict.
8. Write or update only `docs/PRD.md`.
9. Ask the user to approve it as the project Source of Truth and update its status/date in the same file.

## Quality Gate

A project design is ready when all of the following are true:

- the problem, target user, and intended outcome are explicit;
- MVP scope and non-goals prevent adjacent feature expansion;
- current release scope is distinct from long-term architecture and roadmap;
- stable architecture boundaries and invariants are internally consistent;
- current release claims and project-level requirements are testable and traceable;
- success metrics have a measurable signal;
- important edge cases, constraints, and risks are represented;
- the roadmap is an ordered set of independently deliverable vertical slices, not speculative implementation scaffolding;
- blocking open questions are resolved;
- terminology is internally consistent;
- duplicated or superseded project-design documents have been reconciled.

For **Validate**, report `READY` or `NEEDS_REVISION` with concrete findings. Do not create a separate validation report.

## Change Routing

Do not send every Change through cc-sdd. Classify it after checking whether it alters `docs/PRD.md`:

- **Small change:** product scope unchanged, no new user-visible capability, no public contract or persistent-state change, one responsibility boundary, and safely completable in one bounded Agent session. Route directly to a Code Agent with scoped tests, review, and fresh verification. BMAD, cc-sdd documents, and `$kiro-impl` are unnecessary.
- **Feature:** new or materially changed user behavior, public contract, persistent state, cross-boundary integration, or multiple independently verifiable tasks. Route through cc-sdd, then execute approved tasks with `$kiro-impl`. Do not invoke BMAD unless the Feature also changes one of the four project-level facts above.

For a new Feature that does not require a project-level update, provide the next command immediately:

```text
$kiro-spec-init "<feature description>; project design: docs/PRD.md"
```

For an existing Feature, continue from its current cc-sdd phase instead of running initialization again. The approved `docs/PRD.md` remains the project Source of Truth. cc-sdd may refine Feature engineering detail but must send genuine project-level product or architecture conflicts back to it instead of silently redefining them.
