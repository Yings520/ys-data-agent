# YS Data Agent 完整架构设计草案

**日期：** 2026-08-06  
**状态：** 已完成产品与架构讨论，作为 v0.2 规划基线  
**替代范围：** 本文扩展并修正 2026-08-02 v0.1 设计中的长期架构；v0.1 文档继续作为已实现 Demo 的历史记录

---

## 1. 文档目的

本文定义 YS Data Agent 的长期产品定位、核心领域模型、运行时架构、权限边界、数据上下文、工具与执行模型、可观测性、评测体系、Rust 仓库结构，以及 v0.2 与长期能力之间的边界。

本文不是五类 Agent 的一次性实现清单。它首先建立所有 Workflow 共用的控制主干，再让 Query、Analysis、Build/Change、Operate 和 ML Data Prep 按垂直切片逐步接入。

## 2. 产品定位

YS Data Agent 是一个由 Data Engineer 拥有和治理的 full-stack AI data team，面向没有预算组建完整数据团队的小型或精益组织。

它的核心价值不是生成一段 SQL 或代码，而是：

> 在用户已有的数据栈上，端到端完成、验证并交付可审计的数据工作结果，放大一名资深 Data Engineer 的能力。

长期用户与能力如下：

| 用户 | 主要能力 | 权限边界 |
|---|---|---|
| 产品或业务人员 | 受治理的指标查询与业务分析 | 只读；可以提交 ChangeRequest，不能修改代码或生产系统 |
| 数据分析师 | 指标拆解、趋势、异常和归因分析 | 只读；可以提交带证据的 ChangeRequest |
| Data Engineer | ETL/ELT 开发、调试、维护和 DataOps | 可以在隔离环境准备变更；Merge、Deploy 和生产写入继续受策略控制 |
| Data/ML Engineer | 数据清洗、特征定义、样本构建和质量验证 | 仅在授权数据域内工作；模型训练与 MLOps 不属于早期范围 |

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

## 6. 总体架构

~~~text
┌──────────────────────────────── Interfaces ────────────────────────────────┐
│ Claude Code / OpenCode style TUI │ CLI Commands │ Web/API │ Event Sources │
└──────────────────────────────────────┬───────────────────────────────────────┘
                                       │
                              AgentService API
                                       │
┌──────────────────────────── Control Runtime ────────────────────────────────┐
│ Harness / Run Supervisor                                                   │
│ ├── Coordinator                                                            │
│ ├── Workflow State Machines                                                │
│ ├── Agent Loop                                                             │
│ ├── Context Assembler                                                      │
│ ├── Policy / Approval                                                      │
│ ├── Completion Gates                                                       │
│ └── Event / Snapshot / Artifact coordination                               │
└───────────────────────┬──────────────────────────────┬───────────────────────┘
                        │                              │
                Tool Runtime                    ModelProvider
           ToolCatalog → ToolView          OpenAI-compatible first
                        │
┌──────────────────── Domain Capabilities ────────────────────────────────────┐
│ Semantic Service │ Query Service │ Pipeline Service │ Ops Service │ Eval   │
└───────────────────────┬──────────────────────────────────────────────────────┘
                        │
┌──────────────────────── Ports and Adapters ─────────────────────────────────┐
│ Data Connectors │ dbt/Git Adapters │ Orchestrator Adapters │ Stores        │
│ SQLite/Postgres │ Airflow/Dagster  │ Spark/Python Workers  │ Telemetry     │
└───────────────────────┬──────────────────────────────────────────────────────┘
                        │
       Warehouse / dbt / Git / Airflow / Dagster / Spark / Object Storage
~~~

## 7. 核心领域模型

### 7.1 Workspace

Workspace 是治理和资源隔离边界，包含：

- 成员与角色；
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

如果目标、结果类型和权限边界保持一致，可以在同一个 Task 内切换 Workflow。例如 Query 结果不足以回答原因时，可从 Query 进入 Analysis。

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

这些属于 Workflow 和 Domain Service。

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

- SQL 或语义查询已经实际执行；
- 指标口径与时间范围明确；
- 数据来源和新鲜度已记录；
- QueryVerifier 的硬性检查通过；
- QueryArtifact 已持久化。

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
→ Domain Service
→ Connector
→ Execution Backend
→ 外部数据平台
~~~

- Tool 表达稳定的领域意图；
- Tool Runtime 负责参数校验、权限、审批、超时、事件和结果归一化；
- Domain Service 负责语义、查询、Pipeline 或运维规则；
- Connector 负责平台协议和方言；
- Execution Backend 负责实际运行。

Connector 默认不直接暴露给模型。

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

DataEngineer 是 Workspace 提供的一组默认授权。

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

## 16. Connector 与 Domain Service

不定义巨型 DataConnector。使用 capability-based ports：

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

Adapter 只实现真实支持的能力，并公开 CapabilityDescriptor。Workflow 在规划前即可知道环境支持什么。

PipelineIntent 可以描述输入、输出、转换、调度、质量和验收目标，但真正产物必须是用户框架的原生代码：

- dbt SQL、YAML、tests 和 macros；
- Dagster assets、resources 和 sensors；
- Airflow DAG 和 operators；
- Spark job 与测试。

## 17. Execution Control Plane

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
└── artifacts/
~~~

共享模式：

~~~text
Postgres Runtime Store
+ Object Storage Artifact Store
~~~

业务数据源与 Agent Runtime Store 必须分离。

## 20. 可观测性与 Eval

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

## 21. ModelProvider

Core 定义自己的 ModelRequest、ModelAction、ModelUsage 和 ModelFailure，不暴露具体厂商消息类型。

v0.2 实现：

- OpenAICompatibleProvider；
- FakeModelProvider；
- ReplayModelProvider。

OpenAI-compatible Adapter 支持配置 base_url、api_key 和 model，并要求 Tool Calling、Tool Call ID、结构化参数和多轮 Tool Result 回传。Provider 不满足能力时，在 Run 启动前拒绝。

后续可以新增 AnthropicProvider、GeminiProvider 或 LocalProvider，不修改 Workflow。

## 22. AgentService 与产品入口

AgentService 是所有入口共用的应用 API：

- create_session；
- create_task；
- send_message；
- start_run；
- answer_clarification；
- approve_action；
- cancel_run；
- subscribe_events；
- get_task；
- get_artifact。

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
- 当前 Session、Task、Workflow 和 Run 状态栏；
- Tool Call 折叠展示；
- 结构化澄清；
- Task 中断、恢复和切换；
- Slash Command 与命令面板。

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
│   │   └── Task、Run、Event、Tool、Artifact、Policy、Ports
│   ├── ys-agent-runtime/
│   │   └── AgentService、Harness、Loop、Workflow、Context、Verifier
│   ├── ys-agent-store/
│   │   └── SQLite/Postgres Runtime Store 与 Artifact Metadata
│   └── ys-agent-adapters/
│       └── Model、Data、dbt、Git、Telemetry Adapter
├── apps/
│   └── ysda/
│       └── CLI/TUI、配置和依赖装配
├── evals/
├── fixtures/
└── docs/
~~~

初期仍然可以编译为一个 ysda 可执行文件。Server 和 Worker 在真正需要共享 Runtime 与长任务时再新增。

Crate 拆分依据是稳定依赖边界，不是架构图中出现的名词数量。Memory、Policy、Eval 等在足够复杂前保持为 Runtime 内部模块。

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

用 Query Workflow 打穿长期公共 Runtime 主干，同时交付可用的 Claude Code/OpenCode 风格 TUI。

### 26.2 必须实现

- Cargo Workspace 与四层依赖边界；
- 本地 AgentService；
- Session、Task、Run、Step、Event、Snapshot 和 Artifact；
- SQLite Runtime Store 与本地 Artifact Store；
- Typed Event 和恢复；
- Harness、Agent Loop 与 Query Workflow；
- ToolCatalog、ToolView、Tool Runtime 和只读 Policy；
- OpenAICompatibleProvider、FakeProvider 和 ReplayProvider；
- SQLite 测试数据源；
- Postgres 第一个真实数据源；
- dbt manifest 第一个工程 Context Adapter；
- 文件型最小 Metric Registry；
- ContextManifest 与预算内 ContextPack；
- QueryVerifier 和 Query Completion Gate；
- 结构化 QueryArtifact；
- TUI 交互、Task 列表、恢复与澄清；
- Query Eval Dataset 与确定性发布门禁；
- tracing 基础埋点和 TelemetrySink 接口。

### 26.3 只定义契约，不在 v0.2 完整实现

- Approval 的 Core 类型与 action_hash；
- ExecutionHandle 与长任务状态；
- ChangeRequest 与 TaskHandoff；
- SemanticProvider 扩展接口；
- Telemetry 外部 Adapter 接口。

### 26.4 明确排除

- Analysis、Build/Change、Operate 和 ML Data Prep 完整 Workflow；
- Web、Mobile、共享 Server 和多用户认证；
- 后台 Worker、Webhook 和 Reconciler；
- Python Worker；
- 生产写入、Merge 和 Deploy；
- Langfuse 正式集成；
- 向量数据库与全量 RAG Pipeline；
- 完整语义引擎；
- Dashboard 生成；
- 多种非 OpenAI 协议 Provider。

### 26.5 v0.2 可信查询闭环

~~~text
用户在 TUI 输入问题
→ AgentService 创建或继续 Task
→ Coordinator 选择 Query Workflow
→ Resolver 获取 Metric、dbt 和 Schema Context
→ Harness 生成 ToolView 与 ContextManifest
→ Model 提出 ToolCall
→ Tool Runtime 校验并执行
→ QueryVerifier 检查口径、范围、来源和新鲜度
→ Completion Gate
→ QueryArtifact
→ TUI 渲染答案、SQL、证据和警告
~~~

### 26.6 v0.2 恢复边界

v0.2 支持在持久化 Step 之间恢复，以及 WaitingForInput 后恢复。进程在外部 SQL 正在执行时崩溃，该 ToolCall 标记为 Unknown；由于 v0.2 只允许只读查询，可以在用户恢复 Run 时创建新的 ToolCall 安全重试。

精确恢复远端长任务、Webhook、Reconciler 和后台 Worker 留到 Build/Operate 阶段。

### 26.7 v0.2 验收标准

1. 运行 ysda 进入全屏交互式 TUI。
2. TUI 展示 Workspace、Model、数据连接、权限、Session 和当前 Task。
3. 用户可以对 SQLite 和 Postgres 执行只读 Query。
4. 注册指标查询使用 Active Metric Contract，并展示版本。
5. dbt manifest、Schema 和 Freshness 通过 Context Tool 按需获取。
6. 模型只看到当前 Step 的最小 ToolView。
7. Query 失败可以在预算内由 Agent Loop 修复，不依赖字符串错误判断。
8. SQL、结果、来源、时间范围、新鲜度和 VerificationReport 进入 QueryArtifact。
9. 关闭并重新打开 TUI 后，可以恢复未完成 Task。
10. /new 创建新 Session，不取消 Task。
11. Runtime Event 与 Telemetry 分离，观测后端不可用不影响任务状态。
12. Fake/Replay Provider 可以无网络运行核心测试。
13. Query deterministic eval 全部通过后才允许发布。
14. cargo fmt、cargo clippy --all-targets --all-features -- -D warnings 和 cargo test --workspace 全部通过。

## 27. 后续演进顺序

### v0.3：Analysis Workflow

- QueryArtifact 驱动分析；
- 可复现 Analysis Artifact；
- 图表与 Dashboard Artifact；
- 证据与假设 Eval。

### v0.4：Build/Change Workflow

- ChangeRequest；
- Git Worktree 沙箱；
- 原生 dbt/SQL Artifact 修改；
- 测试、Diff、ImpactReport；
- action_hash 审批。

### v0.5：Operate 与 Durable Execution

- Worker；
- ExecutionHandle；
- Webhook 和 Reconciler；
- Airflow/Dagster Adapter；
- 长任务恢复和健康验证。

### v0.6：共享 Runtime

- Server AgentService；
- Postgres Runtime Store；
- Object Storage；
- 多用户身份、授权和事件入口；
- Web 客户端基础。

### v0.7：ML Data Prep 与 Python Worker

- Rust/Python 协议；
- Arrow/Parquet Artifact；
- 数据清洗、特征和样本 Workflow；
- 数据泄漏和质量 Eval。

### v1.0：受治理的 full-stack AI data team

- 五类 Workflow 统一入口；
- 共享 Data Context；
- 团队级 Memory；
- 完整 LLM-Ops 和持续 Eval；
- 多入口与可持续运行。

## 28. 主要风险与应对

| 风险 | 应对 |
|---|---|
| 五类 Agent 同时开发导致 Runtime 复制 | v0.2 只用 Query 打穿公共主干 |
| Data Context 变成新 Catalog 项目 | 保持逻辑检索平面，真实系统继续作为权威来源 |
| Metric Registry 演变为完整语义引擎 | 限制在治理与有限查询闭环 |
| Tool 数量膨胀 | ToolCatalog 与按 Step 生成的 ToolView 分离 |
| LLM 自己验证自己 | 确定性 Verifier 和 Completion Gate 优先 |
| 共享 Service Account 越权 | 用户权限、Workspace Policy 与 Connector Role 取交集 |
| Trace 被误当 Runtime 状态 | Run Event、Telemetry、Eval Record 分离 |
| TUI 与 Runtime 耦合 | TUI 仅通过 AgentService 与 Event Stream |
| 长任务占用 Loop 和 token | 持久化等待、ExecutionHandle、事件唤醒 |
| 过早拆分服务与 crate | 模块化单体，按依赖边界演进 |

## 29. 架构不变量

以下规则一旦违反，应视为架构回归：

1. 普通用户无需选择内部 Agent。
2. 一个 Workflow 不得拥有独立的 Harness、Policy 或 Event 体系。
3. 模型不能绕过 Tool Runtime 直接调用 Connector。
4. 模型不能单方面完成 Task、发布 Metric 或写入长期 Memory。
5. 生产副作用不能使用模糊 Session 级审批。
6. Agent Runtime Store 不能与用户业务数据混用。
7. Telemetry 平台不能成为恢复 Task 的权威状态源。
8. Context Index 不能被当成永远正确的事实源。
9. 非 Data Engineer 不能获得 change.prepare 能力。
10. CLI、Web 和 Worker 不得各自复制 Agent 执行逻辑。

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

> 一个受治理、可恢复、可评测的 Task-centric Data Agent Runtime，承载多个领域 Workflow，并通过共享 Data Context 和现有数据平台端到端完成数据工作。

v0.2 不追求功能数量。它以 Query 为第一个垂直切片，证明公共 Harness、Tool Runtime、Context、持久化、验证、Eval 和 TUI 可以形成一个可信闭环。
