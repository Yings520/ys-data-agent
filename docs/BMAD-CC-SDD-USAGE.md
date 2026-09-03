# BMAD + cc-sdd + `kiro-impl` 使用手册

本文说明本仓库唯一的 Agentic SDLC。项目级事实、Feature 工程规格与实现状态各有一个明确来源：

```text
docs/PRD.md
    ↓
Change
  ├── 小改动 → 有界 Code Agent → Test → Review → Fresh Verification
  └── Feature  → requirements.md → design.md → tasks.md
                                             ↓
                                    $kiro-impl <feature>
                                             ↓
                         TDD → task commit → push → Draft PR
```

## 1. 三类责任

| 组件 | 负责 | 不负责 |
|---|---|---|
| BMAD `$bmad-prd` | `docs/PRD.md` 中的项目方向、稳定架构、发布边界和演进顺序 | 普通 Feature 的详细 Requirements、Design、Tasks |
| cc-sdd | `.kiro/specs/<feature>/` 下批准的 `requirements.md`、`design.md`、`tasks.md` | 项目级产品和稳定架构重定义 |
| `$kiro-impl` | 直接执行批准的 `tasks.md`，逐项测试、审查、验证、提交和推送 | 重新拆任务、改变依赖、自动合并 PR |

`spec.json` 只保存阶段与人工审批状态，不是第四份工程文档。`tasks.md` 是唯一任务图和完成状态，不生成第二份调度状态。

## 2. 先判断 Change 类型

### 小改动

同时满足以下条件时直接交给一个有界 Code Agent：

- 不改变项目范围、稳定架构和已批准 Feature 行为；
- 不增加用户可见能力；
- 不改变公共契约或持久状态；
- 责任边界清晰，可在一个 Agent 会话内安全完成。

小改动不调用 BMAD、不创建 cc-sdd 文档，也不调用 `$kiro-impl`，但仍必须有范围匹配的测试、实际 Diff Review 和新鲜验证。

### 普通 Feature

普通 Feature 跳过 BMAD，直接使用 cc-sdd。Feature 的 Requirements、Design 和 Tasks 全部位于 `.kiro/specs/<feature>/`，不得塞入 `docs/PRD.md`。

### 项目级变化

只有 Change 修改项目方向、稳定架构、发布边界或演进顺序时，才先调用 `$bmad-prd`。更新并批准 `docs/PRD.md` 后，再为具体 Feature 进入 cc-sdd。

例如 `provider-management` 的项目级上游变更只修改项目级 Provider 架构、v0.2 发布边界和演进计划；具体 Provider 管理行为仍由 cc-sdd Requirements 定义，并追溯 `docs/PRD.md` §26.5 等批准章节。

## 3. Feature 规格流程

在 Codex 对话中依次调用：

```text
$kiro-spec-init "<feature description>; project design: docs/PRD.md"
$kiro-spec-requirements <feature>
$kiro-spec-design <feature>
$kiro-spec-tasks <feature>
```

每个阶段都必须人工阅读并批准。`spec.json` 中必须满足：

```json
{
  "approvals": {
    "requirements": { "approved": true },
    "design": { "approved": true },
    "tasks": { "approved": true }
  }
}
```

如果 task graph 暴露真实 Requirements 或 Design 缺口，返回相应阶段修复，不得用含糊任务掩盖。

## 4. 自动实现

批准 `tasks.md` 后，在 Codex 对话中调用：

```text
$kiro-impl <feature>
```

例如：

```text
$kiro-impl provider-management
```

该 Skill 会直接读取 `tasks.md`：

1. 校验 Requirements、Design、Tasks 均已批准；
2. 校验依赖图、阻塞状态和 `feat/<feature>` 分支；
3. 选择第一个未完成且 `_Depends:_` 已满足的 leaf task；
4. 读取任务对应的 Requirements、Design、Boundary 和完成条件；
5. 执行 RED → GREEN → REFACTOR；
6. 完成 task-local review 与 fresh verification；
7. 只勾选当前 task；
8. 创建一个 task commit，普通 push，并创建或复用同一个 Draft Feature PR；
9. 重新读取 `tasks.md`，继续下一项；
10. 全部 task 完成后执行 Feature validation，并仅在 `GO` 时把 PR 标记为 Ready。

`(P)` 只表示任务边界可并行分析，不允许同一工作树并发修改。默认执行始终串行。

## 5. TDD 和完成门禁

行为变化必须留下真实 RED 证据：测试先因缺少目标行为而失败，而不是因为语法、夹具或环境错误失败。最小实现使其 GREEN 后，才能重构并运行回归。

单个 task 只有同时满足以下条件才能勾选：

- 任务相关测试和静态检查通过；
- Diff 没有越过 `_Boundary:_`；
- task-local review 返回 `APPROVED`；
- fresh completion verification 返回 `VERIFIED`；
- 发布 helper 确认只变化一个 checkbox、只暂存声明路径并成功 push。

Agent 描述和已勾选 checkbox 本身都不是完成证据。

## 6. Commit、Push 和 PR

- 每个成功 task 一个 commit；
- 每个 task commit 后一次普通 push；
- 所有 task 复用 `feat/<feature>` 和一个 Draft PR；
- commit 带 `CC-SDD-Feature` 与 `CC-SDD-Task` trailers；
- Feature validation 可把 Draft PR 标记为 Ready；
- 永不自动 merge、force push 或暂存无关文件。

## 7. 失败与恢复

任何测试失败、审查拒绝、证据不足、发布失败或规格冲突都会停止执行，当前 task 保持未完成。需要人工或规格决策的长期阻塞可写为：

```text
_Blocked: <exact reason>_
```

问题解决后再次调用同一命令：

```text
$kiro-impl <feature>
```

Skill 会重新读取 `tasks.md`，从首个依赖满足的未完成 task 继续。没有迭代额度，也没有外部运行态需要恢复。

## 8. 最终人工接受

`VALIDATE` 必须运行完整测试、静态检查、可信 smoke、Requirements traceability、跨 task 契约和设计边界检查。测试通过并不自动等于 `GO`。

PR 标记 Ready 后，仍由人类对照 `docs/PRD.md`、批准的 Feature spec、验证证据和实际 Diff 决定是否合并或发布。

一句话记忆：项目级事实看 `docs/PRD.md`；Feature 事实和状态看 cc-sdd 三文档；批准后用 `$kiro-impl <feature>` 直接执行全部剩余 task。
