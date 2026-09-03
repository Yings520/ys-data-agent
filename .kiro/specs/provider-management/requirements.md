# Requirements Document

## Introduction

`provider-management` 为 v0.2 Trustworthy Query Runtime 增加本地、受治理的 LLM Provider 管理能力。技术用户可以通过 TUI 配置、验证、保存和切换 Provider Profile，而不必修改代码或理解各 Provider 的协议差异。Provider 或模型的变化不得扩大 Query 产品边界，也不得改变 Runtime、Policy、Tool Runtime、QueryBudget、数据外发策略或 Completion Gate 的权威。

## Upstream Product Source

- **BMAD PRD**: `docs/PRD.md`
- **Source revision**: 2026-09-01 工作树版本，基于 commit `e35f174a0c9f422e1f1fa75653fe68dfd4d614a5`
- **Covered PRD sections**: §2.3、§5、§6、§11、§13.4、§21、§23、§26.1、§26.3、§26.5、§26.6、§26.9、§26.10、§27「v0.2 当前扩展：统一 Model Provider 管理」、§29

> 产品目标、v0.2 发布边界、稳定架构或演进顺序的变化必须先回到 `docs/PRD.md` 协调。本文件只定义本 Feature 的可观察行为。

## Boundary Context

- **In scope**: 9 个目标 Provider 的产品目录；多个本机应用级 Provider Profile；API Key 与 ChatGPT Subscription OAuth；模型选择、参数、兼容性校验；原子激活与面向新 Run 的切换；TUI 管理闭环；Provider 指纹；既有 Query 模型调用契约回归。
- **Out of scope**: 任意自定义 Provider 协议；9 个目标 Provider 之外的支持承诺；OpenAI API；非 Chat 模态；自动回退、负载均衡或隐式路由；Web/API 管理入口；多用户控制平面；旧 `YSDA_LLM_*` 配置迁移；Analysis、Build/Change、Operate 或 ML Data Prep。
- **Adjacent expectations**: Provider 管理沿用既有 Query Runtime、Doctor、Policy、Tool Runtime、Completion Gate、Event、Artifact 与 Telemetry 约束；本 Feature 不重新定义这些能力。

## Requirements

### Requirement 1: Provider Catalog 与支持状态

**Objective:** 作为技术用户，我希望查看明确的 Provider 支持目录，以便只选择产品实际承诺并验证过的 Provider。

#### Acceptance Criteria

1. The YS Data Agent shall 在 Provider Catalog 中列出且仅列出以下 9 个目标 Provider：ChatGPT Subscription、OpenCode Go、OpenCode Zen、DeepSeek、xAI、Z.AI、OpenRouter、MiniMax 与 Anthropic。
2. The YS Data Agent shall 分别使用 `chatgpt/`、`opencode-go/`、`opencode/`、`deepseek/`、`xai/`、`zai/`、`openrouter/`、`minimax/` 与 `anthropic/` 作为目标 Provider 的模型前缀。
3. The YS Data Agent shall 为每个目标 Provider 显示名称、认证要求、配置状态与支持状态。
4. The YS Data Agent shall 将 ChatGPT Subscription 表达为 `chatgpt/` OAuth Provider，而不是 OpenAI API `openai/` Provider。
5. If 某 Provider 仅存在目录级 Chat 声明但没有模型级验收证据, the YS Data Agent shall 不将其标记为 Supported。
6. If 某目标 Provider 尚未取得认证、协议探测、参数行为和错误处理证据, the YS Data Agent shall 显示非 Supported 状态并说明缺失证据。
7. The YS Data Agent shall 不把目标 Provider 之外的目录条目标记为本期 Supported 或计入本期完成度。

### Requirement 2: Provider Profile 生命周期

**Objective:** 作为技术用户，我希望管理多个相互独立的 Provider Profile，以便保存不同 Provider 与模型的可复用配置。

#### Acceptance Criteria

1. The YS Data Agent shall 允许用户创建、查看、编辑、复制和删除多个本机应用级 Provider Profile。
2. The YS Data Agent shall 要求每个 Profile 使用本地唯一名称，并包含目标 Provider、`provider/model` 模型标识、认证关联、通用参数和状态。
3. When 用户保存未完成的 Profile, the YS Data Agent shall 将其保存为 Draft 且禁止激活。
4. If Profile 名称冲突、模型前缀与 Provider 不匹配或必填字段缺失, the YS Data Agent shall 显示对应字段的可修复错误。
5. When 用户取消编辑, the YS Data Agent shall 保持已保存 Profile 与活动 Profile 不变。
6. If Profile 编辑、保存或验证失败, the YS Data Agent shall 保持该 Profile 的上一完整状态和当前活动 Profile 不变。
7. When 用户删除活动 Profile, the YS Data Agent shall 要求用户先激活另一个 Ready Profile，或明确确认进入无活动 Provider 且不能提交 Query 的状态。
8. The YS Data Agent shall 在任一时刻最多维护一个全局活动 Provider Profile。

### Requirement 3: Provider Credential 安全与隔离

**Objective:** 作为技术用户，我希望安全保存每个 Profile 的认证材料，以便重启后继续使用且不会发生泄露或串用。

#### Acceptance Criteria

1. Where 目标 Provider 使用 API Key, the YS Data Agent shall 允许用户在 TUI 中输入、替换和删除该 Profile 专属的 API Key。
2. Where 目标 Provider 是 ChatGPT Subscription, the YS Data Agent shall 使用 OAuth Connection 而不是要求用户粘贴 OpenAI API Key。
3. When Credential 保存成功, the YS Data Agent shall 在应用重启后继续为所属 Profile 使用该 Credential，除非它已过期、撤销或删除。
4. When Credential 已保存, the YS Data Agent shall 只显示已保存、缺失或安全遮蔽状态，不重新显示完整秘密值。
5. The YS Data Agent shall 使每个 Credential 仅与一个 Profile 关联，且不支持共享凭证语义。
6. While 并发 Run、兼容性校验、请求重试或失败处理正在发生, the YS Data Agent shall 保持各 Profile 的 Credential 隔离。
7. When Credential 创建、替换、刷新或删除失败, the YS Data Agent shall 保留上一完整状态且不产生部分更新或悬空关联。
8. If 本地 Credential 保护能力不可用或保护级别无法确认, the YS Data Agent shall 拒绝保存且不得降级为明文配置、普通持久字段或环境变量回写。
9. When 用户删除 Profile, the YS Data Agent shall 经用户确认后删除其专属 Credential。
10. The YS Data Agent shall 不把明文 Credential 写入普通配置、Profile 界面、日志、错误、Telemetry、Run Event、Artifact、Provider 指纹、测试夹具、剪贴板回写或崩溃转储。

### Requirement 4: 模型选择与参数

**Objective:** 作为技术用户，我希望选择或手工输入模型并配置受支持参数，以便处理不同 Provider 的发现能力和参数差异。

#### Acceptance Criteria

1. Where Provider 支持模型发现, the YS Data Agent shall 允许用户从发现结果选择模型。
2. If 模型发现不可用、不完整或失败, the YS Data Agent shall 允许用户保存 Draft 并手工输入模型 ID。
3. When 用户手工输入模型 ID, the YS Data Agent shall 要求该模型通过与发现模型相同的兼容性校验后才能激活。
4. The YS Data Agent shall 支持模型 ID、temperature、max tokens、timeout 与 retry 作为首期通用参数。
5. When 用户保存参数, the YS Data Agent shall 校验参数类型、范围和组合，并显示可修复的字段错误。
6. If 目标 Provider 不支持某通用参数, the YS Data Agent shall 阻止该参数或明确标记为不生效，不得静默丢弃。
7. Where Provider 提供专属参数, the YS Data Agent shall 明确标识其限定语义且不得宣称跨 Provider 等价。
8. When 用户复制或切换 Profile, the YS Data Agent shall 不把不适用参数静默应用到另一个 Profile。

### Requirement 5: 配置与模型兼容性校验

**Objective:** 作为技术用户，我希望在激活前验证 Profile 和模型，以便阻止无效或能力不足的配置进入运行。

#### Acceptance Criteria

1. When 用户请求本地配置校验, the YS Data Agent shall 在不发起业务 Query 的情况下检查必填项、模型前缀、Credential 绑定和参数范围。
2. If 本地配置不完整, the YS Data Agent shall 显示字段级错误且不发起网络探测。
3. When 用户请求模型兼容性校验, the YS Data Agent shall 使用不含客户业务数据的安全探测验证 Tool Calls、非空 Tool Call IDs、多轮 Tool Result 和已知上下文限制。
4. If Provider 级能力声明与模型级探测结果冲突, the YS Data Agent shall 以模型级探测结果作为激活门禁。
5. If 探测遇到认证失败、模型不存在、能力不兼容、限流、超时、网络错误、服务端错误或协议错误, the YS Data Agent shall 返回稳定分类与可执行修复动作并保持 Profile 未激活。
6. When Profile 的 Provider、模型、关键参数或 Credential 发生变化, the YS Data Agent shall 使旧兼容性结果失效。
7. If 模型缺少任一必需协议能力或上下文限制未知, the YS Data Agent shall fail closed 且不得退化为无 Tool 的自由文本回答。
8. The YS Data Agent shall 不允许用户覆盖目标 Provider 的 Base URL、认证 origin 或 redirect 目标。

### Requirement 6: 原子激活与 Run 切换边界

**Objective:** 作为技术用户，我希望安全切换活动 Profile，以便新 Run 使用新配置而进行中的 Run 保持一致。

#### Acceptance Criteria

1. When 用户激活已通过当前兼容性校验的 Ready Profile, the YS Data Agent shall 原子更新活动 Provider Profile。
2. If Profile 为 Draft、Invalid、未验证或验证已失效, the YS Data Agent shall 拒绝激活并保持原活动 Profile 不变。
3. If 激活失败, the YS Data Agent shall 保持 TUI 与 Runtime 对原活动 Profile 的显示和使用一致。
4. When 用户准备切换活动 Profile, the YS Data Agent shall 在确认前说明切换只影响之后启动的 Task/Run。
5. While Run 正在进行, the YS Data Agent shall 保持该 Run 启动时的 Provider、模型、Credential 和关键参数不变。
6. When 切换成功后启动新 Run, the YS Data Agent shall 使用新的活动 Profile。
7. If 活动 Provider 调用失败, the YS Data Agent shall 返回明确失败且不得自动切换 Profile、降低安全标准或隐式路由。
8. The YS Data Agent shall 确保 Profile 配置、验证或切换不修改或绕过 Policy、Tool Runtime、QueryBudget、数据外发限制、Workflow 或 Completion Gate。

### Requirement 7: TUI Provider 管理闭环

**Objective:** 作为技术用户，我希望仅通过 TUI 完成 Provider 管理，以便无需直接编辑配置文件。

#### Acceptance Criteria

1. The YS Data Agent shall 允许用户在 TUI 中完成“选择 Provider → 认证 → 选择或输入模型 → 配置参数 → 校验 → 保存 → 激活”的完整流程。
2. The YS Data Agent shall 显示活动 Profile、Provider、模型、非敏感参数摘要、认证状态和最近一次兼容性校验结果。
3. When 用户在激活前返回编辑, the YS Data Agent shall 允许修改任一字段并将保存 Draft 与激活显示为不同操作。
4. If 配置或校验失败, the YS Data Agent shall 保留已输入的非敏感字段，并提供返回编辑或重试入口。
5. While 网络发现、OAuth 或兼容性校验正在进行, the YS Data Agent shall 保持界面可响应、显示进行中状态并允许安全取消。
6. If Profile 状态过期、未验证或失败, the YS Data Agent shall 明确标识且不得误显示为 Active。
7. The YS Data Agent shall 使本地 Catalog 与 Profile 浏览在无网络时仍可完成。
8. The YS Data Agent shall 不要求用户读取配置文件来确认活动 Provider 状态。

### Requirement 8: ChatGPT Subscription OAuth Connection

**Objective:** 作为 ChatGPT Subscription 用户，我希望通过 TUI 管理 OAuth Connection，以便安全处理 Token 生命周期。

#### Acceptance Criteria

1. The YS Data Agent shall 允许用户为 `chatgpt/` Profile 发起、完成、查看、刷新、重新授权和登出 OAuth Connection。
2. The YS Data Agent shall 显示 Pending、Connected、Expired、Revoked 或 Failed 状态，并提供适用的修复动作。
3. If OAuth Connection 不处于 Connected 状态, the YS Data Agent shall 阻止该 Profile 通过兼容性校验或激活。
4. When Access Token 或 Refresh Token 刷新或轮换成功, the YS Data Agent shall 原子替换受保护 Credential。
5. If Token 刷新失败或 Token 已知失效, the YS Data Agent shall fail closed 且不得继续使用该 Token 或切换 Provider。
6. When 用户登出或删除对应 Profile, the YS Data Agent shall 删除本地 OAuth Connection 并尝试远端撤销。
7. If 远端撤销失败, the YS Data Agent shall 明确报告残留风险和用户可执行的后续动作。

### Requirement 9: Query 契约、Doctor 与 Provider 指纹

**Objective:** 作为技术用户，我希望 Provider 变化不破坏可信 Query，并能解释每个 Run 实际使用的配置。

#### Acceptance Criteria

1. The YS Data Agent shall 保持既有 Query 工作流、Tool 调用闭环、Doctor、错误归一化、Query Artifact 和显式非成功状态的产品语义。
2. When 模型发起 Tool Call 并接收后续 Tool Result, the YS Data Agent shall 在多轮交互中保持非空 Provider Tool Call ID 一致。
3. The YS Data Agent shall 不允许 Provider 或模型决定权限、正式业务口径、验证通过或 Task 完成。
4. When 用户运行 Doctor, the YS Data Agent shall 检查活动 Profile 的认证与模型协议能力，并在不显示秘密值的情况下报告阻断项、警告和修复动作。
5. When 新 Run 启动, the YS Data Agent shall 绑定包含 Profile 版本、Provider、模型和关键参数标识的非敏感 Provider 指纹。
6. While Run 存续, the YS Data Agent shall 保持其 Provider 指纹不可变且不受后续 Profile 编辑或切换影响。
7. The YS Data Agent shall 不在 Provider 指纹中包含 Credential 或未经 Policy 允许的业务数据。
8. The YS Data Agent shall 继续提供无网络的 Fake 与 Replay 模型能力，以支持确定性测试和可复现运行。

### Requirement 10: 直接替换与配置边界

**Objective:** 作为项目维护者，我希望收敛到唯一 Provider 管理路径，以便避免无存量用户前提下的双实现和迁移负担。

#### Acceptance Criteria

1. The YS Data Agent shall 将 Provider Profile 与 TUI 管理流程作为唯一面向用户的 Provider 配置路径。
2. The YS Data Agent shall 不导入、迁移或兼容 `YSDA_LLM_BASE_URL`、`YSDA_LLM_API_KEY` 与 `YSDA_LLM_MODEL`。
3. The YS Data Agent shall 不提供旧 Provider 的用户可选开关、弃用窗口、迁移 Profile 或运行时回退。
4. When 正式产品路径发起模型调用, the YS Data Agent shall 通过同一个统一 Provider 契约处理目标 Provider，不新增厂商专属的 YS 配置入口。
5. The YS Data Agent shall 不因统一 Provider 替换而重构与 Provider 管理无关的 Agent Loop、Policy、Completion Gate 或 Query Artifact 行为。

### Requirement 11: 安全、隐私与发布证据

**Objective:** 作为用户与项目维护者，我希望 Provider 管理遵守敏感数据边界且支持声明可复核，以便避免泄露和虚假兼容。

#### Acceptance Criteria

1. When Provider 错误呈现或写入诊断信息, the YS Data Agent shall 清理潜在 Credential、请求正文、业务数据和 Provider 回显的敏感值。
2. While Credential 以明文存在于内存, the YS Data Agent shall 将其生命周期限制在输入、授权、刷新、验证和调用所需的最短范围。
3. When 用户切换 Provider, the YS Data Agent shall 保持已批准的数据外发范围和敏感数据 Policy 不变。
4. When 目标 Provider 准备被标记为 Supported, the YS Data Agent shall 要求具备代表性真实模型或经批准等价环境的认证、协议探测、错误处理和参数行为证据。
5. When Provider 能力基线升级, the YS Data Agent shall 重新验证全部 9 个目标 Provider 的目录、认证、参数、错误行为与模型能力门禁。
6. If 任一目标 Provider 未通过发布证据门槛, the YS Data Agent shall 不宣称 9/9 Provider 已完成。
7. The YS Data Agent shall 要求既有 Provider、Doctor 与 Query 关键契约全部通过后才允许发布。
8. The YS Data Agent shall 维持 Credential 泄露事件为 0、Provider 相关严重静默错误为 0 的发布门槛。
9. The YS Data Agent shall 不通过降低模型能力门禁、静默忽略参数差异或中途切换 Provider 来提高覆盖率或恢复速度。
