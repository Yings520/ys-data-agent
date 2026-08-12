# YS Data Agent 完整架构设计草案

**日期：** 2026-08-06  
**状态：** 已按 2026-08-12 产品愿景讨论修订，作为长期产品与 v0.2 架构基线  
**替代范围：** 本文扩展并修正 2026-08-02 v0.1 设计中的长期架构；v0.1 文档继续作为已实现 Demo 的历史记录

---

## 1. 文档目的

本文定义 YS Data Agent 的长期产品定位、核心领域模型、运行时架构、权限边界、数据上下文、工具与执行模型、可观测性、评测体系、Rust 仓库结构，以及 v0.2 与长期能力之间的边界。

本文不是五类 Agent 的一次性实现清单。它首先建立所有 Workflow 共用的控制主干，再让 Query、Analysis、Build/Change、Operate 和 ML Data Prep 按垂直切片逐步接入。

## 2. 产品定位

YS Data Agent 是一个受业务责任人治理的 full-stack AI data team，面向没有能力或预算组建完整数据团队的中小型公司。

它的长期愿景是：

> 让大多数无力负担完整专业数据团队成本的中小型公司，通过 YS Data Agent 获得接近大厂的数据工程、数据分析和数据科学服务能力，让数据能够被简单地接入、治理、管理、查询和使用，而不要求用户掌握复杂的数据专业知识。

它的核心价值不是生成一段 SQL 或代码，而是把数据工作推进到可交付状态：理解业务目标、选择和管理成熟的数据基础设施、执行工作、验证结果、披露限制并交付可审计的 Artifact。复杂性由系统和底层专业工具吸收，用户主要表达业务目标、提供必要授权并确认关键业务判断。

`full-stack AI data team` 是长期产品定位，不表示任何单个 v0.x 版本已经交付完整能力。每个版本必须同时声明当前直接用户、数据成熟度前置条件、已启用 Workflow 和明确不支持的任务。

### 2.1 终局客户、用户与责任

终局客户包括两类中小型公司：

- 已经拥有数据库、Warehouse、dbt 或 Orchestrator，但没有完整数据团队；
- 只有 Excel、CSV、SaaS 或业务数据库，尚未建立 Warehouse、数据模型和治理体系。

终局产品不要求客户常驻 Data Engineer，但要求每个 Workspace 指定至少一名 **业务数据责任人（Accountable Data Owner）**。该角色不需要理解 SQL、数据建模或调度系统，其不可委托的责任是：

- 确认指标含义、关键业务规则和结果用途；
- 决定谁可以访问哪些数据；
- 批准生产写入、部署、删除、高成本操作和其他高风险动作；
- 对 Agent 无法从事实中决定的业务冲突作最终选择。

YS Data Agent 负责把技术问题翻译为该角色可以理解的业务选择，提出推荐方案、影响和风险，并完成获准范围内的专业执行与验证。Agent 不能自行决定企业的业务真相，也不能因为用户不懂技术而扩大权限。

长期用户与能力如下：

| 用户 | 主要能力 | 权限边界 |
|---|---|---|
| 业务数据责任人 | 用业务语言确认口径、权限、成本和高风险动作 | 不要求技术能力；保留关键业务与授权决定权 |
| 产品或业务人员 | 受治理的指标查询与业务分析 | 只读；可以提交 ChangeRequest，不能修改代码或生产系统 |
| 数据分析师 | 指标拆解、趋势、异常和归因分析 | 只读；可以提交带证据的 ChangeRequest |
| Data Engineer | ETL/ELT 开发、调试、维护和 DataOps | 可以在隔离环境准备变更；Merge、Deploy 和生产写入继续受策略控制 |
| Data/ML Engineer | 数据清洗、特征定义、样本构建和质量验证 | 仅在授权数据域内工作；模型训练与 MLOps 不属于早期范围 |

### 2.2 客户数据成熟度与双接入路径

YS Data Agent 使用同一个产品入口和可信控制内核服务不同成熟度的客户，但按条件逐步开放能力：

| 客户状态 | 接入路径 | YS Data Agent 的责任 |
|---|---|---|
| 已有数据栈 | Bring Your Own Stack | 连接并管理客户已有的 Database、Warehouse、dbt、Orchestrator 和权限体系 |
| 没有完整数据栈 | Starter Data Stack | 通过 Workspace Bootstrap 诊断现状，选择、配置和管理一套受支持的标准化成熟工具组合 |

Starter Data Stack 不是 YS 自研数据库或调度器。它是由 PostgreSQL、DuckDB、dbt、Dagster、对象存储等成熟基础设施组成的受支持 Profile。YS Data Agent 拥有统一的控制、治理和用户体验，并通过 Adapter 管理这些工具。

两条路径最终共享相同的 Task、Workflow、Policy、Context、Artifact、Verification、Recovery 和 Eval 体系。它们不是两个产品；用户只看到 YS Data Agent，底层差异由 Workspace Profile 和 Capability Descriptor 隔离。

### 2.3 v0.2 首个用户与承诺

v0.2 的直接用户是 Data Engineer 和能够理解 SQL、数据口径与权限边界的技术型数据分析师。产品或业务人员在 v0.2 主要是 QueryArtifact 的间接消费者，不是本地 TUI 的主要操作者。

适合 v0.2 Pilot 的 Workspace 需要满足：

- 已有可查询的 SQLite 或 Postgres 数据源；
- 有可验证的最小权限只读身份；
- 有人负责确认时区、新鲜度规则、敏感数据策略和查询预算；
- 指标查询已有 Active Metric Contract；
- dbt manifest 可选；没有 dbt 时仍可使用 ObservedSchema，但可用 Context 较少。

v0.2 的产品承诺是：

> 对受支持的只读问题，完成受治理的上下文解析、安全执行、确定性验证和可审计交付；无法可信回答时明确澄清、警告或拒绝。

v0.2 不承诺归因分析、代码修改、Pipeline 恢复、业务用户自助 Web 入口或完整 AI data team 能力。

v0.2 也不实现 Workspace Bootstrap、Starter Data Stack、SaaS/Excel 接入、基础设施自动部署、托管控制平面或面向非技术责任人的完整治理向导。Data Engineer Pilot 是验证可信控制内核的版本切入点，不是长期产品对客户组织能力的要求。

## 3. 五类任务产物

系统对外强调任务结果，而不是内部 Agent 名称。

| Workflow | 必须交付的结果 |
|---|---|
| Query | 已执行的数据答案、SQL、数据来源、指标口径、时间范围和数据新鲜度 |
| Analysis | 分析结论、证据、假设、可复现查询、图表或 Dashboard Artifact |
| Build/Change | Pipeline、dbt 或 SQL 代码变更，测试结果、影响说明和可审查 Diff |
| Operate | 故障诊断证据、根因判断、恢复方案、执行记录和恢复后的健康验证 |
| ML Data Prep | 可复现的清洗与特征定义、数据集版本、质量检查和血缘 |

Dashboard 是 Analysis 的一种 Artifact，不单独建一个 Dashboard Agent。Pipeline 生成直接修改用户技术栈的原生 Artifact，不自研通用可执行 Pipeline DSL。

## 4. 产品边界与非目标

YS Data Agent 是现有数据平台之上的控制与智能层，不重新实现：

- 数据仓库或 Lakehouse；
- Airflow、Dagster 等调度器；
- Spark 等分布式计算引擎；
- 完整语义引擎、BI 引擎、缓存与预聚合平台；
- 通用跨框架 Pipeline 编译器；
- 自动拥有生产超级权限的常驻 Agent；
- v0.x 阶段的模型训练和完整 MLOps。

系统可以在本地使用 SQLite、DuckDB、Python、Polars 或沙箱执行轻量任务，但重计算和长任务应委托给现有 Warehouse、dbt、Spark 或 Orchestrator。

“不重新实现基础设施”不等于“只服务已经拥有数据平台的客户”。后续版本可以代表客户选择、Provision、配置、升级和运维受支持的 Starter Data Stack，但数据库、存储、计算、转换和调度仍由成熟产品提供。YS Data Agent 对组合后的用户体验、治理契约、验证结果和运维闭环负责，不把底层工具复杂度直接转嫁给非技术用户。

### 4.1 v0.2 Query 产品边界

v0.2 只接受三类 QueryIntent：

| QueryIntent | 目标 | 语义状态 | 完成要求 |
|---|---|---|---|
| GovernedMetric | 查询 Active Metric Contract 定义的指标 | Confirmed | Metric 版本、维度、时间范围、来源和新鲜度可验证 |
| AdHocRead | 执行不改变数据的受限事实查询 | Inferred | 明确假设、来源、范围和只读 Policy 结果，不伪装成正式指标 |
| Metadata | 查询 Schema、Owner、能力或新鲜度等元数据 | Observed | 结果来自授权 Connector 或可验证的 Context Evidence |

归因、趋势解释和“为什么”类任务属于 Analysis。代码或数据修改属于 Build/Change。任务重跑、恢复和生产诊断属于 Operate。

v0.2 遇到未实现能力时返回 `UnsupportedCapability`，说明当前边界、已获得的 Evidence 和对应后续 Workflow。它不得进入不存在的 Workflow、创建假的 ChangeRequest，或用普通 Query 冒充完成任务。

## 5. 架构原则

1. **Task-centric：** Session 管交互，Task 管目标，Run 管一次执行尝试。
2. **一个共享 Harness：** 五类 Workflow 共享生命周期、策略、事件、上下文和工具治理。
3. **Workflow 有领域差异：** Query、Build 和 Operate 不强行压成一个通用 DAG。
4. **关键行为 framework-free：** Agent Loop、状态转换、Tool 协议、策略和 Completion Gate 保持显式。
5. **基础设施使用成熟库：** 网络、异步、存储、序列化、TUI、追踪和 SQL 解析不重复造轮子。
6. **模型只提出动作：** Runtime、Policy 和 Workflow 决定动作是否允许以及任务是否完成。
7. **先持久化再执行副作用：** 可恢复性、幂等和审计优先于便利。
8. **Context 按需获取：** 完整 Data Context 不等于 Prompt；每次模型调用只获得预算内 ContextPack。
9. **事实、推断、契约分离：** LLM 推断不能伪装成数据库事实或正式业务规则。
10. **权限不放大：** Agent 的有效权限不能高于当前用户与 Workspace Policy 的交集。
11. **专业复杂度由系统吸收：** 普通用户用业务语言表达目标和确认关键选择，不需要理解 SQL、数据模型、dbt 或调度器。
12. **控制平面与数据平面分离：** 托管控制状态、策略和编排不要求复制客户原始业务数据；实际数据处理优先留在客户授权的数据平面。
13. **按风险分级自治：** 只读和低风险动作可以在明确 Policy 内自动执行；代码变更先在隔离环境验证；生产写入、部署、删除和高成本操作必须获得明确批准。
14. **按客户成熟度渐进启用：** 先通过受支持的窄场景证明可靠性，再降低对 Data Engineer、Metric Contract 和现有数据平台的前置要求。

## 6. 总体架构

~~~text
┌──────────────────────────── Product Interfaces ─────────────────────────────┐
│ TUI │ CLI Commands │ Web Client │ HTTP Adapter │ Event Source Adapters     │
└──────────────────────────────────┬───────────────────────────────────────────┘
                                   │
                         AgentService Interface
                                   │
┌───────────────────────────── Control Runtime ────────────────────────────────┐
│ Harness / Run Supervisor                                                    │
│ ├── Coordinator                                                             │
│ ├── Query │ Analysis │ Build/Change │ Operate │ ML Data Prep Workflows     │
│ ├── Agent Loop │ Context Resolver │ Policy / Approval                      │
│ ├── Completion Gates │ Durable Execution                                   │
│ └── Event / Snapshot / Artifact coordination                                │
└──────────────┬──────────────────────┬───────────────────────┬────────────────┘
               │                      │                       │
       deterministic calls       ModelProvider       model/effectful actions
               │              OpenAI-compatible first          │
               │                                              ▼
               │                                      Tool Runtime
               │                                ToolCatalog → ToolView
               │                                              │
               └──────────────────┐                      Tool Handlers
                                  ▼                            │
┌────────────────────────────── Domain Modules ────────────────────────────────┐
│ Semantic & Metric          │ Metadata / Lineage / Freshness                 │
│ Query Planning & Verification │ Data Quality & Validation                   │
│ Artifact / Change / Impact │ Operations Diagnostics / Health                │
│ Analysis / Data Processing                                                │
└──────────────────────────────────┬───────────────────────────────────────────┘
                                   │ Ports
┌──────────────────────────── Ports and Adapters ──────────────────────────────┐
│ SQL / Catalog / Semantic / Lineage / Artifact / Job / Test Interfaces      │
│ Warehouse │ dbt/Git │ Airflow/Dagster │ Spark/Python │ Store Adapters      │
└──────────────────────────────────┬───────────────────────────────────────────┘
                                   │
        Warehouse / dbt / Git / Airflow / Dagster / Spark / Object Storage

┌────────────────────── Shared Knowledge and Quality Planes ──────────────────┐
│ Data Context / Memory / ContextRepository / Agent Context Lakehouse        │
│ Run Events / Telemetry / Eval Records / Eval Runner / Release Gates        │
└──────────────────────────────────────────────────────────────────────────────┘
~~~

架构有两条正交轴：

- Workflow 表达用户目标、阶段、状态转换、Artifact 和完成条件；
- Domain Module 提供可被多个 Workflow 复用的确定性数据领域规则。

Workspace Bootstrap 是后续版本的产品 Onboarding 与治理建立流程，不是第六种业务 Workflow，也不拥有独立 Harness。Starter Data Stack 的准备与变更复用 Build/Change 的隔离、验证和审批能力，日常健康与恢复复用 Operate；平台 Provision 的具体差异留在 capability-based Adapter。

不得按照 Query、Pipeline、Ops 等角色再建立一组拥有自己 Loop 和状态的 Service。它们要么是 Workflow，要么拆成跨 Workflow 复用的深 Domain Module。Eval 是独立质量平面，Data Context 是共享知识平面；二者都不是某一种业务 Workflow 的下游 Service。

Context Resolver 是 Control Runtime 消费 Data Context 的统一入口；Eval Runner 通过版本化 Event、Artifact、ContextManifest 和 EvalRecord 评估系统。共享知识与质量平面不位于 Tool Runtime 的线性调用链中。

### 6.1 长期部署拓扑

长期默认采用“YS 托管控制平面 + 客户环境数据平面”的混合部署：

~~~text
┌──────────────────────── YS Managed Control Plane ──────────────────────────┐
│ Identity / Workspace / Policy / Task / Workflow / Model / Eval / Telemetry│
└──────────────────────────────────┬───────────────────────────────────────────┘
                                   │ scoped commands, metadata and references
                                   │
┌──────────────────────── Customer Data Plane ───────────────────────────────┐
│ Database / Warehouse / Object Storage / dbt / Dagster / Execution Identity│
│ Raw and restricted business data remains here unless policy allows export  │
└──────────────────────────────────────────────────────────────────────────────┘
~~~

原始业务数据默认留在客户数据库、客户云账号或明确授权的 Starter Data Stack 数据平面。控制平面只接收完成任务所需、经过 Policy 允许的 Metadata、脱敏 Preview、Artifact 引用和派生结果。客户可以选择本地控制平面、YS 托管控制平面或满足相同 Interface 的其他部署 Adapter。

微型公司的 Starter Data Stack 可以由 YS 托管，但仍保持 Runtime State、客户业务数据、Artifact 和 Context Projection 的职责分离。部署位置不能改变权限不放大、敏感数据出境和可审计性等架构不变量。

## 7. 核心领域模型

### 7.1 Workspace

Workspace 是治理和资源隔离边界，包含：

- 业务数据责任人、成员与角色；
- 客户数据成熟度、Bring Your Own Stack 或 Starter Data Stack Profile；
- 数据源和代码仓库连接；
- Metric Registry 或外部 Semantic Provider；
- 默认时区、数据新鲜度规则和输出约定；
- Tool Policy、审批策略和资源 ACL；
- Model Provider 配置；
- Runtime Store 与 Artifact Store 配置。

所有 Session、Task、Run、Artifact 和 Context Evidence 都属于一个 Workspace。

### 7.2 Session

Session 只管理一次交互上下文：

- 用户消息；
- 当前聚焦 Task；
- TUI 展示状态；
- Session 级显式偏好。

Session 不拥有长期 Task 生命周期。关闭 Session 或使用 /new 创建新 Session，不得删除、取消或隐式完成正在运行的 Task。

推荐命令语义：

| 命令 | 含义 |
|---|---|
| /new | 创建新 Session，不影响已有 Task |
| /tasks | 查看 Workspace Task |
| /task new | 在当前 Session 显式创建 Task |
| /task switch ID | 切换当前 Task |
| /resume ID | 在当前 Session 恢复 Task |
| /cancel ID | 显式请求取消 Task 或 Run |

### 7.3 Task

Task 表示稳定的用户目标、验收条件和权限边界。一个 Task 可以：

- 跨多个 Session；
- 包含多个 Run；
- 暂停并恢复；
- 创建子 Task；
- 引用多个 Artifact；
- 等待用户输入、审批或外部执行。

Task 不因为模型调用失败而消失。Task 的目标或关键权限发生变化时，应更新 Task 契约或创建新的 Task，而不是静默修改当前 Run。

### 7.4 Run

Run 是完成一个 Task 的一次可审计执行尝试。

以下情况恢复同一个 Run：

- CLI 或 Runtime 重启；
- 等待澄清或审批；
- 等待外部 Execution；
- LLM 限流；
- 可安全重试的瞬时网络错误。

以下情况创建新 Run：

- 原 Run 已进入 Failed 终态；
- 用户修改目标或验收条件；
- 权限范围发生关键变化；
- 代码、模型、Context 或执行环境发生影响结果的关键变化；
- 用户要求采用不同方案重新执行。

新 Run 通过 retry_of_run_id 指向上一尝试。

### 7.5 Step、ToolCall 和 Execution

Step 是一次模型决策或确定性 Workflow 动作。ToolCall 是 Step 提出的能力调用。Execution 是实际执行单位。

一个 ToolCall 可能：

- 立即返回成功；
- 被 Policy 拒绝；
- 等待审批；
- 提交一个长时间 Execution；
- 进入状态不确定；
- 产生一个或多个 Artifact。

这三个概念不能合并，否则无法清晰表达重试、审批和远端任务恢复。

### 7.6 Artifact

Artifact 是 Task 的可复用、可审计产物。至少包括：

- QueryArtifact；
- QueryPlan；
- QueryPreflight；
- VerificationReport；
- AnalysisReport；
- DashboardArtifact；
- ChangeSet；
- ImpactReport；
- ExecutionLog；
- DatasetArtifact；
- ContextSummary；
- TaskHandoff。

大型结果保存在 Artifact Store，Event 只保存引用、Hash、类型、Owner、ACL 和敏感级别。

## 8. Coordinator 与 Workflow

### 8.1 单一入口

产品只有一个主入口。普通用户直接描述目标，不需要选择 Query Agent、Analysis Agent 或 Build Agent。

Coordinator 将输入分类为：

- ContinueCurrentTask；
- CreateNewTask；
- CreateChildTask；
- CreateChangeRequest；
- RequestClarification。

Data Engineer 可以使用手动 Mode Override。普通业务用户不显示 Build/Operate 等无权限模式。

### 8.2 Workflow 切换

同一个 Task 内只允许改变当前 Workflow 的阶段、工具策略或执行方案，前提是 Task 目标、主要 Artifact、Completion Contract 和权限边界不变。

跨 Workflow 默认创建子 Task，因为 Workflow 变化通常意味着产物、完成条件或风险边界变化。Query 结果不足以回答“为什么”时，Query Task 交付已有 QueryArtifact，再通过 TaskHandoff 创建 Analysis 子 Task；不得在原 Run 中静默改写验收条件。

v0.2 尚未实现 Analysis、Build/Change、Operate 和 ML Data Prep。Coordinator 对这些请求只返回 `UnsupportedCapability`。结构化 TaskHandoff 和子 Task 自动创建从相应 Workflow 版本开始实现。

如果进入 Build/Change 后产物、权限和风险边界发生变化，则创建子 Task：

~~~text
Analysis Task
    │
    ├── 普通用户：创建 ChangeRequest，等待 Data Engineer 接收
    │
    └── 有 change.prepare 权限：创建 Build/Change 子 Task
~~~

### 8.3 TaskHandoff

父子 Task 不共享可变 Working Memory，也不复制完整聊天历史。它们通过结构化 Handoff 传递：

- 目标与验收条件；
- 已确认事实；
- Evidence 和 Artifact 引用；
- 未解决问题；
- 显式假设；
- 所需权限。

## 9. Harness 与 Run Supervisor

Harness 是所有 Workflow 共用的控制内核，负责：

- 创建、加载和恢复 Run；
- deadline、step、token 和 cost 预算；
- 取消、暂停与恢复；
- Context 预算；
- ToolView 生成；
- Policy 和 Approval 调用；
- Event 追加与 Snapshot 更新；
- Artifact 注册；
- Workflow 调度；
- Execution 唤醒；
- Completion Gate 调用；
- Telemetry 发射。

Harness 不负责：

- 编写 SQL；
- 解释指标；
- 生成 dbt 代码；
- 判断具体 Pipeline 是否健康。

这些属于 Workflow 和 Domain Module。

## 10. Agent Loop

Agent Loop 保持显式，不隐藏在通用 Agent Framework 中。

~~~text
加载 Run Snapshot
→ 构建 ContextManifest 与 ToolView
→ 调用 ModelProvider
→ 得到 AgentAction
→ Workflow 校验状态转换
→ Tool Runtime 执行动作
→ 记录 Observation / Artifact / Event
→ 检查预算和 Completion Gate
→ 继续、等待、完成或失败
~~~

模型允许提出的动作：

- CallTool；
- 提交 Workflow-specific typed proposal；v0.2 仅有 ProposeQueryPlan；
- RequestCapability；
- RequestClarification；
- ProposeCompletion。

模型不能：

- 直接更改 Run 状态；
- 自行解锁 Tool；
- 绕过审批；
- 宣布任务已经完成；
- 将推断写成正式 Memory 或 Data Contract。

## 11. Completion Gate

ProposeCompletion 只是模型建议。Workflow 使用可编码、可测试的条件决定是否完成。

### Query

所有 QueryIntent 的共同条件：

- 结果来自实际 Tool 执行或确定性的授权元数据读取；
- 数据来源、查询范围、PolicyDecision 和敏感级别已记录；
- QueryVerifier 的硬性检查通过；
- VerificationReport 与 QueryArtifact 已持久化。

GovernedMetric 还要求 Active Metric id/version、维度、时间范围和新鲜度状态明确。AdHocRead 必须标记 `semantic_status = inferred` 并披露假设。Metadata 不得伪造 Metric Contract 或业务结论。

### 11.1 澄清、警告与阻断矩阵

Query Workflow 不允许静默猜测影响口径、权限、成本或结果解释的关键条件：

| 情况 | 行为 |
|---|---|
| 指标有多个候选、时间范围或时区不明确、关键维度含义不确定 | 必须 RequestClarification，Run 进入 WaitingForInput |
| AdHoc 语义未经确认、结果为空或截断、数据过期但用户未要求实时 | 可以完成，但 QueryArtifact 必须显示结构化 Warning |
| Active Contract 与生产实现冲突 | 披露冲突；未获得用户或 Owner 选择时不得声称正式口径正确 |
| 无授权数据源、非 Active 指标、SQL 不安全、预算或敏感策略超限 | 拒绝执行或阻断完成 |
| Freshness 无法确认且问题明确要求“当前”“最新”或 SLA 内数据 | 阻断完成或请求用户接受已知限制 |

空结果不得自动解释为业务值为零。系统必须区分 `empty_result`、`all_null_result`、`source_unavailable` 和 `freshness_unknown`。

### Analysis

- 每个主要结论有 Evidence；
- 查询或分析可复现；
- 假设和不确定性已披露；
- Analysis Artifact 已生成。

### Build/Change

- ChangeSet 可审查；
- 测试已运行并记录；
- 影响范围已说明；
- 未执行未经批准的 Merge、Deploy 或生产写入。

### Operate

- 根因判断有诊断证据；
- 恢复动作符合 Policy；
- 恢复后完成健康验证。

### ML Data Prep

- 清洗与特征定义可复现；
- 数据集有版本；
- 质量和泄漏检查完成；
- 血缘可追踪。

## 12. Tool Runtime

### 12.1 分层

~~~text
Model-visible Tool
→ Tool Runtime
→ Tool Handler
→ Domain Module + Port
→ Adapter / Execution Backend
→ 外部数据平台

Workflow / Completion Gate
→ deterministic Domain Module
~~~

- Tool 表达稳定的领域意图；
- Tool Runtime 负责参数校验、权限、审批、超时、事件和结果归一化；
- Tool Handler 把模型可见意图映射为 Domain Module 调用和 Port 操作；
- Domain Module 负责可编码、可测试的语义、查询、质量、变更、影响和验证规则；
- Port 表达 Runtime 需要的外部能力，Adapter 负责具体平台协议和方言；
- Execution Backend 负责实际运行，长任务通过 ExecutionHandle 脱离 Agent Loop。

Workflow 和 Completion Gate 可以直接调用无外部副作用的确定性 Domain Module。模型提出的动作以及任何外部读取、写入、提交和取消必须经过 Tool Runtime 或 Execution Control Plane；Workflow 不得直接调用 Adapter。Port 和 Adapter 默认不直接暴露给模型。

### 12.2 ToolCatalog 与 ToolView

ToolCatalog 是 Runtime 内部完整能力目录。每个 Step 只向模型暴露最小 ToolView。

ToolView 由以下条件共同决定：

~~~text
Task 类型
+ Workflow 当前阶段
+ 用户权限
+ Connector 可用能力
+ 工具风险
+ 当前 Run 状态
= 当前模型可见工具
~~~

模型可以 RequestCapability，但不能自行解锁。Harness 决定扩展 ToolView、切换 Workflow、请求审批或拒绝。

### 12.3 Tool 协议

ToolSpec 至少声明：

- 输入与输出 Schema；
- risk_level；
- side_effect；
- idempotency；
- timeout；
- required_permissions；
- sensitivity；
- 版本。

ToolOutcome 至少包括：

- Succeeded；
- Failed；
- Rejected；
- ApprovalRequired；
- Running；
- Indeterminate。

Runtime 只自动重试相同参数的安全瞬时故障。修改参数后的重试由 Agent Loop 决定。副作用状态不确定时禁止盲目重试。

### 12.4 v0.2 Query 安全契约

只读不是单一布尔标记。v0.2 必须同时执行以下防线：

1. SqlReadOnlyPolicy 只接受单条、受支持方言的 `Statement::Query`；拒绝多语句、控制语句和策略禁止的函数。
2. Connector 使用数据库侧最小权限身份，并为每次 Postgres 查询开启 read-only transaction。
3. Workspace Policy 可以限制 Source、Schema、Relation、Column 和敏感级别；模型不能扩大 allowlist。
4. QueryBudget 明确 max_sql_bytes、statement_timeout、acquire_timeout、max_rows、max_result_bytes、max_concurrency，以及可用时的 max_estimated_cost 或 max_scanned_bytes。
5. 支持的 Connector 应在执行前使用 EXPLAIN、Dry Run 或平台等价能力做成本预检；不支持预检时必须采用更保守的静态和运行时限制。
6. 每次远端查询记录 query_tag 和可用的 external_query_id，以支持取消、审计和恢复判断。
7. Unknown 只读调用不得因“无写副作用”自动视为零风险。超过低成本阈值的重试必须等待用户确认，并生成新的 ToolCallId。
8. QueryResult 在进入模型、TUI 和 Artifact Store 前执行字段级敏感策略、Preview 限制和结果大小限制。

Policy 拒绝、预算超限、认证失败、查询取消、方言错误、Schema 变化和暂时性网络错误必须使用 typed error category，不依赖字符串匹配。

## 13. 权限、身份与审批

### 13.1 能力权限

Core 使用能力权限而不是硬编码职位名称：

- data.query；
- data.analyze；
- change.request；
- change.prepare；
- change.review；
- change.merge；
- production.execute。

DataEngineer 是 Workspace 提供的一组默认技术授权。业务数据责任人拥有确认业务契约和批准策略允许动作的责任，但不因此自动获得 `change.prepare`、`change.merge` 或 `production.execute`；批准权与实际执行权限保持分离。

### 13.2 有效权限

一次动作的有效权限为：

~~~text
用户身份权限
∩ Workspace Policy
∩ Task 权限范围
∩ Connector 身份权限
∩ Tool 风险策略
~~~

后台元数据索引身份与用户查询身份必须分离。支持身份代理的平台使用用户 OAuth、临时凭证或 impersonation；不支持的平台映射到受限 Role。

### 13.3 审批

审批绑定不可变动作 Hash，而不是 Session、Agent 或工具名称。审批记录至少包含：

- 工具和版本；
- 目标环境与资源；
- 完整参数；
- 代码或 Artifact 版本；
- 预估影响；
- 幂等键；
- 风险等级；
- 有效期；
- action_hash；
- 申请人、审批人和实际执行 Principal。

关键字段变化后必须重新审批。

### 13.4 Secret、敏感数据与模型出境

CredentialReference 与 Secret 值必须分离。可序列化的 Core 类型、Event、Artifact、Telemetry、ContextManifest 和 Prompt 只能保存 CredentialReference，不能保存 API Key、密码、Token 或完整 DSN。

Tool Runtime 负责统一脱敏 Tool 参数、Connector 错误和结果 Preview。Tracing 默认禁止记录 Prompt body、QueryResult row 和完整数据库错误。测试必须使用 canary secret 证明这些值不会进入持久化或 Telemetry。

每个 QueryArtifact 和 QueryResult Artifact 必须记录 sensitivity、owner、ACL、retention_policy 和 expires_at。默认本地文件权限仅允许当前用户访问；过期清理由显式命令或受治理的维护流程执行。

Workspace Policy 决定哪些字段可以进入外部 Model Provider。不能发送的字段只允许进入确定性 Tool 与本地 Artifact；模型只获得脱敏摘要。来自 dbt docs、数据库文本和历史 Artifact 的内容一律视为不可信数据，不能覆盖 System 指令或被解释为 Tool 调用。

### 13.5 风险分级自治

系统按动作的副作用、环境、数据敏感性、可逆性、成本和影响范围决定自治级别，而不是要求非技术用户理解底层命令：

| 动作类型 | 默认行为 |
|---|---|
| 授权范围内的低成本只读查询、Metadata 读取和确定性验证 | 自动执行并保留审计记录 |
| 可逆的低风险配置建议和日常维护 | 在 Workspace 预授权范围内自动执行，超出范围时请求确认 |
| 代码、模型或 Pipeline 变更 | 只在隔离环境准备和测试，交付 Diff、测试与影响报告 |
| 生产写入、Merge、Deploy、删除、高成本或不可逆动作 | 绑定不可变 action_hash，由业务数据责任人或策略指定的审批人明确批准 |
| 无法可靠判断影响、权限或执行状态 | Fail closed，进入澄清、Reconcile 或人工接管 |

批准界面必须使用业务影响、成本、数据范围、可逆性和失败处理解释动作，不得只展示 SQL、Git 命令或基础设施术语。低风险预授权必须有明确 Scope、预算、有效期和撤销机制。

## 14. Data Context

### 14.1 定位

Data Context 是受治理的数据知识与检索平面，不是所有业务数据的物理 Source of Truth，也不是一个简单向量数据库。

~~~text
Warehouse / dbt / Git / Orchestrator / Docs / Historical Tasks
                         │
                  Context Adapters
                         │
             Normalized Context Evidence
                         │
            Metadata Store + Search Index
                         │
                  Context Resolver
                         │
              Budgeted ContextPack
~~~

Warehouse、Git、dbt 和 Orchestrator 继续作为各自事实的权威来源。索引是可重建投影。容易变化的信息在关键决策前进行现场核实。

每条 Evidence 包含：

- source；
- source_type；
- version 或 content_hash；
- observed_at；
- freshness；
- owner；
- ACL；
- sensitivity；
- confidence；
- token_cost。

### 14.2 Schema 三种状态

- ObservedSchema：Connector 从真实系统确定性读取；
- InferredAnnotation：Agent 根据代码、查询和样本提出；
- ConfirmedContract：负责人确认或测试验证后的正式契约。

Agent 不能自动将 Inferred 提升为 Confirmed。数据采样必须受行数、字段敏感性、脱敏和权限策略约束。

### 14.3 ContextManifest

每次模型调用生成不可变 ContextManifest，记录：

- Task 目标引用；
- Workflow 状态；
- 纳入的 Evidence 和 Summary；
- ToolView 版本；
- token 预算；
- 被省略内容及原因。

完整 Event 和 Artifact 不直接进入 Prompt。压缩摘要是可重建投影，不能覆盖原始审计记录。

该设计延续 OpenAI 内部 Data Agent 的核心经验：离线聚合与索引组织知识，查询时只检索相关上下文，缺失或易过期信息通过实时工具核实，并控制模型可见工具数量。参考：https://openai.com/index/inside-our-in-house-data-agent/

### 14.4 物理存储职责

Data Context 是逻辑知识与检索平面，不意味着所有相关数据进入同一个物理 Store。长期存储拓扑必须拆开控制状态、权威事实、检索投影和大型 Artifact：

| 数据类型 | 权威来源 | 物理存储 | Lance 的角色 |
|---|---|---|---|
| Task、Run、Event、Approval、Checkpoint | Agent Runtime | 本地 SQLite；共享 Runtime 使用 PostgreSQL | 不使用 |
| 业务事实数据 | Warehouse、Lakehouse、Database | 用户现有数据平台 | 不复制，只保存引用、摘要和必要样本 |
| 正式指标与语义契约 | 外部语义层、Git、Metric Registry | Git 或事务数据库 | 只保存可重建的检索投影 |
| Schema、血缘、dbt 元数据和文档 | dbt、Git、Catalog、Database | Agent Context Lakehouse | 主要存储对象 |
| 已治理的 Task 经验和运维案例 | Runtime Event 与 Artifact | Agent Context Lakehouse | 主要存储对象 |
| SQL 结果、日志、报表、Diff 和大文件 | Artifact Store | 本地文件系统或对象存储 | 仅保存引用、摘要、元数据和可选 Embedding |
| Trace、Token、Latency 和 Tool Call Telemetry | Runtime 派生记录 | OpenTelemetry、Langfuse 等观测后端 | 不使用 |
| Eval 数据集 | Git、Artifact 和人工标注 | Git 或对象存储 | 可选检索投影，不是唯一副本 |

因此系统包含三个相互独立的持久化职责：

~~~text
Runtime State Store                 Agent Context Lakehouse
SQLite / PostgreSQL                 Lance / LanceDB（后续 Adapter）
Task / Run / Event                  Context Evidence / Relations
Approval / Checkpoint               Task Episodes / Procedural Memory
          │                                      │
          └────────── Artifact Reference ────────┘
                              │
                        Artifact Store
                   Filesystem / S3 / MinIO
              Result / Log / Report / Diff / File
~~~

Agent Context Lakehouse 中的数据是可重建 Projection。删除该 Store 不应丢失 Task 的权威执行状态、企业业务事实或正式语义契约；系统应能从权威来源重新同步和构建索引。

### 14.5 Lance Agent Context Lakehouse

Lance 可以作为长期 Agent Context Lakehouse 的物理实现，但不能替代 Runtime State Store、企业 Warehouse、正式语义层或 Artifact Store。

本节是长期候选设计，不是 v0.2 的实现要求。启动 Spike 前应将具体 Dataset、格式版本、Compaction、迁移和恢复策略提取为独立 ADR，并以当时 SDK 与 Eval 结果重新确认，不能直接把本节字段表当作代码生成清单。

Lance 是 Arrow-native 的列式文件与表格式，适合对象存储随机访问、结构化元数据、全文检索和向量检索；LanceDB 在 Lance 之上提供更直接的嵌入式检索能力。Rust 实现初期优先评估 LanceDB Rust SDK，并始终通过 ContextRepository Seam 隔离具体 SDK。参考：[Lance 文件格式](https://lance.org/format/file/)、[Lance 索引格式](https://lance.org/format/index/)和 [LanceDB Quickstart](https://docs.lancedb.com/quickstart)。

建议按照更新模式、检索模式、ACL 和敏感级别拆分 Dataset，而不是建立一张万能 Context 表：

| Dataset | 内容 | 关键字段 |
|---|---|---|
| context_evidence | Schema、Metric 投影、dbt 节点、代码与文档片段、质量规则和确认结论 | evidence_id、workspace_id、entity_type、source_uri、source_version、observed_at、knowledge_state、content_hash、ACL、sensitivity、embedding_version |
| context_relations | Evidence 之间可追溯的轻量关系 | from_id、relation_type、to_id、source_version、confidence、evidence_refs |
| task_episodes | 经压缩、验证并允许复用的历史任务经验 | task_id、task_type、summary、outcome、artifact_refs、evidence_refs、approved_for_reuse |
| procedural_memory | 已批准的 Runbook、操作 Playbook 和开发惯例 | playbook_id、scope、preconditions、procedure_ref、owner、status、version、evidence_refs |
| eval_cases | 可选的检索与 Context Eval 投影 | case_id、input_ref、expected_evidence、dataset_version、artifact_refs |

约束如下：

1. 原始对话不能默认进入长期 Memory；只有带 Evidence、Scope 和治理状态的摘要可以成为 Task Episode。
2. 完整敏感查询结果、大型日志和二进制 Artifact 不进入 Lance Dataset，只存 ArtifactReference、Hash、Preview 和必要索引字段。
3. 每条 Evidence 必须带 workspace_id、ACL、sensitivity、source_version、observed_at 和 content_hash。
4. Embedding 必须记录模型、版本和维度；更新模型时建立新索引或显式迁移，不能静默覆盖。
5. Lance 存储格式必须锁定明确的稳定版本，升级前执行兼容性和恢复测试。官方标记为不稳定的格式版本不得用于生产，参考：[Lance 格式版本说明](https://lance.org/format/file/versioning/)。
6. Lance 虽然提供 MVCC 和 ACID 表事务，但 Runtime 的高频状态更新、唯一约束、Lease、锁、幂等键和审批状态机仍由 SQLite/PostgreSQL 负责，参考：[Lance Transaction 规范](https://lance.org/format/table/transaction/)。
7. 索引、摘要和 Relation 全部是可重建 Projection，不能覆盖或反向篡改权威来源。

### 14.6 ContextRepository Seam 与检索流程

Core 定义与存储实现无关的 ContextRepository Interface：

~~~rust
#[async_trait]
pub trait ContextRepository {
    async fn ingest(
        &self,
        evidence: Vec<ContextEvidence>,
    ) -> Result<IngestReport, ContextError>;

    async fn retrieve(
        &self,
        query: ContextQuery,
    ) -> Result<Vec<ContextCandidate>, ContextError>;

    async fn get(
        &self,
        id: &EvidenceId,
    ) -> Result<Option<ContextEvidence>, ContextError>;

    async fn invalidate(
        &self,
        source: &SourceRef,
    ) -> Result<InvalidationReport, ContextError>;
}
~~~

ContextRepository Interface 包含查询语义、隔离要求、失效规则和错误模式，而不仅是 Rust method。Adapter 规划为：

- InMemoryQueryContextProvider：v0.2 测试和 Eval；
- FileMetricRegistry 与 DbtManifestAdapter：v0.2 的确定性 Query Context Provider；
- FileContextRepository：出现跨来源同步、失效和通用 Evidence 检索需求后的第一个 Repository 实现；
- LanceContextRepository：规模和检索 Eval 证明需要后引入；
- RemoteContextRepository：未来共享 Runtime 可选实现。

Agent Loop 不直接调用 ContextRepository，也不能依赖 Lance、Arrow RecordBatch 或 Embedding SDK 类型。ContextResolver 是面向 Harness 的深 Module，负责在这一 Seam 后隐藏检索、去重、关系展开、重排、新鲜度和 token 预算：

~~~text
Task Intent
→ Workspace、Identity、Workflow 和当前 Step
→ ACL、Sensitivity、Entity 和 Freshness 过滤
→ Scalar + Full-text + Vector 混合召回
→ Relation Expansion 与 Rerank
→ Token Budget Selection
→ ContextPack + ContextManifest
~~~

ACL 和 Workspace 过滤必须在召回时生效，禁止先跨租户读取候选再在 Prompt 前过滤。易变化或高风险 Evidence 在关键决策前必须通过 Connector 回源验证。ContextPack 只包含任务所需内容，ContextManifest 保留使用、遗漏和版本证据。

Context 有两条不同路径：

1. ContextResolver 只读取已经同步、可重建且通过 ACL 的 Context Projection；
2. Schema、Freshness、权限和其他易变化事实的实时回源必须经过 Tool Runtime，结果作为当前 Run 的新 Evidence 持久化。

ContextRepository 不得在 `retrieve` 中隐式发起外部 I/O。v0.2 的 Query Context Provider 只支持 exact match、scalar filter 和确定性排序，不提前实现通用 ingest/invalidate Repository；全文、向量、Relation Expansion 与 Rerank 只是长期语义。

所有检索内容都携带 `instruction_trust = untrusted_data`。Context Resolver 和 Prompt Builder 必须保持指令与 Evidence 的结构化分隔，并在 Eval 中覆盖 Context/Prompt Injection 样例。

## 15. 语义层与 Metric Registry

YS Data Agent 必须依赖受治理的语义契约，但不要求客户预先部署特定语义产品。

统一 SemanticProvider 能力：

- resolve_metric；
- list_dimensions；
- validate_metric；
- compile_metric_query；
- explain_metric。

后端可以是 MetricFlow、Cube、Looker 或 YS Native Metric Registry。

原生 Registry 仅负责最小闭环：

- 指标绑定明确数据模型；
- 显式聚合表达式；
- 允许的维度和时间字段；
- Owner、版本、状态和验证规则；
- Draft、Active、Deprecated 生命周期；
- 有限范围的查询编译和验证。

复杂 Join、通用语义 SQL、缓存和预聚合不属于原生 Registry 的早期目标。Agent 可以提出 Draft Metric，只有授权 Data Engineer 或 Data Owner 可以发布为 Active。

语义层是业务期望的权威来源，但不是不可出错。正式定义与生产实现冲突时：

- Query 可以按当前实际实现给出结果；
- 必须披露口径冲突；
- Agent 不能替公司决定业务规则；
- 修改通过 ChangeRequest 和 Build/Change Task 完成。

## 16. Domain Modules、Ports 与 Adapters

### 16.1 Workflow 与 Domain Module 的职责

Workflow 和 Domain Module 不能按照一一对应关系设计：

| 概念 | 拥有什么 | 不拥有什么 |
|---|---|---|
| Workflow | Task 阶段、状态转换、所需能力、Artifact 目标和 Completion Contract | 独立 Harness、独立 Event 体系、具体平台 SDK |
| Domain Module | 可复用领域规则、Typed Input/Result、确定性校验和错误语义 | Session、Task 生命周期、Agent Loop、模型对话 |
| Tool Handler | 模型可见 ToolIntent 到 Domain Module 与 Port 的受治理映射 | 领域规则副本、长期状态 |
| Port | Runtime 所需的外部能力及其不变量、错误和性能语义 | 具体厂商协议 |
| Adapter | 某个平台对 Port 的具体实现 | Workflow 状态和业务完成判断 |

Domain Module 使用删除测试判断是否值得存在：删除后，如果复杂规则会散落到多个 Workflow、Tool Handler 和测试中，它是有深度的 Module；如果删除后只少一层转发，它不应存在。

### 16.2 共享 Domain Modules

| Domain Module | 责任 | 主要调用者 | 明确不负责 |
|---|---|---|---|
| Semantic & Metric | 解析、验证、编译和解释 Metric Contract | Query、Analysis、Build/Change | 未经授权发布正式指标 |
| Metadata / Lineage / Freshness | 规范化 Schema、Owner、血缘和新鲜度 Evidence | 所有 Workflow、Context Resolver | 取代 Catalog 或成为永久事实源 |
| Query Planning & Verification | 形成查询计划、AST/方言校验、范围与结果验证 | Query、Analysis、Operate、ML Data Prep | 直接持有 Warehouse 连接和执行查询 |
| Data Quality & Validation | 执行质量规则、泄漏检查和健康判定 | Query、Build/Change、Operate、ML Data Prep | 让 LLM 自己定义并通过硬性规则 |
| Artifact / Change / Impact | 构建 Typed Artifact、ChangeSet、Diff 和影响分析 | Analysis、Build/Change、Operate | 未经审批 Merge、Deploy 或生产写入 |
| Operations Diagnostics / Health | 规范化诊断证据、验证恢复计划和执行后健康状态 | Build/Change、Operate、ML Data Prep | 持久化调度 ExecutionHandle 或直接控制外部任务 |
| Analysis / Data Processing | 可复现的统计、清洗、特征和可视化计算 | Analysis、ML Data Prep | 绕过数据权限或把临时推断发布为契约 |

这些是逻辑 Module，不要求每个 Module 对应独立 crate、进程或网络 Service。v0.2 只实现 Query 垂直切片真正需要的部分，不为未来能力创建空 Trait、空目录或透传 Facade。

### 16.3 Capability-based Ports

不定义巨型 DataConnector。使用 capability-based Ports：

| Port | 责任 |
|---|---|
| CatalogReader | 表、列、约束和分区元数据 |
| SqlQueryExecutor | 只读 SQL 执行 |
| DataSampler | 受控采样 |
| MutationExecutor | 写入或 DDL |
| LineageReader | 血缘 |
| FreshnessReader | 新鲜度 |
| JobController | 查询、取消和重跑任务 |
| ArtifactRepository | 读取和提交代码 Artifact |
| TestRunner | 执行数据或项目测试 |

Adapter 只实现真实支持的能力，并公开 CapabilityDescriptor。Workflow 在规划前即可知道环境支持什么。外部 I/O 由 Tool Handler 通过 Port 发起；Domain Module 不依赖具体 Adapter 类型。

### 16.4 原生 Artifact，而非通用 Pipeline DSL

PipelineIntent 可以描述输入、输出、转换、调度、质量和验收目标，但真正产物必须是用户框架的原生代码：

- dbt SQL、YAML、tests 和 macros；
- Dagster assets、resources 和 sensors；
- Airflow DAG 和 operators；
- Spark job 与测试。

## 17. Execution Control Plane

Execution Control Plane 独占长任务的持久化提交、等待、取消、事件唤醒和 Reconcile 责任。Tool Handler 通过 JobController Port 操作外部任务；Operations Diagnostics / Health Domain Module 只处理诊断规则、恢复计划验证和执行后健康判断，不维护另一套执行状态。

长任务返回持久化 ExecutionHandle：

- execution_id；
- backend；
- external_job_id；
- state；
- idempotency_key；
- submitted_at；
- last_observed_at。

ExecutionState 至少包括：

- Queued；
- Running；
- Succeeded；
- Failed；
- CancelRequested；
- Cancelled；
- Unknown。

提交长任务后 Run 进入 WaitingForExecution，停止 Agent Loop，不再调用 LLM。Webhook 或事件负责快速唤醒，Reconciler 负责丢失、重复和乱序事件的最终核对。

恢复时只加载预算内 Context。完整执行日志保存在 Artifact Store。取消 Agent Run 与取消外部任务是两个独立动作和状态。

## 18. Memory

Memory 不是单一 Store：

| 类型 | 用途 |
|---|---|
| Session History | 当前交互，不自动成为长期知识 |
| Run Working Context | 当前执行的临时 ContextPack |
| Task/Run History | 审计、恢复、Artifact 和结果摘要 |
| Data Context | 指标、Schema、血缘、Owner 和契约 |
| Procedural Memory | 已验证的操作 Playbook 和开发惯例 |
| Workspace/User Preferences | 显式偏好和默认配置 |

模型只能提出 MemoryCandidate。治理流程检查 Evidence、冲突、Scope、TTL、Sensitivity 和 ACL 后，才能提升为长期 Data Context 或 Procedural Memory。

原始凭证、完整敏感查询结果、未经验证的模型解释不得自动写入长期 Memory。

## 19. 持久化与事件

### 19.1 Typed Run Events

Run Event 是执行和审计记录，使用 append-only typed events：

- RunStarted；
- StepStarted；
- ModelRequested；
- ModelResponded；
- ToolCallProposed；
- PolicyEvaluated；
- ApprovalRequested；
- ToolExecutionStarted；
- ToolExecutionSucceeded；
- ToolExecutionFailed；
- ArtifactCreated；
- RunWaiting；
- RunResumed；
- RunCompleted；
- RunFailed；
- RunCancelled。

Event 使用 schema_version，并保持向后可读。

### 19.2 Snapshot

Snapshot 是从 Event 投影出的当前状态缓存，用于快速加载。它不是独立真相。写入规则为：

~~~text
同一事务追加 Event
→ 更新 Snapshot
→ 提交
→ 异步导出 Telemetry
~~~

本架构只对 Task/Run 执行链使用事件加 Snapshot，不把整个产品做成全面 Event Sourcing 或 CQRS。

### 19.3 Artifact Store

Runtime Store 保存 Artifact Metadata。Artifact Store 保存大型内容。两者通过 artifact_id、content_hash 和 ACL 关联。

本地模式：

~~~text
.ysda/
├── runtime.db
├── artifacts/
└── context/       # v0.2 Metric/dbt 等确定性 Query Context 投影
~~~

共享模式：

~~~text
Postgres Runtime Store
+ Object Storage Artifact Store
+ Agent Context Lakehouse（达到启用条件后使用 Lance/LanceDB）
~~~

业务数据源、Agent Runtime Store、Artifact Store 和 Agent Context Lakehouse 必须保持独立职责。Metadata 通过 workspace_id、task_id、artifact_id、evidence_id、source_uri 和 content_hash 关联，不通过复制完整业务数据关联。

### 19.4 命令幂等、单写者与 Artifact 原子性

所有会创建 Session、Task、Run 或推进 Run 的 AgentService Command 必须带 command_id。Runtime Store 对 command_id 建立唯一约束，相同命令重放返回原结果，不重复创建 Run 或执行 Tool。

同一 Run 同一时刻只有一个推进者。v0.2 使用 Snapshot version 的 optimistic concurrency 保证单写者；版本冲突方重新加载 Event，不得继续使用旧状态执行外部动作。

Artifact 内容先在目标目录写入临时文件、计算 Hash、fsync 并原子 rename，再在同一 Runtime 事务中提交 ArtifactMetadata、ArtifactCreated Event 和 Snapshot 引用。恢复流程清理无 Metadata 引用的临时文件，并报告有 Metadata 但内容缺失的损坏 Artifact。

Event Subscription 使用单调递增 sequence 作为 cursor。广播丢失或客户端重连时，从持久化 Event 继续读取，不能把内存 Channel 当作权威来源。

## 20. 可观测性与 Eval

Eval 是独立质量平面，不属于 Domain Module，也不是模型可调用的 Tool。在线 Runtime 负责生成可关联的 Event、Artifact、VerificationReport 和 EvalRecord；离线 Eval Runner 负责数据集执行、版本比较和发布门禁。确定性 Verifier 可以被 Completion Gate 在线调用，但观测平台或 LLM Judge 不能成为 Task 状态和完成判断的权威来源。

### 20.1 三个数据面

| 数据面 | 目的 | 是否可采样 |
|---|---|---|
| Run Event | 恢复、状态和审计 | 不可随意采样 |
| Telemetry | 延迟、错误率、token、工具耗时 | 可以采样和聚合 |
| Eval Record | 版本对比、质量评估和发布门禁 | 按版本长期保留 |

三者通过 workspace_id、task_id、run_id、step_id、model_call_id、tool_call_id、execution_id 和 artifact_id 关联。

Langfuse、OpenTelemetry 等平台只接收派生 Telemetry，不能成为 Task/Run 权威状态数据库。观测平台故障不能阻塞 Agent。

### 20.2 Eval Contract

每类 Workflow 有独立 Eval Contract：

- Query：指标解析、SQL、结果、来源、新鲜度和成本；
- Analysis：证据、可复现性、假设和结论；
- Build：Diff、测试、影响与越权检查；
- Operate：根因、恢复安全性、重复操作和健康验证；
- ML Data Prep：复现、泄漏、质量、版本与血缘。

评分顺序：

~~~text
确定性检查
→ 实际执行对比
→ Policy 与安全检查
→ 必要时 LLM Judge
→ 高风险人工复核
~~~

Model、Prompt、Tool、Context Retriever、Workflow 和 Policy 都必须版本化。核心确定性 Eval 退化时禁止发布。

v0.2 Query Eval 必须覆盖 GovernedMetric、AdHocRead、Metadata、歧义澄清、UnsupportedCapability、空/全 Null、过期数据、成本超限、敏感字段、Context Injection、Unknown 重试和中断恢复。每个生产缺陷必须新增回归 case。

## 21. ModelProvider

Core 定义自己的 ModelRequest、ModelAction、ModelUsage 和 ModelFailure，不暴露具体厂商消息类型。

v0.2 实现：

- OpenAICompatibleProvider；
- FakeModelProvider；
- ReplayModelProvider。

OpenAI-compatible Adapter 支持配置 base_url、api_key 和 model，并要求 Tool Calling、Tool Call ID、结构化参数和多轮 Tool Result 回传。Provider 不满足能力时，在 Run 启动前拒绝。

“OpenAI-compatible”只表示通过 v0.2 明确测试的协议子集，不表示所有兼容服务都可用。v0.2 禁用并行 Tool Call、Streaming 和 Provider-specific reasoning 参数；Provider Profile 必须声明 context window、Tool Schema 上限和错误语义。未知能力 fail closed。

后续可以新增 AnthropicProvider、GeminiProvider 或 LocalProvider，不修改 Workflow。

## 22. AgentService 与产品入口

AgentService 是所有入口共用的应用 Interface。TUI/CLI 使用进程内 Adapter，Web、Mobile 和 Event Source 通过 HTTP/Event Adapter 访问同一个 Interface：

- create_session；
- create_task；
- send_message；
- start_run；
- answer_clarification；
- approve_action；
- cancel_run；
- subscribe_events；
- get_task；
- get_artifact；
- export_artifact。

Command 语义必须唯一：`send_message` 负责创建或继续一个 Task，并以 command_id 幂等地调度至多一个 Run；`start_run` 只用于显式启动已存在且可运行的 Task。重复命令不得创建第二个 Run。

`subscribe_events` 接受持久化 event sequence cursor。`get_artifact` 默认只返回 Metadata 和受 Policy 限制的 Preview，完整内容需要再次进行身份与敏感级别检查。

本地模式：

~~~text
TUI/CLI → InProcess AgentService → Runtime → SQLite + local artifacts
~~~

共享模式：

~~~text
TUI/Web/Mobile/Event Source → Remote Client → Server AgentService
                                              → Runtime + Worker
~~~

CLI/TUI 不直接创建 QueryAgent、连接数据库或调用 Model Provider。

## 23. TUI 交互设计

v0.2 首个产品入口参考 Claude Code 和 OpenCode：

- 启动欢迎页；
- Workspace、Model、数据源和权限状态；
- 最近 Task；
- 可滚动对话与事件区；
- 固定输入区；
- 当前 Task 的用户可见状态；Session、Workflow、Run 和 Step 只在诊断详情显示；
- Tool Call 折叠展示；
- 结构化澄清；
- Task 中断、恢复和切换；
- Slash Command 与命令面板。

首次启动必须先运行 Workspace Doctor：

1. 检查 Model Provider 能力且不显示 Secret；
2. 测试数据源连接、数据库侧只读权限和 Connector capability；
3. 验证 Metric Registry、可选 dbt manifest、时区和 Freshness 配置；
4. 显示阻断项、警告和具体修复动作；
5. 提供一条 Fixture 或用户数据源上的首个可信 Query 验证路径。

默认界面只突出任务目标、当前状态、是否等待用户、答案、警告和主要 Artifact。SessionId、RunId、Workflow phase、Step 和完整 Tool Event 放入诊断详情，不能要求普通用户理解内部状态模型。

Query 结果默认按“答案/表格 → 范围与单位 → 新鲜度与警告 → SQL 与 Evidence”的顺序显示。v0.2 至少支持将允许导出的 QueryArtifact 转为 JSON、CSV 或 Markdown；敏感策略禁止导出时必须明确说明原因。

普通用户不在首页选择 Agent。Coordinator 自动识别 Workflow。只有具备权限的 Data Engineer 可以手动 Mode Override。

TUI 通过 AgentService 和 Event Stream 驱动，不包含 Workflow 业务逻辑。

## 24. Rust 与 Python 边界

Rust 长期拥有：

- Harness 与 Agent Loop；
- Task/Run/Event 状态；
- Tool Runtime、Policy 和 Approval；
- Connector 调度；
- Context 预算；
- TUI、Server 和 Worker 控制；
- Eval 运行与 Telemetry。

Python 作为可选 Capability Worker，用于：

- Polars/Pandas 数据处理；
- 统计分析；
- 特征工程；
- Python 原生数据生态集成。

Rust 与 Python 通过版本化消息协议和 Artifact 引用通信。控制消息不传输大型数据；大型数据使用 Arrow IPC、Parquet 或对象存储引用。v0.2 不实现 Python Worker。

## 25. Rust 仓库结构

采用一个 Cargo Workspace 的模块化单体：

~~~text
ys-data-agent/
├── Cargo.toml
├── crates/
│   ├── ys-agent-core/
│   │   └── Task、Run、Event、Tool、Artifact、Policy、Domain Types、Ports
│   ├── ys-agent-runtime/
│   │   └── AgentService、Harness、Loop、Workflow、Domain Modules、Context、Verifier
│   ├── ys-agent-store/
│   │   └── SQLite/Postgres Runtime Store 与 Artifact Metadata
│   └── ys-agent-adapters/
│       └── Model、Data、dbt、Git、Context Repository、Telemetry Adapter
├── apps/
│   └── ysda/
│       └── CLI/TUI、配置和依赖装配
├── evals/
├── fixtures/
└── docs/
~~~

初期仍然可以编译为一个 ysda 可执行文件。Server 和 Worker 在真正需要共享 Runtime 与长任务时再新增。

Crate 拆分依据是稳定依赖边界，不是架构图中出现的名词数量。Domain Module 不等于 crate 或进程。Memory、Policy、Eval 和 Context Resolver 等在足够复杂前保持为 Runtime 内部 Module。File/Lance 等 ContextRepository Adapter 保持在 ys-agent-adapters；只有当某个实现形成独立发布、依赖或测试节奏时，才评估拆出专用 crate。

依赖方向：

~~~text
ys-agent-runtime ───┐
ys-agent-store ─────┼──→ ys-agent-core
ys-agent-adapters ──┘

apps/ysda ──→ runtime + store + adapters + core
~~~

Runtime、Store 和 Adapters 互不直接形成循环依赖，由 apps/ysda 负责装配。Core 不依赖 Axum、SQLx、具体 LLM SDK、Warehouse SDK 或 TUI。

## 26. v0.2：Trustworthy Query Runtime

### 26.1 版本目标

为 Data Engineer 和技术型分析师交付一个本地 Trustworthy Query Pilot，证明“首次配置 → 受治理 Query → 安全执行 → 确定性验证 → 可审计交付 → 中断恢复”的完整闭环。

公共 Runtime 只实现 Query 真正使用的能力。v0.2 的成功不以未来类型数量衡量，而以用户能否更快、更安全地获得可信答案衡量。

### 26.2 首个用户、前置条件与支持场景

直接用户、Workspace 前置条件和产品承诺遵循 §2.3。Pilot 至少覆盖：

- Active Metric 的时间范围与维度查询；
- 受限 AdHocRead；
- Schema、Owner 和 Freshness 等 Metadata；
- 关键歧义澄清、不可支持能力提示和可信拒答；
- 中断后继续同一个等待中 Run；
- QueryArtifact 的安全 Preview 与 JSON/CSV/Markdown 导出。

### 26.3 必须实现

v0.2 只实例化 Semantic & Metric、Metadata/Freshness、Query Planning & Verification 和 Artifact Packaging 中 Query 闭环需要的行为。其他 Domain Module 保持为后续演进方向，不预建空壳。

- Cargo Workspace 与四层依赖边界；
- First-run Workspace Doctor 与明确的配置诊断；
- 带 command_id 幂等语义的本地 AgentService；
- Query 真正使用的 Session、Task、Run、Step、Event、Snapshot 和 Artifact；
- SQLite Runtime Store 与本地 Artifact Store；
- Typed Event、单 Run 乐观并发、Artifact 原子提交和恢复；
- 显式 QueryIntent、QueryPhase、Harness、Agent Loop 和 Completion Gate；
- Query/Clarification/UnsupportedCapability 的确定性 Coordinator；
- ToolCatalog、按 QueryPhase 生成的静态 ToolView、Tool Runtime 和 QueryBudget；
- SQL AST、数据库只读身份、Source ACL、成本/行数/字节/超时与敏感结果防线；
- OpenAICompatibleProvider、FakeProvider 和 ReplayProvider；
- SQLite 测试数据源；
- Postgres 第一个真实数据源；
- dbt manifest 第一个工程 Context Adapter；
- 文件型最小 Metric Registry；
- 最小 ContextProvider/ContextResolver Seam、ContextManifest 与预算内 ContextPack；
- GovernedMetric、AdHocRead 和 Metadata 的 QueryVerifier；
- 带 sensitivity、retention 和安全 Preview 的结构化 QueryArtifact；
- 聚焦任务、答案、警告和下一步的 TUI；
- 用户可读错误、Task 恢复、结构化澄清和最小导出；
- Query Eval Dataset、场景式验收和确定性发布门禁；
- Secret canary、Context Injection 和敏感数据不进入 Telemetry 的测试；
- tracing 基础埋点和最小 TelemetrySink 实现。

### 26.4 长期概念不进入 v0.2 Rust 类型

Approval、action_hash、ExecutionHandle、ChangeRequest、TaskHandoff、通用 SemanticProvider 和生产审批身份仍保留在长期架构中，但 v0.2 不创建对应空 Trait、enum variant、ArtifactKind、Event 或透传 Module。

Workspace Bootstrap、WorkspaceReadinessReport、Starter Data Stack Profile、ProvisioningPlan 和远程控制平面/数据平面协议同样不进入 v0.2 Rust 类型或配置 Schema。

这些类型在首个真实调用者出现时，依据当时的权限、状态与错误语义设计。v0.2 只保留已经被 Query Runtime 使用的 TelemetrySink。

### 26.5 明确排除

- Analysis、Build/Change、Operate 和 ML Data Prep 完整 Workflow；
- Web、Mobile、共享 Server 和多用户认证；
- 后台 Worker、Webhook 和 Reconciler；
- Python Worker；
- 生产写入、Merge 和 Deploy；
- Langfuse 正式集成；
- Lance/LanceDB Adapter、Embedding、向量索引与全量 RAG Pipeline；
- 完整语义引擎；
- Dashboard 生成；
- 多种非 OpenAI 协议 Provider；
- Workspace Bootstrap、客户成熟度自动诊断和非技术治理向导；
- Excel、CSV、SaaS Connector 和增量数据接入；
- Starter Data Stack 的 Provision、配置、升级和托管运维；
- YS 托管控制平面与客户数据平面的远程协议。

### 26.6 v0.2 首次使用与可信查询闭环

~~~text
ysda doctor 检查 Model、Connector、只读权限、Metric、dbt 和 Freshness
→ 用户在 TUI 输入问题
→ AgentService 创建或继续 Task
→ Coordinator 分类为 Query、Clarification 或 UnsupportedCapability
→ QueryIntent 分类与关键歧义检查
→ Resolver 读取已有 Metric/dbt Projection
→ Harness 生成 ToolView 与 ContextManifest
→ Model 提出受当前 QueryPhase 允许的 ToolCall
→ 实时 Schema/Freshness 通过 Tool Runtime 回源
→ SQL Policy、QueryBudget 和敏感策略 Preflight
→ Connector 安全执行
→ QueryVerifier 检查口径、范围、来源、新鲜度、权限和结果
→ Completion Gate
→ QueryArtifact
→ TUI 渲染答案、范围、警告、SQL 和 Evidence
→ 按 Policy 导出 JSON、CSV 或 Markdown
~~~

### 26.7 Query 状态机

~~~text
Clarify
→ ClassifyIntent
→ ResolveContext
→ Plan
→ ValidateAndPreflight
→ Execute
→ Verify
→ Package
→ ReadyToComplete
~~~

每个 QueryPhase 必须声明 typed 输入/输出、允许的转换、ToolView、可修复错误、澄清条件和进入下一阶段所需 Evidence。模型不能跳过 ValidateAndPreflight、Execute 或 Verify。

### 26.8 v0.2 恢复边界

v0.2 支持在持久化 Step 之间恢复，以及 WaitingForInput 后恢复。进程在外部 SQL 执行时崩溃，该 ToolCall 标记为 Unknown。

恢复后只有低成本、参数相同且无远端 query_id 可核对的查询可以在显式 resume 后创建新 ToolCall。高成本查询、已有 remote query_id 或成本未知的调用必须先 Reconcile、取消或请求用户确认。

精确恢复远端长任务、Webhook、Reconciler 和后台 Worker 留到 Build/Operate 阶段。

### 26.9 v0.2 验收标准

1. `ysda doctor` 能从空 Workspace 检查配置，并给出阻断项、警告和修复动作。
2. 运行 `ysda` 进入 TUI；默认界面不要求用户理解 Session、Run、Step 或 Workflow 内部术语。
3. 用户可以对 SQLite 和 Postgres 执行 GovernedMetric、AdHocRead 和 Metadata Query。
4. 未实现的 Analysis、Build、Operate 和 ML Data Prep 请求得到明确 UnsupportedCapability，不假装完成。
5. Active Metric 查询展示 Contract 版本；Draft 或冲突 Metric 不被静默使用。
6. 关键时间、时区、指标或维度歧义进入 WaitingForInput；空结果不解释为零。
7. dbt Projection 由 Resolver 读取；Schema 和 Freshness 实时回源通过 Tool Runtime。
8. 模型只看到当前 QueryPhase 的最小 ToolView，不能动态解锁 v0.2 之外的能力。
9. 单语句 AST、数据库 read-only、ACL、timeout、行数、结果字节和成本策略均有拒绝测试。
10. Secret、敏感行和禁止出境字段不进入 Prompt、Event、Telemetry 或未授权 Preview。
11. Query 失败可以按 typed error 在预算内修复，不依赖字符串判断。
12. SQL、参数摘要、结果引用、来源、时间范围、新鲜度、Policy 和 VerificationReport 进入 QueryArtifact。
13. QueryArtifact 可以在 Policy 允许时导出 JSON、CSV 或 Markdown；截断和敏感限制清晰可见。
14. 重复 command_id 不创建第二个 Run；同一 Run 的并发推进由版本冲突阻止。
15. 关闭并重新打开 TUI 后可以恢复 WaitingForInput；高成本 Unknown 查询不会被盲目重试。
16. `/new` 创建新 Session，不取消 Task；`/quit` 不取消 Run。
17. Runtime Event 与 Telemetry 分离，观测后端不可用不影响任务状态。
18. Fake/Replay Provider 可以无网络运行核心测试。
19. Query deterministic eval、Context Injection、安全和恢复场景全部通过后才允许发布。
20. `cargo fmt`、`cargo clippy --all-targets --all-features -- -D warnings` 和 `cargo test --workspace` 全部通过。

### 26.10 Pilot 成功指标

v0.2 技术验收通过后，还必须在 Pilot 记录：

- 从首次启动到首个可信答案的时间；
- 受支持 Query 的 task success rate；
- 无需人工修改 SQL 的完成率；
- 正确澄清、可信拒答和错误放行比例；
- p50/p95 首答时间、Model token 和数据库执行成本；
- 用户相对手写 SQL 的时间节省；
- QueryArtifact 的导出、复用与用户纠错次数；
- 完成首次可信答案所需的 Data Engineer 配置时间和手工步骤数。

这些指标用于判断可信 Query 内核是否成立，以及后续 Workspace Bootstrap 应优先消除哪些专业配置步骤。后续版本仍由独立 Spec、Eval 和发布门禁控制，不能因为长期愿景扩大 v0.2 范围。

## 27. 后续演进顺序

以下版本表达产品演进顺序，不表示一次实现长期愿景。每个版本都必须形成可独立使用、可测试和可发布的垂直切片；未列入该版本的长期概念不得提前创建空 Trait、状态或基础设施。

### Agent Context Lakehouse：按条件启用的横向里程碑

LanceContextRepository 不与某个业务 Workflow 版本强绑定。v0.2 之后只有同时出现以下信号，才启动独立 ADR、Spike 和正式实现：

- 确定性 Query Context Provider 或后续 FileContextRepository 的 Evidence 规模与检索延迟超出目标；
- 全文、向量或多模态混合检索在 Eval 中显著优于确定性检索；
- 已定义 Embedding 版本、增量同步、失效、重建和 Compaction 策略；
- Workspace、ACL 和 Sensitivity 可以在召回阶段正确隔离；
- 本地文件系统与对象存储上的版本锁定、崩溃恢复和迁移测试通过。

Spike 至少覆盖 10k/100k Evidence 的写入、更新、失效、混合检索、重启恢复、索引重建和权限隔离。未达到质量和运维门槛时继续使用确定性 Provider 或 FileContextRepository，不为技术选型本身扩大版本范围。

### v0.3：Workspace Bootstrap

- 面向没有常驻 Data Engineer、但已有 SQLite 或 Postgres 的客户；
- 识别 Workspace 数据成熟度、可用 Source、Schema、权限和能力缺口；
- 通过业务语言引导业务数据责任人确认时区、新鲜度、敏感数据和 Query Budget；
- 从 ObservedSchema 和可选 dbt Evidence 提出 Draft Metric、维度和数据质量建议；
- 生成 WorkspaceReadinessReport，明确当前可可信支持的问题和修复路径；
- Draft 不能由 Agent 自动提升为 Active；
- 不实现 SaaS/Excel 数据接入，不 Provision Starter Data Stack，也不要求业务责任人编辑 JSON、SQL 或 dbt 配置。

### v0.4：Analysis Workflow

- QueryArtifact 通过 TaskHandoff 创建 Analysis 子 Task；
- 可复现 Analysis Artifact；
- 图表与 Dashboard Artifact；
- 证据与假设 Eval。

### v0.5：Build/Change Workflow

- ChangeRequest；
- Git Worktree 沙箱；
- 原生 dbt/SQL Artifact 修改；
- 测试、Diff、ImpactReport；
- action_hash 只绑定沙箱内 change.prepare；
- 单用户本地模式不实现真实多人 Merge、Deploy 或生产审批。

### v0.6：Operate 与 Durable Execution

- 只读诊断、恢复计划和健康验证；
- Worker、ExecutionHandle、Webhook 和 Reconciler；
- Airflow/Dagster Adapter；
- 长任务恢复；
- 在共享身份上线前不执行需要多人审批的生产恢复动作。

### v0.7：共享 Runtime 与托管控制平面

- Server AgentService；
- Postgres Runtime Store；
- Object Storage；
- 多用户身份、授权和事件入口；
- 申请人、审批人和执行 Principal 分离；
- 在外部审批或共享身份支持下启用受治理的 Merge、Deploy 与 production.execute；
- Web 客户端基础。

### v0.8：Starter Data Stack

- 面向只有 Excel、CSV、受支持 SaaS 或业务数据库的客户；
- 提供少量经过验证的 Data Stack Profile，而不是任意技术组合；
- 通过 Adapter Provision 和配置成熟的存储、转换、调度与质量工具；
- 建立受治理的数据接入、增量同步、基础模型、调度、质量检查和运维闭环；
- 托管控制平面与客户数据平面保持分离，原始数据默认不进入控制平面；
- 以端到端可靠性和客户运维成本决定扩展 Connector，不追求 Connector 数量。

### v0.9：ML Data Prep 与 Python Worker

- Rust/Python 协议；
- Arrow/Parquet Artifact；
- 数据清洗、特征和样本 Workflow；
- 数据泄漏和质量 Eval。

### v1.0：受治理的 full-stack AI data team

- 五类 Workflow 统一入口；
- Bring Your Own Stack 与 Starter Data Stack 使用统一产品体验；
- 业务数据责任人无需数据工程知识即可完成治理与高风险决策；
- 共享 Data Context；
- 团队级 Memory；
- 完整 LLM-Ops 和持续 Eval；
- 多入口与可持续运行。

## 28. 主要风险与应对

| 风险 | 应对 |
|---|---|
| 长期愿景被误当成当前版本范围 | 每个版本声明直接用户、数据前置条件、可执行 Workflow 和明确排除项；长期概念不提前进入 Rust 类型 |
| 只服务已有成熟数据栈，无法触达真正没有数据团队的客户 | v0.3 先降低治理配置门槛，v0.8 再以有限、标准化 Profile 提供 Starter Data Stack |
| Starter Data Stack 演变为自研数据库和调度平台 | YS 只拥有控制、治理、验证和用户体验；底层始终采用成熟基础设施与 Adapter |
| 非技术责任人无法理解审批内容 | 用业务影响、数据范围、成本、可逆性和失败处理表达选择，不把 SQL 或基础设施命令当作审批界面 |
| 五类 Agent 同时开发导致 Runtime 复制 | v0.2 只用 Query 打穿公共主干 |
| Query/Pipeline/Ops Service 与 Workflow 重复 | 按目标组织 Workflow，按跨 Workflow 复用规则组织 Domain Module；Domain Module 不拥有 Loop 和 Task 状态 |
| 通用 Pipeline 或 Ops Facade 退化成巨型 switch | 统一 Intent、Artifact 和 Verification，平台差异留在 capability-based Adapter |
| Data Context 变成新 Catalog 或万能数据库 | 保持逻辑检索平面；Lance 只保存可重建 Projection，真实系统继续作为权威来源 |
| Lance 引入后污染 Core | 只通过 ContextRepository Seam 接入，Core 和 Agent Loop 不暴露 Lance、Arrow 或 Embedding SDK 类型 |
| 向量检索造成跨租户或敏感信息泄露 | Workspace、ACL 和 Sensitivity 在召回阶段过滤，并加入确定性权限 Eval |
| Embedding 或 Lance 格式升级破坏历史索引 | 显式记录版本，锁定稳定格式，先重建和兼容性验证再切换 |
| Metric Registry 演变为完整语义引擎 | 限制在治理与有限查询闭环 |
| Tool 数量膨胀 | ToolCatalog 与按 Step 生成的 ToolView 分离 |
| LLM 自己验证自己 | 确定性 Verifier 和 Completion Gate 优先 |
| 共享 Service Account 越权 | 用户权限、Workspace Policy 与 Connector Role 取交集 |
| Trace 被误当 Runtime 状态 | Run Event、Telemetry、Eval Record 分离 |
| TUI 与 Runtime 耦合 | TUI 仅通过 AgentService 与 Event Stream |
| 长任务占用 Loop 和 token | 持久化等待、ExecutionHandle、事件唤醒 |
| 过早拆分服务与 crate | 模块化单体，按依赖边界演进 |
| v0.2 基础设施挤压用户价值 | 只实现 Query 实际调用的状态、事件、类型和工具；以 Doctor、可信答案和 Pilot 指标验收 |
| 只读查询造成资源或数据泄露 | 数据库 read-only、AST、ACL、QueryBudget、敏感策略和成本预检共同防御 |
| Context 内容诱导模型越权 | Evidence 按 untrusted_data 隔离，Tool Runtime 仍执行独立 Policy |

## 29. 架构不变量

以下规则一旦违反，应视为架构回归：

1. 普通用户无需选择内部 Agent。
2. 一个 Workflow 不得拥有独立的 Harness、Policy 或 Event 体系。
3. 模型不能绕过 Tool Runtime 直接调用 Port 或 Adapter。
4. 模型不能单方面完成 Task、发布 Metric 或写入长期 Memory。
5. 生产副作用不能使用模糊 Session 级审批。
6. Agent Runtime Store 不能与用户业务数据混用。
7. Telemetry 平台不能成为恢复 Task 的权威状态源。
8. Context Index 不能被当成永远正确的事实源。
9. 没有 `change.prepare` capability 的 Principal 不能准备变更；职位名称不能替代能力检查。
10. CLI、Web 和 Worker 不得各自复制 Agent 执行逻辑。
11. Lance/LanceDB 不能存储 Task/Run 权威状态，也不能取代 Warehouse、语义层或 Artifact Store。
12. Context Repository 的具体存储类型不能泄漏到 Agent Loop、Workflow 或领域类型。
13. Domain Module 不能拥有独立 Agent Loop、Session、Task 生命周期或 Event 体系。
14. Workflow 不能绕过 Tool Runtime 或 Execution Control Plane 直接调用外部 Adapter。
15. Eval Runner、Telemetry 后端和 LLM Judge 不能成为在线 Task 状态或完成判断的权威来源。
16. Secret 不能进入 Prompt、Event、Artifact、Telemetry 或可序列化 Core 类型。
17. 只读 Tool 不能绕过 QueryBudget、数据范围和敏感策略，也不能仅因 SideEffect::None 被无条件重试。
18. v0.2 不能为未实现 Workflow 创建假的执行结果、审批或 Handoff。
19. 长期产品不能把拥有 Data Engineer 或现成数据平台作为所有客户的永久前提。
20. 业务数据责任人负责业务确认和高风险授权，但批准身份不能自动获得实际生产执行权限。
21. YS Data Agent 可以 Provision 和管理成熟基础设施，但不能重新实现数据库、计算引擎、转换框架或调度器。
22. 托管控制平面不能默认复制客户原始业务数据；任何数据出境都必须经过 Workspace Policy 和敏感级别检查。

## 30. 参考项目的取舍

### Datus-agent

借鉴数据领域 Workflow、Context、语义工具和 Artifact；避免复制大量彼此耦合的 Agentic Node 和巨型工厂。

### VTCode

借鉴 Rust Harness、Run Loop、Tool Registry、权限和 Context 管理；避免早期复制已经膨胀的 Facade 与目录规模。

### Codex

借鉴 Session/Turn/Step 分层、Tool Router、Typed Protocol、审批与多个入口共享 Runtime；将 Code Agent 语义替换为 Task-centric Data Workflow。

### Waku

借鉴透明手写 Loop、Retrieval Gate、Memory Consolidation、确定性 Eval 与 Judge Eval 分离；不复制个人助理的 Python 领域模型。

## 31. 结论

YS Data Agent 的长期形态不是五个独立聊天机器人，而是：

> 一个受业务数据责任人治理、面向没有完整数据团队的中小型公司的 AI data team。它通过统一、简单的产品入口，使用可信的 Task-centric Runtime、多种领域 Workflow 和成熟数据基础设施，在明确支持的数据源、技术栈和风险边界内，逐步覆盖并端到端完成、验证数据接入、治理、工程、分析、数据科学准备和运维工作。

v0.2 不追求功能数量，也不实现终局愿景的全部能力。它面向 Data Engineer 和技术型分析师 Pilot，以 Query 为第一个垂直切片，证明 Doctor、Harness、Tool Runtime、Context、安全执行、验证、Artifact、恢复、Eval 和 TUI 可以形成可信控制内核。后续版本再依次降低治理配置门槛、增加领域 Workflow、提供共享控制平面和有限的 Starter Data Stack。
