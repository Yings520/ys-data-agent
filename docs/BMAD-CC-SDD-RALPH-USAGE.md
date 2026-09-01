# BMAD + cc-sdd + Ralph TUI 使用手册

本文说明如何在 `ys-data-agent` 中，把一个产品想法依次交给 BMAD、cc-sdd 和 Ralph TUI，直到完成编码、Review、验证和人工验收。

文中的 `<feature>`、`<实际目录>`、`<commit>` 等尖括号内容都是待替换变量；执行命令前应替换成真实值，不要连同尖括号原样输入。

## 1. 先分清两种命令

### 1.1 发给 Codex 的聊天指令

以下指令输入在 **Codex 对话框**，不是 Terminal：

```text
$bmad-product-brief
$bmad-prd
$kiro-spec-init ...
$kiro-spec-requirements <feature>
$kiro-spec-design <feature>
$kiro-spec-tasks <feature>
```

你可以在指令后直接补充自然语言、文件路径和约束。Skill 会读取仓库、与你对话，并创建或更新 Markdown/JSON 文件。

### 1.2 在 Terminal 执行的命令

以下命令才输入在项目根目录的 Terminal：

```bash
rtk ralph-tui doctor
rtk ./scripts/ralph-cc-sdd.sh <feature>
rtk ralph-tui status
rtk ralph-tui resume
rtk ralph-tui logs
```

本文中所有终端命令都假设当前目录是：

```text
/Users/ysc/Documents/Data_Engineering/projects/ys-data-agent
```

## 2. 根据工作大小选择路径

| 情况 | 使用路径 |
|---|---|
| 产品方向还不清楚 | Product Brief → PRD → cc-sdd → Ralph |
| 产品方向明确，但还没有正式需求 | PRD → cc-sdd → Ralph |
| 已有批准的 PRD，只开发其中一个功能 | cc-sdd → Ralph |
| 很小、一次对话能完成的修复 | 不使用 BMAD 和 Ralph；按仓库 TDD、Review、验证约束完成 |

BMAD 在本仓库只允许使用 `bmad-product-brief` 和 `bmad-prd`。不要再用 BMAD 生成架构、Epic、Story、Sprint 或实现任务。

## 3. 第一次使用前检查

在 Terminal 执行：

```bash
rtk git status --short
rtk ralph-tui doctor
rtk ralph-tui config show
```

预期：

- `doctor` 显示 `HEALTHY`。
- Agent 是 `codex`。
- Tracker 是 `json`。
- Command 是 `./scripts/codex-ralph`。
- 已有未提交文件属于你时，先记住它们；Agent 不应覆盖或回滚这些文件。

## 4. 阶段一：用 BMAD 明确产品

### 4.1 Product Brief：可选

只有在产品方向、目标用户或价值主张还不稳定时才需要 Brief。

在 Codex 对话框输入：

```text
$bmad-product-brief

创建一个 Product Brief。我的初步想法是：
- 产品/功能：为数据查询 Agent 增加查询历史导出
- 目标用户：需要保存和分享查询结果的数据工程师
- 当前问题：查询结果只能在当前会话查看
- 希望变化：用户可以可靠地导出历史查询及必要元数据
- 项目性质：个人项目，但希望达到可发布质量
```

BMAD 会先让你补充背景，再让你选择：

- `Fast path`：集中问一两轮问题，快速生成，推断处标记 `[ASSUMPTION]`。
- `Coaching path`：逐段讨论，适合方向还不够清楚时。

完成后，Skill 会告诉你实际文件路径。默认类似：

```text
_bmad-output/planning-artifacts/briefs/
└── brief-ys-data-agent-YYYY-MM-DD/
    ├── brief.md
    ├── addendum.md       # 有额外技术/背景材料时才出现
    └── .memlog.md        # 决策与变更记录
```

不要自己猜文件夹名称，以 Skill 最终返回的路径为准。

### 4.2 PRD：必需

如果有 Brief，在一个新的 Codex 对话中把它作为输入：

```text
$bmad-prd

基于下面的 Product Brief 创建 PRD：
_bmad-output/planning-artifacts/briefs/<实际目录>/brief.md

这是个人项目，采用 Fast path。PRD 只描述为什么做、做什么、范围、FR/NFR、非目标和成功指标；不要生成架构、技术设计、Epic、Story 或实施任务。
```

如果产品方向已经明确，可以直接跳过 Brief：

```text
$bmad-prd

创建 PRD。我要解决的问题是……
目标用户是……
当前情况是……
希望用户最终能够……
明确不做的内容是……
```

默认产物类似：

```text
_bmad-output/planning-artifacts/prds/
└── prd-ys-data-agent-YYYY-MM-DD/
    ├── prd.md
    ├── addendum.md       # 需要时出现
    ├── .memlog.md
    └── review-*.md       # 执行 Reviewer Gate 时出现
```

### 4.3 人工批准 PRD

打开 `prd.md`，至少确认：

- 问题、用户和产品价值正确。
- In Scope、Out of Scope 和非目标明确。
- FR/NFR 可验证，没有混入数据库、框架、API 结构等实现方案。
- 成功指标和反指标合理。
- 所有 `[ASSUMPTION]`、开放问题和阻塞项均已处理。
- Frontmatter 中 `status: final`。

批准后，在 Terminal 提交 PRD，并记录提交哈希：

```bash
rtk git add _bmad-output/planning-artifacts/prds/<实际目录>
rtk git commit -m "docs(product): approve query history export PRD"
rtk git rev-parse HEAD
```

后面的 cc-sdd Requirements 必须引用这个 PRD 路径、提交哈希和具体章节。

## 5. 阶段二：用 cc-sdd 建立开发合同

以下示例假设 cc-sdd 生成的 feature 名为：

```text
query-history-export
```

实际操作时，始终使用 `$kiro-spec-init` 返回的名称，不要中途改名。

### 5.1 初始化 Spec

在 Codex 对话框输入：

```text
$kiro-spec-init

为查询历史导出功能初始化规格：
- 谁有问题：需要保存和分享查询记录的数据工程师
- 当前情况：查询历史只能在当前产品会话中查看，无法可靠导出
- 希望变化：用户可以导出 PRD 规定范围内的查询历史和元数据
- 上游 PRD：_bmad-output/planning-artifacts/prds/<实际目录>/prd.md
- PRD 提交：<git rev-parse HEAD 返回值>
- 覆盖章节：<例如 FR-3、FR-4、NFR-2>
```

这一步只创建骨架，不会生成完整 Requirements、Design 或 Tasks：

```text
.kiro/specs/query-history-export/
├── spec.json
└── requirements.md
```

`spec.json` 是阶段和审批状态；`requirements.md` 此时只包含项目描述。

### 5.2 生成 Requirements

在 Codex 对话框输入：

```text
$kiro-spec-requirements query-history-export
```

Skill 会生成 EARS 风格的可测试需求，并执行 Requirements Review Gate。

打开以下文件审核：

```text
.kiro/specs/query-history-export/requirements.md
```

必须确认：

- `Upstream Product Source` 中有真实 PRD 路径、提交哈希和章节，不是模板占位符。
- Requirements 只写用户可观察的“做什么”，不写技术实现。
- Requirement 标题和验收条件使用稳定的数字 ID，例如 `1`、`1.1`、`2.3`。
- In Scope、Out of Scope 和边界行为没有歧义。
- 每条验收条件都能通过测试或可重复操作验证。

如果要修改，直接在对话中说明问题并重新运行同一 Skill。确认后输入：

```text
我已审核并批准 .kiro/specs/query-history-export/requirements.md。
只把 spec.json 中 approvals.requirements.approved 更新为 true，不执行下一阶段。
```

### 5.3 生成 Technical Design

Requirements 批准后，在 Codex 对话框输入：

```text
$kiro-spec-design query-history-export
```

根据功能类型，Skill 会进行完整、轻量或最小 Discovery，然后生成：

```text
.kiro/specs/query-history-export/
├── design.md
└── research.md
```

打开 `design.md`，至少审核：

- 每个 Requirement 都映射到组件和接口。
- Boundary Commitments 明确说明本 Spec 拥有什么、不拥有什么。
- Allowed Dependencies 和 Revalidation Triggers 明确。
- File Structure Plan 列出具体新建/修改文件及单一职责。
- 测试策略直接来自 Requirements 的验收条件。
- 没有无关重构或为未来假设预留的大型抽象。

批准后输入：

```text
我已审核并批准 .kiro/specs/query-history-export/design.md。
只把 spec.json 中 approvals.design.approved 更新为 true，不执行下一阶段。
```

### 5.4 生成 Tasks

个人项目推荐串行任务图。在 Codex 对话框输入：

```text
$kiro-spec-tasks query-history-export --sequential
```

Skill 会先做 Task Plan Review，再做独立 Task-Graph Sanity Review，然后写入：

```text
.kiro/specs/query-history-export/tasks.md
```

当 Skill 询问是否批准时，先打开文件，确认每个可执行叶子任务：

- 工作量约为 1–3 小时。
- 有至少一个可以观察到的完成结果。
- 有 `_Requirements: 1.1, 1.2_`。
- 有 `_Boundary: <文件或组件>_`。
- 有 `_Depends: none_` 或真实前置任务 ID。
- 不跨越多个职责边界；跨边界工作被明确标为集成任务。
- 测试、运行时准备和配置没有被隐含省略。

满意后在同一对话回复：

```text
批准任务，继续更新审批状态。
```

此时必须满足：

```json
{
  "approvals": {
    "requirements": { "approved": true },
    "design": { "approved": true },
    "tasks": { "approved": true }
  }
}
```

首次使用不建议给 `$kiro-spec-design` 或 `$kiro-spec-tasks` 添加 `-y`。`-y` 会自动批准前置阶段，适合你已经在外部完成审核的批处理场景，不适合跳过人工阅读。

### 5.5 提交批准后的开发合同

在启动 Ralph 前提交 Spec：

```bash
rtk git add .kiro/specs/query-history-export
rtk git commit -m "docs(query-history-export): approve cc-sdd spec"
rtk git status --short
```

`.kiro/specs/<feature>/tasks.md` 从此是唯一实施任务源。不要另建 BMAD Story、Ralph PRD 或第二份任务列表。

## 6. 阶段三：启动 Ralph 自动执行

### 6.1 预检

在 Terminal 执行：

```bash
rtk ralph-tui doctor
rtk node tools/workflow/cc-sdd-to-ralph.mjs query-history-export
rtk node tools/workflow/cc-sdd-to-ralph.mjs query-history-export --check
```

第一条转换命令会生成：

```text
.ralph-tui/generated/query-history-export.json
```

它只是 `tasks.md` 的运行时投影，已被 Git 忽略。不要手工编辑它。第二条 `--check` 用于确认投影与当前 `tasks.md` 一致。

### 6.2 启动循环

```bash
rtk ./scripts/ralph-cc-sdd.sh query-history-export
```

启动脚本会再次生成最新投影，然后以串行方式启动 Ralph。

Ralph 每一轮只做一件事：

1. 从生成的 JSON 选择一个依赖已满足的任务。
2. 调用 `$run-cc-sdd-task query-history-export <task-id>`。
3. 检查投影、审批状态、任务依赖和修改前工作区。
4. 对当前任务执行 RED → GREEN → REFACTOR。
5. 执行任务相关测试和机械检查。
6. 使用 `kiro-review` 做任务局部 Review。
7. 使用 `kiro-verify-completion` 检查新鲜证据。
8. 只有全部通过，才把当前任务改成 `[x]` 并向 Ralph 返回完成信号。
9. 下一轮重新读取 `tasks.md`，选择下一个任务。

你不需要、也不应该手工调用 `$run-cc-sdd-task`。它是 Ralph 的内部执行入口。

项目配置为 `autoCommit = false`：Ralph 不会自动暂存或提交文件，避免把你原有的工作区改动误带入提交。每次中断或完成一批任务后，都应先审查 `git diff`，再显式暂存当前功能的文件。

失败策略来自 `.ralph-tui/config.toml` 的 `[errorHandling] strategy = "abort"`，不是启动脚本参数。任一 Agent 轮次返回非零状态时，Ralph 会停止循环。此时先查看对应日志、`tasks.md` 和 `git diff`；解决失败或阻塞后，再使用 `rtk ralph-tui resume` 恢复原 Session，或重新运行启动脚本创建新循环。

### 6.3 查看状态和日志

另开一个 Terminal：

```bash
rtk ralph-tui status
rtk ralph-tui status --json
rtk ralph-tui logs
rtk ralph-tui logs --iteration 3
rtk ralph-tui logs --task 1.2
```

同时可以直接检查权威状态：

```bash
rtk cat .kiro/specs/query-history-export/tasks.md
rtk git log --oneline -10
rtk git status --short
```

### 6.4 中断、恢复和重新运行

需要暂停时可以正常终止 Ralph。恢复同一个 Ralph Session：

```bash
rtk ralph-tui resume
```

如果旧 Session 已结束，希望从 `tasks.md` 的当前状态开始一个新循环：

```bash
rtk ./scripts/ralph-cc-sdd.sh query-history-export
```

已完成任务已经是 `[x]`，不会被当作待执行任务重新实现。恢复前仍要检查未提交 diff，因为 Ralph 本身不负责 Git 提交。

默认最多执行 10 轮。任务超过 10 个或某轮提前停止时，先检查任务、日志和 Git 状态，再恢复或重新运行；不要为了“继续跑”而删除审批或手工修改生成 JSON。

## 7. 最终验证和人工验收

普通任务全部完成后，转换器会提供一个保留任务：

```text
VALIDATE
```

它不是编码任务。`VALIDATE` 会让 cc-sdd：

- 运行完整测试集和真实 Smoke 检查。
- 检查 Requirements 覆盖率。
- 检查跨任务接口和共享状态。
- 检查 Design、依赖方向和 File Structure Plan。
- 检查 Boundary Violations、阻塞任务、残留占位符和疑似秘密。

只有返回 `GO`，Ralph 才能把它标记完成：

- `NO-GO`：存在具体失败，修复后重新验证。
- `MANUAL_VERIFY_REQUIRED`：缺少环境或人工操作，不能视为完成。
- `GO`：工程验证通过，可以进入产品验收。

最终由你按 BMAD PRD 做人工验收：

- 产品行为满足 PRD 的 FR/NFR。
- 非目标没有被偷偷加入。
- 成功指标具备采集或验证方式。
- 所有 `tasks.md` 叶子任务和 `VALIDATE` 都完成。
- Git 工作区没有意外修改。
- 发布所需环境门禁已经执行。

审查并提交经过验证的功能文件：

```bash
rtk git diff --check
rtk git status --short
rtk git add path/to/changed-file.rs .kiro/specs/query-history-export/tasks.md
rtk git commit -m "feat(query-history-export): complete approved feature"
```

不要使用 `git add .` 或 `git add -A`，以免把用户原有改动、日志或本地状态加入提交。

本仓库完整发布门禁：

```bash
rtk ./scripts/v0.2-release-gate.sh
```

## 8. 常见失败怎么处理

| 现象 | 原因 | 处理方式 |
|---|---|---|
| `Requirements not yet approved` | `requirements.approved` 仍为 `false` | 审核 `requirements.md`，明确批准后更新 `spec.json` |
| `Requirements and design must be approved` | Design 尚未批准 | 审核 `design.md`，批准后再生成 Tasks |
| `cc-sdd tasks are not approved` | Tasks 未通过人工批准 | 审核 `tasks.md`，把 `approvals.tasks.approved` 更新为 `true` |
| `Task ... is missing _Boundary:_` | 任务不满足调度合同 | 返回 `$kiro-spec-tasks` 修订任务，不要修改 Ralph JSON |
| `Task ... has unknown dependency` | `_Depends:_` 引用了不存在的任务 | 修复任务图并重新审批 |
| `Task ... is blocked` | Agent 已记录真实阻塞 | 阅读 `_Blocked: ..._`，解决产品/设计/环境问题后再重新生成投影 |
| `Ralph projection is stale` | `tasks.md` 与生成 JSON 不一致 | 停止当前循环，确认改动，然后重新运行启动脚本 |
| Review 返回 `REJECTED` | 当前实现不满足任务或 Spec | 让当前任务按 Review 结果修复；不要扩大到其他任务 |
| 验证返回 `MANUAL_VERIFY_REQUIRED` | 缺少服务、凭据、运行环境或人工 UI 验证 | 完成指定验证并保留证据，再重新运行验证 |
| Ralph doctor 失败 | Ralph/Codex 版本或配置变化 | 运行 `rtk ralph-tui config show`，检查 `scripts/codex-ralph`，再运行 doctor |

## 9. 不要这样做

- 不要使用 BMAD Architecture、Epic、Story、Sprint 或 Dev 流程。
- 不要同时让 BMAD、cc-sdd、GitHub Issue 和 Ralph 各维护一套实施任务。
- 不要在 Ralph 运行时手工执行无任务范围的 `$kiro-impl <feature>`。
- 不要手工编辑 `.ralph-tui/generated/*.json`。
- 不要看到测试通过就绕过 Review、`kiro-verify-completion` 或最终 `VALIDATE`。
- 不要用 `-y` 掩盖尚未实际完成的人工审核。
- 不要在 Spec 冲突时让 Agent自行选择产品行为；返回 PRD/Requirements 澄清。

## 10. 最短可复制流程

以下顺序不能交换：cc-sdd 初始化时需要引用一个已经提交的 PRD commit。

### 第一步：在 Codex 对话框创建 PRD

```text
$bmad-prd
创建 <功能> 的 PRD。目标用户是……当前问题是……希望变化是……非目标是……
```

### 第二步：在 Terminal 提交 PRD

```bash
rtk git add _bmad-output/planning-artifacts/prds/<实际目录>
rtk git commit -m "docs(product): approve <功能> PRD"
rtk git rev-parse HEAD
```

保存最后一条命令返回的 commit，然后继续。

### 第三步：在 Codex 对话框创建并批准 cc-sdd Spec

```text
$kiro-spec-init
为 <功能> 初始化规格。谁有问题：……当前情况：……希望变化：……
上游 PRD：<prd.md 路径>；提交：<commit>；覆盖章节：<章节 ID>。

$kiro-spec-requirements <feature>

我已审核并批准 requirements.md。只更新 requirements.approved，不执行下一阶段。

$kiro-spec-design <feature>

我已审核并批准 design.md。只更新 design.approved，不执行下一阶段。

$kiro-spec-tasks <feature> --sequential

批准任务，继续更新审批状态。
```

### 第四步：在 Terminal 启动 Ralph

```bash
rtk git add .kiro/specs/<feature>
rtk git commit -m "docs(<feature>): approve engineering contract"
rtk ralph-tui doctor
rtk ./scripts/ralph-cc-sdd.sh <feature>
```

一句话记忆：**你在 Codex 对话框里批准产品和工程合同；Ralph 只在合同批准后，循环调度 cc-sdd 的单个任务。**
