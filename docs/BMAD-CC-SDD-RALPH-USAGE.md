# 项目 Change 工作流使用手册

本项目只维护一个项目总纲，再按 Change 大小选择执行路径：

```text
docs/PRD.md
    ↓
Change
  ├── 小改动 → 直接 Code Agent → Code + Test + Review + Fresh Verification
  └── Feature  → cc-sdd requirements.md → design.md → tasks.md
                                      ↓
                                  Ralph TUI
                                      ↓
                              Code Agent Loop
                                      ↓
                          Code + Test + Review
```

## 1. 三个系统各做什么

| 系统 | 职责 | 不负责 |
|---|---|---|
| BMAD | 创建、更新或校验唯一的项目总纲 `docs/PRD.md` | Feature 级需求/设计/任务、编码、Sprint |
| cc-sdd | 为 Feature 维护 Requirements → Design → Tasks | 全局产品文档、运行循环 |
| Ralph TUI | 按批准的 `tasks.md` 每轮调度一个 Code Agent | 需求、设计、任务定义 |

`docs/PRD.md` 保存整个 ys-data-agent 的产品思考、稳定架构、当前发布边界、全局不变量和后续演进计划。它可以记录跨 Feature 长期有效的架构 HOW，但不保存任何单个 Feature 的详细 Requirements、Design 或 Tasks。

## 2. 永久文件与临时文件

永久维护：

```text
docs/PRD.md

.kiro/specs/<feature>/
├── requirements.md
├── design.md
├── tasks.md
└── spec.json          # 阶段与人工批准状态，不是规划文档
```

Ralph 运行状态可随时重建：

```text
.ralph-tui/generated/
.ralph-tui/iterations/
.ralph-tui/progress.md
```

不要手工修改或提交 Ralph 运行状态。

## 3. 第一步：判断 Change 类型

### 小改动

只有同时满足下列条件才走直接 Code Agent：

- 不改变 `docs/PRD.md` 中的项目范围、稳定架构或发布边界；
- 不增加用户可见能力；
- 不改变公共契约或持久数据形状；
- 责任边界和影响文件清楚；
- 一个有界 Agent 会话可以安全完成。

典型例子：局部 bug、文案、明确的配置修正、局部测试补充、保持行为不变的小型重构。

### Feature

任一条件成立就使用 cc-sdd：

- 新增或显著改变用户行为；
- 改变公共接口、事件或持久状态；
- 引入外部集成或跨模块责任；
- 存在真实的方案选择或需求歧义；
- 需要多个可独立完成和验收的任务。

Feature 如果改变产品愿景、稳定架构、发布边界或演进顺序，先更新 `docs/PRD.md`；Feature 自身的详细需求仍写入 cc-sdd。

## 4. 如何执行小改动

小改动不调用 BMAD、不创建 cc-sdd spec、不启动 Ralph。在 Codex 对话中直接给出边界和完成条件，例如：

```text
这是一个小改动：修复 <问题>。
范围只限 <模块/文件/行为>，不得改变公共契约或做无关重构。
请直接实现，运行相关测试，Review 实际 diff，并用新鲜命令证据验证后再报告完成。
```

最小闭环仍然是：

```text
理解现有行为
    ↓
行为变化时先复现失败
    ↓
最小实现 + 测试
    ↓
Review 实际 diff
    ↓
重新运行验证命令
```

如果实施中发现影响面比预期大，停止直接修改，重新分类为 Feature。

## 5. 如何执行 Feature

下面以 `provider-management` 为例。

### 5.1 先判断是否需要更新项目总纲（通常跳过）

不要因为“这是一个 Feature”就默认运行 BMAD：

- **普通 Feature 默认跳过 BMAD：** Feature 的 Requirements 直接进入 cc-sdd。只要它不改变项目方向、稳定架构、发布边界或演进顺序，就直接执行 §5.2/§5.3。
- **只有项目级事实变化才运行 `$bmad-prd`：** 例如改变目标用户或产品定位、把原本排除的能力放进当前版本、改变跨 Feature 的稳定架构，或者调整版本演进顺序。

BMAD 只维护 `docs/PRD.md`。Feature 的用户行为、FR/NFR、验收标准、接口、数据结构和任务继续写在 `.kiro/specs/<feature>/`，不要复制进项目总纲，也不要为 Feature 创建第二份 PRD。

#### 当前 `provider-management` 为什么需要更新

当前 `docs/PRD.md` §26.5 明确把“多种非 OpenAI 协议 Provider”排除在 v0.2 之外，而 `provider-management` 希望在当前版本支持 9 个 Provider。这改变了项目发布边界，因此需要先在 Codex 对话中输入：

```text
$bmad-prd

更新 docs/PRD.md，使 provider-management 可以进入当前版本范围。
只修改项目级 Provider 架构、v0.2 发布边界和演进计划。
不要把 Provider Feature 的 FR、NFR、AC、TUI 字段或实现细节写入 docs/PRD.md；
这些内容继续由 .kiro/specs/provider-management/requirements.md 管理。
```

Agent 修改完成后检查实际 Diff；确认项目级边界准确时，回复“我批准这次 `docs/PRD.md` 更新”。这只是批准同一份项目总纲的修改，不会创建新的 PRD。然后继续 §5.3。

### 5.2 初始化 Feature

对一个全新的 Feature，在 Codex 对话中输入：

```text
$kiro-spec-init "<Feature 描述>；project design: docs/PRD.md"
```

输出：

```text
.kiro/specs/<feature>/spec.json
.kiro/specs/<feature>/requirements.md
```

本仓库的 `provider-management` 已完成初始化并保存了迁移后的输入基线，不要再次运行 `$kiro-spec-init`。它当前应直接进入下一步 Requirements。

### 5.3 Requirements

```text
$kiro-spec-requirements provider-management
```

人工检查它是否完整描述 WHAT、可测试边界、异常和与 `docs/PRD.md` 的追踪，不提前决定 HOW。批准时告诉 Agent：

```text
我批准 requirements.md，请把 spec.json 中 approvals.requirements.approved 设置为 true。
```

### 5.4 Design

```text
$kiro-spec-design provider-management
```

人工检查责任边界、组件契约、数据与错误模型、文件职责和验证策略。所有实施所需结论必须自包含在 `design.md`。批准时告诉 Agent：

```text
我批准 design.md，请把 spec.json 中 approvals.design.approved 设置为 true。
```

### 5.5 Tasks

```text
$kiro-spec-tasks provider-management
```

每个可执行叶子任务应在一个 Agent 运行内完成，通常约 1–3 小时；它必须有需求映射、清楚边界、非显然依赖和可观察完成条件。批准时告诉 Agent：

```text
我批准 tasks.md，请把 spec.json 中 approvals.tasks.approved 设置为 true。
```

`tasks.md` 是唯一任务源。不要再维护 Story、Sprint 或另一份 Ralph 任务文件。

### 5.6 启动 Ralph

从这里开始使用终端：

```bash
rtk ralph-tui doctor
rtk ./scripts/ralph-cc-sdd.sh provider-management
```

启动器会校验人工批准和任务图，生成一次性 Ralph JSON，然后串行调度。每轮只调用：

```text
$run-cc-sdd-task provider-management <task-id>
```

普通任务只有在范围内实现、测试、独立 Review 和新鲜验证全部通过后，才能勾选 `tasks.md` 中当前项。所有任务完成后，保留任务 `VALIDATE` 执行 Feature 全量验证；最后由人按 `docs/PRD.md` 与已批准的 Feature spec 验收结果。

## 6. Ralph 查看与恢复

```bash
rtk ralph-tui status
rtk ralph-tui logs
rtk ralph-tui logs --iteration 3
rtk ralph-tui resume
```

如果人工修改了 `requirements.md`、`design.md` 或 `tasks.md`：

1. 停止当前 Ralph 会话；
2. 重新完成受影响阶段的批准；
3. 再运行 `rtk ./scripts/ralph-cc-sdd.sh <feature>` 重新生成投影。

不要直接修补 `.ralph-tui/generated/*.json`。

## 7. 日常速查

```text
Change
  小改动？
    是 → 直接 Code Agent + Test + Review + Fresh Verification
    否 → Feature
           项目方向/稳定架构变化？是 → $bmad-prd 更新 docs/PRD.md
           $kiro-spec-init
           $kiro-spec-requirements → 人工批准
           $kiro-spec-design       → 人工批准
           $kiro-spec-tasks        → 人工批准
           rtk ./scripts/ralph-cc-sdd.sh <feature>
```

一句话记忆：项目级产品、稳定架构和演进事实看 `docs/PRD.md`；Feature 事实看 cc-sdd 三文档；小改动直接做；Ralph 只执行批准后的 Feature 任务。
