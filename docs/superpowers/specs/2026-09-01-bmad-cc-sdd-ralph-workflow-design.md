# BMAD、cc-sdd 与 Ralph TUI 单主干工作流设计

**日期：** 2026-09-01  
**状态：** 已批准  
**适用范围：** `ys-data-agent` 个人项目开发工作流

## 1. 目标

建立一套只有一个开发事实主干、可以由 Ralph TUI 长时间驱动、同时保留人工产品与设计审批的 Agentic SDLC：

- BMAD 只生成和维护 Product Brief / PRD；
- cc-sdd 负责需求细化、设计、任务、TDD、Review 与验证协议；
- Ralph TUI 只选择下一个 cc-sdd 任务并启动 Code Agent；
- `tasks.md` 是任务唯一事实源，Ralph JSON 是可删除、可重建的调度投影。

## 2. 职责边界

### BMAD

负责产品层的 Why / What：用户、问题、目标、FR/NFR、范围、非目标和成功信号。PRD 获得人工批准后，BMAD 不再生成 Architecture、Epics、Stories、Sprint 或实现产物。

### cc-sdd

负责开发主干：

1. `requirements.md` 将 PRD 细化为可测试的 feature contract；
2. `design.md` 固定边界、接口、依赖与文件责任；
3. `tasks.md` 将设计拆为一轮可完成的执行单元；
4. 每个任务执行 TDD、独立 Review、有限修复与 fresh verification；
5. 所有任务完成后运行 feature-level integration validation。

### Ralph TUI

负责唯一的外层任务循环：从派生 JSON 中选择 `passes=false` 且依赖已完成的任务，启动 Codex，识别完成信号，然后继续下一任务。Ralph 不得生成或修改产品需求，也不得把 `<promise>COMPLETE</promise>` 当成质量证据。

## 3. 权威产物

| 关注点 | 权威产物 |
|---|---|
| 产品意图 | `_bmad-output/.../prd.md` |
| 项目级技术约束 | `.kiro/steering/*.md` |
| Feature 行为契约 | `.kiro/specs/<feature>/requirements.md` |
| 技术设计与边界 | `.kiro/specs/<feature>/design.md` |
| 任务及完成状态 | `.kiro/specs/<feature>/tasks.md` |
| 可执行事实 | Git diff、测试与运行结果 |
| 调度投影 | `.ralph-tui/generated/<feature>.json` |
| 临时跨轮记忆 | `.ralph-tui/progress.md` |

发生冲突时必须停止并修正上游权威产物，不能让 Ralph JSON 或 progress 反向覆盖 spec。

## 4. 阶段和 Gate

```text
BMAD Brief（可选） → BMAD PRD → 人工批准
                                  ↓
cc-sdd requirements → 人工批准
                                  ↓
cc-sdd design       → 人工批准
                                  ↓
cc-sdd tasks        → 人工批准
                                  ↓
compile tasks.md → Ralph JSON
                                  ↓
Ralph：逐任务调用 run-cc-sdd-task
                                  ↓
cc-sdd feature validation
                                  ↓
人工按 PRD success signal 验收 → Merge / Release
```

仅保留五个人工 Gate：PRD、requirements、design、tasks 和最终产品验收。

## 5. 单任务执行契约

Ralph 每轮只传入 `<feature>` 与 `<task-id>`。项目级 `run-cc-sdd-task` skill 必须：

1. 确认 spec 文件存在且 tasks 已批准；
2. 确认 Ralph 投影在本轮开始时与 `tasks.md` 一致；
3. 只运行 cc-sdd manual-mode 的选定任务；
4. 行为变更执行 RED → GREEN → REFACTOR；
5. 使用 fresh reviewer（环境支持时）检查实际 diff、spec、边界与测试；
6. Review 通过后用 fresh evidence 验证，再更新 `tasks.md`；
7. 只有上述条件全部满足才能输出 `<promise>COMPLETE</promise>`；
8. 对需求冲突、越界或无法验证返回 blocker，不伪装完成。

特殊任务 `VALIDATE` 不对应 `tasks.md` leaf task；它运行 cc-sdd feature-level validation，只有 `GO` 才完成。

## 6. 调度投影

转换器把 cc-sdd leaf task 映射为 Ralph JSON：

| cc-sdd | Ralph |
|---|---|
| 任务编号 | `id` |
| 任务文本 | `title` |
| Requirements / Boundary / 来源路径 | `description` 和 `notes` |
| 可观察完成条件 | `acceptanceCriteria` |
| `_Depends:_` | `dependsOn` |
| Major task 顺序 | `priority` |
| `[ ]` / `[x]` | `passes` |

转换必须是确定性的，并拒绝缺少 Requirement、Boundary、未知依赖、循环依赖、已完成任务依赖未完成任务或存在 `_Blocked:_` 的计划。最后追加依赖全部 leaf task 的 `VALIDATE` 调度任务。

## 7. 质量闭环

### Task-local Gate

- 新行为有真实 RED 证据；
- focused tests 通过；
- 格式、lint、类型或编译检查通过；
- independent reviewer 返回明确 `APPROVED`；
- diff 未越过 `_Boundary:_`；
- 无无关重构、残余 TODO 或敏感信息。

### Feature Gate

- `cargo fmt --all --check`；
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`；
- `cargo test --workspace`；
- 运行仓库现有 `scripts/v0.2-release-gate.sh` 所定义的集成、smoke 与 eval 检查；
- cc-sdd validation 返回 `GO`。

### Product Gate

人工依据 BMAD PRD 的成功信号检查真实用户路径、无法自动验证的行为、剩余风险和最终 diff，然后决定 Merge / Release。

## 8. 简化规则

- 不使用 BMAD implementation、stories 或 Sprint 产物；
- 不使用 Ralph `create-prd`；
- 不维护第二套任务账本；
- 不为一次会话可完成的小修改强制创建 PRD、完整 spec 或 Ralph run；
- 不默认并行修改同一 worktree；
- Ralph `autoCommit=false`，避免 stage 用户已有修改；
- `.ralph-tui/generated/`、progress 和 iteration logs 不提交 Git。

## 9. 变更回流

- 用户、目标、范围或产品约束变化：更新 BMAD PRD，再重做受影响的 cc-sdd 阶段；
- 技术实现或边界变化：更新 cc-sdd design/tasks，不改 PRD；
- 当前任务内的实现缺陷：在同一任务有限修复；
- 无关问题：记录到 deferred backlog，不在当前任务顺手处理。
