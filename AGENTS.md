# Minimal Agentic SDLC

This repository keeps one project-level product document and routes each Change by impact.

```text
docs/PRD.md
    ↓
Change
  ├── small change → direct Code Agent → code + test + review + fresh verification
  └── Feature      → cc-sdd requirements.md → design.md → tasks.md
                                      ↓
                                  Ralph TUI
                                      ↓
                              one-task Code Agent Loop
```

## Source-of-Truth Boundaries

- **BMAD** owns `docs/PRD.md`: the single project-wide product, stable architecture, and evolution design for ys-data-agent. It contains durable project reasoning and boundaries, but never a Feature's detailed requirements, design, or tasks.
- **cc-sdd** is used only for Features. Each Feature keeps exactly three human-maintained engineering documents under `.kiro/specs/<feature>/`: `requirements.md`, `design.md`, and `tasks.md`.
- `spec.json` stores machine-readable phase and human-approval state; it is not a fourth planning document.
- **Ralph TUI** executes approved Feature tasks only. `.ralph-tui/generated/*.json`, iteration logs, and progress files are disposable runtime state.
- `.kiro/specs/<feature>/tasks.md` is the sole authoritative Feature task list and completion state.

Project-level product, stable architecture, or evolution conflicts return to `docs/PRD.md`. Feature conflicts return to the owning cc-sdd document. Ralph must never redefine either.

## Change Routing

Classify before creating workflow artifacts.

### Small change

Use a direct, bounded Code Agent when all are true:

- project scope, stable architecture, and existing approved Feature behavior remain unchanged;
- no new user-visible capability is introduced;
- no public contract or persistent-state shape changes;
- the responsibility boundary is clear;
- one Agent session can implement and verify it safely.

Do not invoke BMAD, create cc-sdd documents, or start Ralph. Still require scoped implementation, risk-proportionate tests, review of the actual diff, and fresh verification evidence.

### Feature

Use cc-sdd when the Change adds or materially changes user behavior, a public contract, persistent state, external integration, cross-module responsibility, or requires multiple independently verifiable tasks. If it changes project scope, stable architecture, or evolution order, update and approve `docs/PRD.md` first; keep the Feature's detailed requirements in cc-sdd.

Required Feature flow:

1. `$kiro-spec-init "<feature description>; project design: docs/PRD.md"`
2. `$kiro-spec-requirements <feature>` → human approval
3. `$kiro-spec-design <feature>` → human approval
4. `$kiro-spec-tasks <feature>` → human approval
5. `rtk ./scripts/ralph-cc-sdd.sh <feature>`
6. Ralph dispatches exactly one task per iteration through `$run-cc-sdd-task <feature> <task-id>`.
7. The reserved `VALIDATE` item runs full Feature validation; a human accepts the result against `docs/PRD.md` and the approved Feature spec before merge or release.

## Allowed Project Skills

- `$bmad-prd` — create, update, or validate `docs/PRD.md`
- `$kiro-spec-init`
- `$kiro-spec-requirements`
- `$kiro-spec-design`
- `$kiro-spec-tasks`
- `$run-cc-sdd-task` — Ralph-only implementation bridge

Do not introduce BMAD architecture, epic/story, sprint, task, implementation, or review workflows. Do not add a second cc-sdd discovery/status/implementation layer or use Ralph's PRD/task-authoring features.

## Ralph Task Execution Constraints

When and only when Ralph dispatches `$run-cc-sdd-task`, the Agent must read completely
`.ysda/agents/rust-engineer.md` and
`.ysda/agents/code-change-pr-workflow.md` before implementation.
Missing or unreadable policy files leave the task blocked. Their commit, push, and PR
authority is limited to the selected approved Feature task on its Feature
branch; they never authorize force-push, automatic merge, or unrelated changes.

## Project Memory and Language

- Load `.kiro/steering/` as stable project policy. Use local `AGENTS.md` only for folder-specific domain or test contracts.
- Respond in Simplified Chinese. Write cc-sdd documents in `spec.json.language`.
- Preserve existing user changes and remain inside the selected Change/task boundary.
- Never edit `.ralph-tui/generated/*.json` by hand.
