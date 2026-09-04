# Requirements Document

## Introduction

`tui-interaction` 定义 v0.2 本地 TUI 的界面和交互。布局与信息层级以 `docs/prototypes/ysda-tui.html` 为基准。

TUI 只公开 `/mode`、`/model` 和 `/exit`。Query 完成后显示结果卡片；用户按 Enter 或点击卡片查看详情。

所有展示状态和配置操作都经过 `AgentService`。TUI 不读取 Repository、配置文件或 Secret，也不自行推断产品状态。

## Upstream Product Source

- **BMAD PRD**: `docs/PRD.md`
- **Source revision**: 2026-09-03 工作树版本，基于 commit `02830a7a9535c3e5115416303a9a2b0c21fe5153`
- **Covered PRD sections**: §5、§7.2、§13.4、§19、§21、§22、§23、§26.1、§26.3、§26.6、§26.9、§27、§29

> 本文件只定义该 Feature 的用户行为。产品范围或稳定架构如需变化，应先更新 `docs/PRD.md`。

## Scope

**In scope**

- 原型中的 Header、Timeline、结果详情、Composer 和 Footer。
- `/mode`、`/model` 及其键盘选择流程。
- `Providers / Plans → Models` 两级模型选择。
- AgentService 提供的非敏感 Display Context。
- 宽屏、标准和紧凑终端布局。

**Out of scope**

- `/providers`、`/query`、`/artifact`、`/sql` 和 `/datasource`。
- `Custom` 模型、任意 Base URL、自定义 Provider 协议或新 Provider。
- Query 之外的新 Workflow。
- Web UI 或必须使用鼠标的操作。

## Requirements

### Requirement 1: 壳层与显示上下文

**Objective:** 用户能随时确认当前环境和 Query 状态。

#### Acceptance Criteria

1. TUI 应按原型实现 Header、内容区、固定 Composer、Footer、边框层级和深色配色。
2. Header 应显示 Workspace、活动数据源或不可用状态、Mode、当前模型、只读状态和 Query 状态。
3. Workspace、数据源、只读状态和 Query 状态必须来自 AgentService 的非敏感 Display Context。
4. Mode 只来自 TUI 当前选择，显示为 `AUTO › QUERY` 或 `QUERY`。
5. 内容滚动或页面切换时，Header、Composer 和 Footer 应保持可见。
6. 空间不足时，TUI 应保留产品标识、主要内容、Composer 和当前键位，且文本不得重叠。

### Requirement 2: Timeline 与结果详情

**Objective:** 用户无需阅读内部日志，也能判断 Query 是否成功以及结果是否可用。

#### Acceptance Criteria

1. Timeline 应显示用户问题、关键 Query 阶段、当前状态、警告和主要结果。
2. 等待输入、拒绝、失败和取消必须明确显示原因与下一步，不得伪装为成功。
3. Query 成功后，Timeline 应显示答案摘要、验证状态、关键结果和可聚焦的结果卡片。
4. 结果卡片获得焦点时，Enter 应打开 `Results`；终端支持鼠标时，点击卡片应执行相同操作。
5. 结果详情应包含 `Summary`、`Results`、`SQL`、`Schema` 和 `Evidence` 五个页签。
6. 页签内容必须来自持久化 Artifact，并受现有 Policy 限制；缺失或受限内容应显示真实原因。
7. 切换页签或按 Esc 返回 Timeline 不得重新提交 Query、调用 Tool 或恢复任务。
8. Timeline 和结果详情不得使用生产 fixture、固定 SQL 或演示数据。

### Requirement 3: 最小命令面板与 Mode

**Objective:** 用户只需三个命令即可选择 Mode、Model 或退出应用。

#### Acceptance Criteria

1. Composer 为空，或 `/` 是首个非空白字符时，输入 `/` 应打开命令面板。
2. 命令目录、面板、Footer 和帮助界面只可公开 `/mode`、`/model` 与 `/exit`。
3. 命令面板应支持实时搜索、↑/↓ 移动、Enter 确认和 Esc 取消。
4. 没有匹配项时，面板应保持打开并允许继续编辑。
5. `/mode` 只提供 `Auto` 和 `Query`，并使用与命令面板一致的键盘操作。
6. `Auto` 在 v0.2 中解析到 Query；`Query` 显式锁定 Query。两者当前执行结果相同，但用户意图不同。
7. 选择 Mode 不得改变 Policy、Tool Runtime、QueryBudget、数据外发限制、Provider binding 或 Completion Gate。
8. 取消选择时，应恢复原 Mode、Composer 内容和页面。

### Requirement 4: Model Selection

**Objective:** 用户能从真实 Provider 或 Plan 中找到并激活模型。

#### Acceptance Criteria

1. `/model` 应打开 `Model Selection`，顶层只显示 `Providers` 和 `Plans`。
2. Tab、← 和 → 应切换顶层页签；↑/↓、搜索、Enter 和 Esc 应遵循统一选择规则。
3. Provider、Plan、Model、显示名称和状态必须来自受治理 Catalog、Profile、发现结果和活动配置。
4. 顶层候选应显示 `Configured`、`Needs setup` 或 `Unavailable`；当前项应另有唯一的 `Current` 标记。
5. `Needs setup` 只能进入既有 Provider 配置流程；TUI 不再提供独立 `/providers` 入口。
6. 选择可用 Provider 或 Plan 后，应进入其 Model 列表，并显示模型与验证状态。
7. 已验证模型可以请求激活；未配置、未验证、验证失效或能力不足的模型必须阻止激活并给出下一步。
8. 模型发现失败时，应保留已保存且状态可证明的候选，不得回退到假数据。
9. 从子层返回时，应恢复原页签、搜索内容和高亮位置。

### Requirement 5: 激活与 Run 边界

**Objective:** TUI 的“当前模型”必须与 Runtime 实际使用的模型一致。

#### Acceptance Criteria

1. TUI 只能通过 Service 发起模型切换。
2. 激活成功后，TUI 必须重新读取活动配置，再更新 Header 和 `Current` 标记。
3. 激活失败、超时、取消或冲突时，原活动模型必须保持不变。
4. 切换只影响之后启动的 Run；运行中的 Run 继续使用启动时绑定的 Provider、Model、Credential 和参数。
5. 应用重启后，TUI 应从已保存的活动 Profile 恢复当前模型。
6. 没有可用活动模型时，TUI 应明确显示不可用状态，并阻止提交无法执行的 Query。

### Requirement 6: 安全、错误与回归

**Objective:** TUI 只显示真实、允许披露且可验证的产品状态。

#### Acceptance Criteria

1. TUI 不得直接访问 Provider、Repository、数据库、Artifact Store 或 Secret Store。
2. Display Context 刷新失败时，应保留最后一次成功值，并显示“状态暂不可用”；不得用本地配置或猜测值回退。
3. 普通界面不得显示 Credential、Token、完整请求、受限业务数据、原始 Tool payload 或内部 ID。
4. Service 错误应显示稳定原因和恢复动作，同时保留原活动配置与已输入的非敏感内容。
5. 网络发现、认证和验证进行时，TUI 应保持可响应，并允许安全取消或返回。
6. Timeline 有完成结果卡片时，Footer 显示 `/mode  /model  Enter open results`。
7. 结果详情页的 Footer 显示 `/mode  /model  Esc back`。
8. 发布前必须通过真实键盘、关键渲染、失败原子性、结果导航不重跑和新旧 Run 绑定回归。
