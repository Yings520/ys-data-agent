# Requirements Document

## Status

- **Feature:** `provider-management`
- **cc-sdd phase:** Initialized
- **Upstream project design:** `docs/PRD.md`
- **Migration source base:** `702e0dc01b02b1082c949a8ab593ec8f5cebfefe`
- **Approval:** 本文当前是迁移后的 Feature 输入基线，尚未通过 cc-sdd Requirements Review，不代表 `requirements.md` 已获批准。

下一步在 Codex 对话中运行：

```text
$kiro-spec-requirements provider-management
```

该步骤会把以下输入整理为可测试、使用 EARS 表达并可追踪的正式 Requirements。之后必须按顺序人工批准 Requirements、Design 和 Tasks；在 Tasks 批准前不得启动 Ralph。

## Feature Input Baseline：统一 LLM Provider 接入与管理

本基线从项目总纲 `docs/PRD.md` 迁出，因为它描述的是一个尚待 cc-sdd 细化和批准的独立 Feature，而不是项目级稳定设计。该 Feature 若获批，仅针对 Provider 管理能力提议取代项目总纲 §26.5 中“多种非 OpenAI 协议 Provider”排除项；在 cc-sdd Requirements 获得人工批准前，不改变项目总纲的当前发布边界。

#### 0. 文档目的

本节是统一 LLM Provider 接入与管理能力的 Feature 输入基线，供后续 cc-sdd requirements、design、tasks 使用。功能需求使用稳定编号，未经用户确认的推断统一标记为 `[ASSUMPTION]`。具体 adapter 结构、依赖接法、配置持久化机制和 TUI 组件设计交由 cc-sdd 技术设计。

#### 1. 愿景与目标

YS Data Agent 需要用一致的产品模型接入和管理多个 LLM Provider，使用户不必理解每家厂商不同的认证、模型命名和调用差异，就能选择满足 YS 可信运行要求的 Provider 与模型。统一能力由 `liter-llm` 提供协议基础，替换当前由 YS 自行维护的 OpenAI-compatible Provider 实现，从而减少重复适配、降低升级成本，并为后续扩展保留一致边界。

本 Feature 的主轴是 **LLM Provider 接入与管理**。TUI 只是用户查看、配置、验证和切换 Provider Profile 的首期管理入口，不是独立的 TUI 优化项目。

Provider 或模型的变化不得改变 YS 的产品承诺：模型仍只能提出动作；Runtime、Policy 与 Completion Gate 继续决定权限、安全执行、验证和正式完成。当前 v0.2 仍只承诺本地、只读、受治理的 Trustworthy Query；统一 Provider 不扩大为 Analysis、Build/Change、Operate 或 ML Data Prep。

##### 1.1 产品目标

- 用一个统一抽象覆盖首期 9 个目标 Provider，减少 YS 自有 Provider 协议代码和逐厂商适配。
- 让用户通过 TUI 管理多个 Provider Profile，并明确知道当前活动 Provider、模型、参数和兼容状态。
- 让 Provider 切换可验证、可解释且原子生效，不对进行中的 Run 或治理语义造成隐式变化。
- 直接以 `liter-llm` 替换自有 OpenAI-compatible Provider，同时保持已开发的 Query 模型调用契约不回归。
- 保留 `liter-llm` 其余 Provider 的后续扩展路径，但不把 165 个目录条目全部纳入首期实现和验收。

##### 1.2 为什么现在做

当前实现依赖单一、自定义 OpenAI-compatible Provider，配置只有一组 Base URL、API Key 和模型。项目仍处于开发阶段，尚无存量用户，因此现在可以直接收敛接入层，而无需承担用户迁移或旧配置兼容成本。`liter-llm` 已提供统一 Provider 注册表及 `provider/model` 路由，具备替代自有通用适配层的基础；YS 仍需在其上维持自身的能力门禁和治理约束。

#### 2. 目标用户与用户流程

##### 2.1 目标用户

本功能完成后的预期用户，是负责在本地部署、配置或维护 YS Data Agent 的技术用户，包括 Data Engineer、技术分析师和产品维护者。他们需要为 YS 选择可用的 LLM Provider 和模型，但不应被迫修改代码或理解每家 Provider 的底层请求协议。当前项目仍在开发阶段，本文不声称已有用户或已部署安装。

该技术操作角色只负责 Provider Profile 配置，不取代 YS 产品级角色：业务使用者仍提出并使用业务结果，Data Steward 仍维护数据来源、计算逻辑、质量规则与验证方法，Accountable Data Owner 仍裁决正式业务定义、口径冲突和中高影响用途。配置或切换 LLM Provider 不授予任何新增业务或治理权限。

##### 2.2 用户需要完成的工作

- 查看 YS 首期支持的 Provider、模型配置方式及当前兼容状态。
- 为不同 Provider 建立相互隔离的 Provider Profile。
- 为 8 个 API Key Provider 直接在 TUI 输入 API Key；为 ChatGPT Subscription 发起 OAuth 登录；配置模型和必要参数，并在激活前发现缺失或不兼容项。
- 在不同 Provider Profile 之间切换，同时知道切换何时生效、是否影响现有 Run。
- 当 Provider 不可达、凭证无效或模型能力不足时，获得安全且可操作的错误，而不是模糊失败或静默回退。

##### 2.3 非目标用户

- 需要云端多租户 Provider 控制平面的管理员。
- 需要自定义任意 Provider 协议、请求格式或响应转换的插件开发者。
- 只使用 Embeddings、Image、Audio 或 Moderation，而不运行 YS Query 工作流的用户。

##### 2.4 关键用户流程

- **UJ-1：Chen 首次配置 Provider Profile。** Chen 在本地启动 YS，通过 TUI 进入 Provider 管理入口并选择目标 Provider。选择 8 个 API Key Provider 之一时，Chen 粘贴 API Key；选择 ChatGPT Subscription 时，Chen 从 TUI 发起 OAuth 登录并按 Provider 授权流程完成连接。YS 在本地安全保存 API Key 或 OAuth Token；验证通过后，Chen 激活该 Provider Profile。TUI 明确显示活动 Provider、模型和认证状态，但不重新显示秘密值。若模型列表无法自动获取，Chen 可以手工输入模型 ID，不会被阻塞在发现步骤。

- **UJ-2：Chen 在两个 Provider 之间安全切换。** Chen 已保存两个 Provider Profile。一个 Query Run 正在执行时，Chen 选择另一个 Provider Profile；系统说明切换只影响之后启动的 Task/Run，现有 Run 保持启动时快照。新 Provider Profile 只有在配置和能力校验通过后才能原子激活；失败时原活动 Provider Profile 不变。

#### 3. 术语表

- **LLM Provider** — 提供模型调用能力的厂商或服务端点，由 `liter-llm` Provider 前缀标识。
- **目标 Provider** — 本期承诺实现和验收的 LLM Provider；共 9 个。
- **Provider Catalog** — YS 当前可管理的目标 Provider 清单及其前缀、认证要求和已知能力元数据。
- **Provider Profile** — 一组本机应用级、具名且可持久化的 Provider 配置，包含 LLM Provider、模型、Provider Credential 关联和参数；MVP 不按 Workspace 隔离。
- **活动 Provider Profile** — 供之后新建 Task/Run 使用的唯一全局 Provider Profile。
- **模型** — 由 `provider/model` 标识的具体 LLM。
- **模型能力** — YS 运行某个模型所需且经验证的行为，包括当前 Query 所依赖的 Tool Calls、Tool Call IDs、多轮 Tool Result 和已知上下文限制。
- **Provider Credential** — 由 YS 在本地安全保存的 Provider 认证材料；对 8 个 API Key Provider 是用户在 TUI 输入的 API Key，对 ChatGPT Subscription 是 OAuth Access/Refresh Token 及必要状态。
- **OAuth Connection** — ChatGPT Subscription Provider Profile 的授权连接，具有 Pending、Connected、Expired、Revoked 或 Failed 状态，并支持刷新、重新授权和登出。
- **本地安全凭证存储** — 与普通 Provider Profile 配置分离、保护 Provider Credential 静态数据且不向用户重新显示明文的本地持久化能力；具体机制由下游安全设计决定。
- **通用参数** — YS 在多个目标 Provider 间以一致产品语义提供的有限参数集合。
- **Provider 专属参数** — 仅在 `liter-llm` 与目标 Provider 明确支持时可用、且不承诺跨 Provider 等价的参数。
- **兼容性校验** — 在 Provider Profile 激活前进行的本地配置检查和必要的模型协议探测。
- **Provider 指纹** — 记录在 Run 证据中的非敏感 Provider Profile 版本、Provider、模型和关键参数标识，用于解释与复现。

#### 4. 功能需求

##### 4.1 目标 Provider 与 Provider Catalog

**描述：** YS 以 `liter-llm` 的统一抽象接入首期目标 Provider，不为每家目标 Provider 维护独立协议实现。Provider Catalog 是产品支持声明，不等同于每个 Provider 下所有模型都能通过 YS 的模型能力门禁。

首期目标 Provider：

| 显示名称 | `liter-llm` 前缀 |
|---|---|
| ChatGPT Subscription | `chatgpt/` |
| OpenCode Go | `opencode-go/` |
| OpenCode Zen | `opencode/` |
| DeepSeek | `deepseek/` |
| xAI | `xai/` |
| Z.AI | `zai/` |
| OpenRouter | `openrouter/` |
| MiniMax | `minimax/` |
| Anthropic | `anthropic/` |

本期 “ChatGPT” 明确指 `liter-llm` 的 ChatGPT Subscription（`chatgpt/`），不包含 OpenAI API（`openai/`）。

###### FR-1：提供首期 Provider Catalog

用户可以查看全部 9 个目标 Provider 的显示名称、前缀、认证要求、配置状态和支持状态。

**可验收结果：**

- Provider Catalog 恰好包含本期列出的 9 个目标 Provider。
- 目标 Provider 使用 `provider/model` 前缀形成模型标识。
- Provider Catalog 不把 `liter-llm` 的其余 Provider 显示为本期已支持。
- 其余 Provider 可标记为后续扩展，但不能进入本期完成度统计。
- 任一目标 Provider 未通过 AC-11 时，本期不得宣称 9/9 完成；移除目标 Provider 或将其降级为非 Supported 必须重新批准本 Feature 的 Requirements。只有该变化同时改变项目发布边界时，才更新 `docs/PRD.md`。

###### FR-2：区分目录能力与模型兼容性

系统必须区分 `liter-llm` 的 Provider 级能力元数据和具体模型经 YS 验证后的模型能力。

**可验收结果：**

- “Provider 支持 Chat”不会自动使该 Provider 的任意模型成为可激活模型。
- TUI 能显示“未验证”“兼容”“不兼容”“暂不可验证”等明确状态。
- 不兼容原因可定位到缺失的模型能力或失败的验证步骤。

##### 4.2 Provider Profile 配置模型

**描述：** 用户通过 Provider Profile 管理多个目标 Provider。配置模型对 Provider 差异做最小必要抽象，同时保留 Provider 专属能力的明确边界。

本期支持多个本机应用级 Provider Profile，且任一时刻只有一个全局活动 Provider Profile。

Provider Profile 至少包含：

| 字段 | 产品语义 |
|---|---|
| 名称 | 用户可识别且在本地唯一的 Provider Profile 名称 |
| LLM Provider | 9 个目标 Provider 之一 |
| 模型 | `provider/model` 标识；可由发现结果选择或手工输入 |
| 认证方式 | 8 个 API Key Provider 使用 TUI API Key 输入；ChatGPT Subscription 使用 OAuth Connection |
| Provider Credential | API Key 或 OAuth Token；由本地安全凭证存储保护 |
| 通用参数 | 模型 ID、temperature、max tokens、timeout、retry |
| Provider 专属参数 | 仅在明确支持时出现，并标注非跨 Provider 等价 |
| 状态 | Draft、Invalid、Ready 或 Active |
| 来源 | 由用户通过 TUI 创建 |

###### FR-3：创建和管理 Provider Profile

用户可以创建、查看、编辑、复制和删除 Provider Profile。

**可验收结果：**

- 未完成的 Provider Profile 可以保存为 Draft，但不能激活。
- 编辑或验证失败不会破坏当前活动 Provider Profile。
- 删除活动 Provider Profile 前，用户必须先激活另一个 Ready Provider Profile，或明确进入无活动 Provider 的不可提交状态。
- Provider Profile 名称冲突、Provider 前缀错误和缺少必填字段会产生字段级错误。

###### FR-4：选择或输入模型

用户可以在 Provider 支持模型发现时选择模型，也可以在模型列表不可用、不完整或请求失败时手工输入模型 ID。

**可验收结果：**

- 模型发现失败不会阻止用户保存 Draft 或手工输入模型 ID。
- 手工输入的模型仍必须通过兼容性校验才能激活。
- 系统不会用 Provider 的模型数量或前缀推断具体模型必然存在。

###### FR-5：配置模型参数

用户可以配置通用参数，并在适用时配置明确标识的 Provider 专属参数。

首期通用参数为模型 ID、temperature、max tokens、timeout 和 retry。

**可验收结果：**

- 参数在保存前接受类型、范围和组合校验。
- 目标 Provider 不支持的通用参数会被阻止或明确标记为不生效，不能静默丢弃。
- Provider 专属参数不会伪装为跨 Provider 等价参数。
- 切换 Provider Profile 时，系统不会把不适用参数静默复制到新 Provider Profile。

###### FR-6：安全保存并隔离 Provider Credential

用户可以为 8 个 API Key Provider 直接在 TUI 粘贴 API Key，也可以为 ChatGPT Subscription 建立 OAuth Connection。YS 必须将相应 Provider Credential 保存到本地安全凭证存储，并与对应 Provider Profile 隔离关联。不能因切换模型前缀而复用另一个 Provider Profile 的 Provider Credential。

**可验收结果：**

- 用户可在 TUI 输入、替换和删除 API Key，或重新授权、撤销和删除 OAuth Connection；任何秘密值保存后不能重新显示完整内容。
- Provider Credential 在应用重启后仍可供对应 Provider Profile 使用，无需用户每次重新输入。
- Provider Credential 不以明文进入普通配置文件、Provider Profile 列表、详情、调试输出、日志、错误、Telemetry、Run 事件、Artifact 或 Provider 指纹。
- 两个 Provider Profile 使用不同 Provider Credential 时，切换后不会继续使用前一个 Provider Profile 的 Provider Credential。
- Provider Credential 隔离必须覆盖并发 Run、兼容性校验、请求重试和失败路径；不得共享可变的跨 Profile Client 认证状态。
- 创建、替换和删除 Provider Credential 必须原子化；失败时保留上一完整状态，不产生孤立密文、悬空关联或部分更新。
- 本地安全凭证存储不可用或无法确认保护级别时，保存失败关闭，不得降级为明文文件、普通数据库字段或环境变量回写。
- 删除 Provider Profile 时，其专属 Provider Credential 必须按用户确认一并安全删除；共享凭证的产品语义不在本期支持。
- 缺少或无效 Provider Credential 时，兼容性校验失败且活动 Provider Profile 不变。

##### 4.3 校验、激活与切换

**描述：** Provider Profile 的激活是一个受控状态变化。配置存在不代表模型可安全运行；系统必须先证明其满足当前 YS Query 所需协议行为。

###### FR-7：执行本地配置校验

系统可以在不发送业务数据的情况下检查 Provider Profile 的必填项、前缀、Provider Credential 绑定和参数范围。

**可验收结果：**

- 本地校验不触发业务 Query。
- 校验错误指向可修复字段，不包含凭证值。
- 配置不完整时不发起网络探测。
- 9 个目标 Provider 使用随锁定 `liter-llm` 版本定义的端点；首期 TUI 不允许用户覆盖 Base URL、认证 origin 或 redirect 目标。

###### FR-8：执行模型能力校验

系统必须在激活前验证所选模型满足当前 YS Query 依赖的模型能力。

**可验收结果：**

- 校验至少覆盖 Tool Calls、非空 Tool Call IDs、多轮 Tool Result 和已知上下文限制。
- 能力探测不包含客户业务数据，并沿用 YS 的安全 Doctor 原则。
- Provider 级能力标记不能替代模型级探测证据。
- 探测超时、认证失败、限流、不支持能力和响应协议错误会映射为稳定、可操作的失败状态。

###### FR-9：原子激活 Provider Profile

用户可以把 Ready Provider Profile 设为活动 Provider Profile；激活要么完整成功，要么保留原状态。

**可验收结果：**

- Invalid、Draft 或未验证 Provider Profile 不能激活。
- 激活失败不会产生“界面显示新 Profile、Runtime 仍使用旧 Profile”的分裂状态。
- 成功激活后，TUI 和 Runtime 对活动 Provider Profile 的显示一致。
- 激活 Provider Profile 不得修改或绕过当前 Policy、QueryBudget、数据外发限制或 Completion Gate。

###### FR-10：定义切换生效边界

Provider Profile 切换仅影响切换后启动的 Task/Run；进行中的 Run 保留其启动时 Provider Profile 快照。

**可验收结果：**

- TUI 在切换确认前说明生效范围。
- 进行中的 Run 不因活动 Provider Profile 改变而中途换 Provider、模型、凭证或参数。
- 新 Run 使用切换后的活动 Provider Profile。
- 每个 Run 的 Provider 指纹能解释其实际使用的 Provider Profile 版本。

###### FR-11：禁止静默回退

本期不提供 Provider 自动回退、负载均衡或隐式路由。

**可验收结果：**

- 活动 Provider Profile 调用失败时，系统返回明确失败，不自动换用另一个 Provider Profile。
- Provider 不支持某参数或模型能力时，系统不静默降低 YS 的安全或验证标准。
- 任何未来回退机制必须另行定义用户授权、审计和 Run 一致性语义。

##### 4.4 TUI 管理入口

**描述：** TUI 提供 Provider Profile 的首期用户交互，但 Provider Profile 状态由共享权威 Runtime 管理；TUI 不嵌入 Provider 协议或独立维护一份活动配置。

###### FR-12：查看当前 Provider 状态

用户可以从 TUI 查看活动 Provider Profile、LLM Provider、模型、参数摘要和最近一次兼容性校验结果。

**可验收结果：**

- 当前活动状态无需读取配置文件即可在 TUI 中确认。
- Provider Credential 只显示“已保存”“缺失”或安全遮蔽状态，不重新显示完整值。
- 状态过期、未验证或失败时有明确标识。

###### FR-13：完成 Provider Profile 配置流程

用户可以在 TUI 中完成“选择 LLM Provider → 输入 API Key 或完成 OAuth 登录 → 选择或输入模型 → 配置参数 → 校验 → 保存 → 激活”的完整流程。

**可验收结果：**

- 用户可以在激活前返回修改任一字段。
- 取消编辑不会修改已保存 Provider Profile 或活动 Provider Profile。
- 保存 Draft 与激活是可区分操作。
- 失败反馈保留用户已输入的非敏感字段，避免从头重填。

###### FR-14：提供可操作的失败反馈

TUI 必须把统一 Provider 错误转化为用户可理解、可修复的状态。

**可验收结果：**

- 至少区分 API Key 无效、OAuth Pending/Expired/Revoked/Failed、模型不存在、能力不兼容、限流、超时、网络错误、服务端错误和无效参数。
- 错误提示不包含 Provider 原始响应中的凭证、请求正文或业务数据。
- 用户可以从失败状态返回编辑或重试校验，不会误激活无效配置。

##### 4.5 开发期替换约束

**描述：** 项目尚无存量用户或已部署安装。本期直接以 `liter-llm` 替换自有 OpenAI-compatible Provider，不建立旧配置迁移、兼容窗口或长期双实现；但替换不能破坏已经开发并验证的 YS Query 模型调用契约。

###### FR-15：保持现有模型调用契约

新的 Provider 接入层必须继续满足 YS Runtime 所依赖的请求、响应、错误和模型能力契约。

**可验收结果：**

- 当前 Query 工作流、Tool 调用闭环、Doctor、错误归一化和 Query Artifact 行为不因接入层替换而改变产品语义。
- Tool Call ID 必须在多轮 Tool Result 中保持一致。
- Provider 或模型不得决定权限、正式业务口径、验证通过或任务完成。
- 不兼容模型被阻止时，不得退化为无 Tool 的自由文本答案来绕过门禁。

###### FR-16：记录 Provider 指纹

系统必须为每个新 Run 记录足以解释实际 Provider 行为的非敏感 Provider 指纹。

**可验收结果：**

- Provider 指纹至少包含 Provider Profile 版本、LLM Provider、模型和影响结果的关键参数标识。
- Provider 指纹不包含凭证或未经策略允许的业务数据。
- Provider 指纹与 Run 生命周期绑定，不会被之后的 Profile 编辑覆盖。

###### FR-17：直接完成统一接入层替换

YS 的活动模型调用路径必须只使用基于 `liter-llm` 的统一接入层，不保留面向用户的旧配置路径或可选双实现。

**可验收结果：**

- 生产组合路径不再实例化或调用自有 OpenAI-compatible Provider。
- `YSDA_LLM_BASE_URL`、`YSDA_LLM_API_KEY`、`YSDA_LLM_MODEL` 不需要导入、兼容或生成 Provider Profile。
- 新的 Provider Profile 与 TUI 管理模型是唯一产品配置路径。
- 不为旧实现增加迁移 adapter、兼容开关、弃用提示或运行时回退。

###### FR-18：建立 ChatGPT Subscription OAuth Connection

用户可以从 TUI 为 `chatgpt/` Provider Profile 发起、完成、查看、刷新、重新授权和登出 ChatGPT Subscription OAuth Connection。

**可验收结果：**

- `chatgpt/` 不要求用户粘贴 OpenAI API Key，也不伪装成 `openai/`。
- OAuth 登录使用 `liter-llm` 正式支持的 `chatgpt/` 路由与 OAuth 实现；YS 不新增自定义 ChatGPT 协议。
- TUI 明确显示 Pending、Connected、Expired、Revoked 或 Failed 状态，并给出重新授权或登出操作。
- Access/Refresh Token 和轮换后的 Token 只进入本地安全凭证存储；刷新与替换必须原子化。
- 登出或删除 Provider Profile 会撤销或删除本地 OAuth Connection；远端撤销失败时必须明确报告残留风险。
- OAuth Connection 未处于 Connected 状态时，Provider Profile 不能通过兼容性校验或激活。

#### 5. 明确非目标

- 不重构与 Provider 管理无关的 LLM 调用链路、Agent Loop、Runtime、Policy、Completion Gate 或 Query Artifact。
- 不把本 Feature 扩展为通用 TUI 导航、主题、聊天界面或快捷键优化。
- 不引入 `liter-llm` 支持范围外的自定义 Provider 协议、任意请求转换或任意响应转换。
- 不在首期实现 9 个目标 Provider 之外的其余 `liter-llm` Provider。
- 不承诺某个目标 Provider 下的所有模型都满足 YS 模型能力要求。
- 不把 Embeddings、Image、Audio 或 Moderation 变成当前 YS 产品能力。
- 不实现自动回退、基于延迟或成本的路由、负载均衡、Provider 轮询或 Proxy 控制平面。
- 不实现 Web/API Provider 管理界面或多用户权限控制平面。
- 不允许用户覆盖目标 Provider 的 Base URL、认证 origin 或 redirect 目标。
- 除 API Key 输入与 OAuth 授权所需的短暂秘密处理外，不在普通配置、日志、Telemetry、Run 事件、Artifact 或 TUI 中持久化、重新显示或泄露明文 Provider Credential。
- 不迁移、导入或兼容旧 `YSDA_LLM_*` 配置；当前尚无存量用户需要迁移。
- 不扩大 v0.2 的 Trustworthy Query 边界。

#### 6. MVP 范围

##### 6.1 本期范围

- 9 个目标 Provider 的统一接入。
- Provider Catalog 与多个 Provider Profile。
- 模型选择或手工输入、8 个 Provider 的 TUI API Key 输入、ChatGPT Subscription OAuth Connection、本地安全凭证保存、通用参数和受控 Provider 专属参数。
- 配置校验、模型能力校验、原子激活和面向新 Run 的安全切换。
- TUI 查看、配置、验证和切换流程。
- 已开发 Query 模型调用契约回归和 Provider 指纹。
- 明确错误、无静默降级、无静默 Provider 回退。

##### 6.2 后续范围

- `liter-llm` 其余 Provider 的增量支持。
- OpenAI API（`openai/`）。
- Provider 自动回退、复杂路由、预算路由和负载均衡。
- 非 Chat 模态能力。
- Web/API 管理入口及远程多用户配置治理。

#### 7. 跨功能质量要求

##### 7.1 安全与数据治理

- **NFR-1：** Provider Credential 必须使用受支持平台提供的本地安全凭证存储或通过等价威胁模型验证的机制持久化；静态数据不得以明文保存，解密材料不得与可直接解密的密文以等价明文保护级别共同存放。安全存储不可用时必须失败关闭。
- **NFR-2：** Provider 错误在呈现前必须清理潜在凭证、请求正文和 Provider 回显的敏感值。
- **NFR-3：** Provider Credential 仅能在 API Key 输入、OAuth 授权/刷新、验证和调用所需的最短生命周期内以明文存在于内存；不得进入普通配置、日志、错误、Telemetry、测试夹具、Run 事件、Artifact、剪贴板回写或崩溃转储。
- **NFR-4：** 切换 LLM Provider 不得扩大已批准的数据外发范围；原始业务数据默认留在客户数据平面这一原则保持不变。
- **NFR-5：** 能力不足、配置无效或协议不兼容时必须失败关闭，不得绕过安全门禁。

##### 7.2 可靠性与一致性

- **NFR-6：** Provider Profile 保存和激活必须原子化，失败后保留上一有效状态。
- **NFR-7：** 进行中的 Run 必须保持不可变 Provider 指纹，不受后续 Profile 编辑或切换影响。
- **NFR-8：** 统一错误分类必须至少保持现有认证、限流、超时、传输和 Provider HTTP 失败的可区分性。
- **NFR-9：** 模型能力探测结果必须与 Provider Profile 版本关联；Profile 关键字段或 Provider Credential 变化后旧结果失效。

##### 7.3 性能与可用性

- **NFR-10：** 本地 Provider Catalog 和 Provider Profile 浏览不依赖网络即可完成。
- **NFR-11：** 网络校验期间 TUI 保持可响应，显示进行中状态，并允许安全取消。
- **NFR-12：** 模型发现不可用时必须提供手工模型 ID 回退路径。
- **NFR-13：** 错误信息必须指出下一步可执行修复动作，而不是只暴露底层库错误。

##### 7.4 依赖与版本

- **NFR-14：** 发布必须锁定经过验证、可获取的 `liter-llm` 版本或审核后的精确版本引用，不能依赖浮动 main。
- **NFR-15：** 升级 `liter-llm` 时必须重新验证 9 个目标 Provider 的注册表、参数映射、错误行为和模型能力门禁。
- **NFR-16：** Provider Catalog 的产品声明必须与随产品发布的 `liter-llm` 版本一致。
- **NFR-17：** ChatGPT Subscription OAuth Token 刷新、轮换、过期、撤销和登出必须失败关闭且状态可观察；刷新失败不得继续使用已知失效 Token 或切换到其他 Provider。

#### 8. 开发阶段交付边界

- 当前没有存量用户、已部署安装或需要维护的旧配置。
- 本期直接删除并替换自有 OpenAI-compatible Provider，不设计用户迁移、配置导入、兼容窗口、弃用周期或双实现回退。
- 现有 Provider、Doctor 和 Query 契约测试只作为开发回归基线，不代表用户迁移承诺。
- 交付证据聚焦 9 个目标 Provider、Provider Profile、TUI 配置闭环、本地凭证安全、模型能力门禁和既有 Query 契约不回归。

#### 9. 验收标准

- **AC-1（Provider 范围）：** Provider Catalog 包含并仅将 9 个目标 Provider 标记为本期 Supported；每项使用正确的 `liter-llm` 前缀。验证 FR-1、FR-2。
- **AC-2（统一接入）：** 9 个目标 Provider 均通过同一 YS Provider 管理与模型调用契约接入，不新增厂商专属 YS 协议实现。验证 FR-1、FR-15、FR-17。
- **AC-3（Provider Profile）：** 用户可在 TUI 创建至少两个使用不同 LLM Provider 的 Provider Profile；API Key Profile 与 ChatGPT OAuth Profile 在应用重启后可分别验证和使用，无需重复输入或登录，除非凭证已过期或撤销。验证 FR-3 至 FR-6、FR-18。
- **AC-4（凭证安全与隔离）：** 测试证明 API Key 与 OAuth Token 静态数据受本地安全存储保护；在并发 Run、校验、刷新、重试、失败和两个 Provider Profile 切换场景中不会复用、串用、记录、重新显示或泄露；安全存储不可用时失败关闭。验证 FR-6、FR-18、NFR-1 至 NFR-3、NFR-17。
- **AC-5（能力门禁）：** 缺少 Tool Calls、Tool Call IDs、多轮 Tool Result 或已知上下文限制的模型不能激活，并返回可操作原因。验证 FR-2、FR-8。
- **AC-6（模型发现回退）：** Provider 无法列出模型时，用户仍可手工输入模型 ID，并在验证通过后激活。验证 FR-4。
- **AC-7（原子切换）：** 有 Run 进行时切换 Provider Profile，原 Run 保持原 Provider 指纹，新 Run 使用新 Provider 指纹；切换失败时原活动 Provider Profile 不变。验证 FR-9、FR-10、FR-16。
- **AC-8（无静默回退）：** 模拟认证失败、限流、超时、网络错误和能力不兼容时，系统不会改用其他 Provider 或降低安全标准。验证 FR-11、FR-14。
- **AC-9（TUI 闭环）：** 用户只通过 TUI 即可完成 Provider Profile 的查看、创建、编辑、校验、保存和激活；取消编辑不污染已保存状态。验证 FR-12 至 FR-14。
- **AC-10（Query 回归）：** Provider 替换后，现有 Query、Tool 调用、Doctor、Query Artifact、治理门禁和显式非成功状态的验收行为保持不变；严重静默错误为 0。验证 FR-15。
- **AC-11（Provider 证据）：** 每个目标 Provider 在正式标记 Supported 前，都有对代表性真实模型或经批准等价环境的认证、协议探测、错误处理和参数行为证据；仅有注册表 Chat 标记不算通过。验证 FR-2、FR-8、NFR-14、NFR-15。
- **AC-12（直接替换）：** 活动模型调用路径仅使用 `liter-llm` 统一接入层；不存在旧环境变量导入、兼容开关、迁移 Provider Profile 或可选双实现。验证 FR-17。
- **AC-13（ChatGPT OAuth）：** 使用 `liter-llm` 正式支持的 `chatgpt/` 能力完成 OAuth 登录、跨重启恢复、Token 刷新、过期/撤销处理、登出和代表性模型协议探测。验证 FR-18、NFR-17。

#### 10. 成功指标

以下指标只衡量统一 Provider 管理 Feature 是否具备交付条件，不能替代 `docs/PRD.md` §26.10 定义的项目级 Query Pilot 结果门槛。Provider 数量、配置成功率或契约测试通过率本身不能证明 YS 产品价值已验证。

##### 10.1 主要指标

- **SM-1：目标 Provider 覆盖率** — 9/9 目标 Provider 满足 AC-11 后标记 Supported。验证 FR-1、FR-2、FR-8。
- **SM-2：安全兼容性** — Provider 相关严重静默错误为 0，凭证泄露事件为 0。验证 FR-6、FR-11、FR-15。
- **SM-3：契约回归率** — 现有 Provider/Doctor/Query 关键契约测试在新统一层上 100% 通过。验证 FR-8、FR-15。

##### 10.2 次要指标

- **SM-4：配置闭环完成率** — 在可用凭证和兼容模型前提下，预期用户能仅通过 TUI 完成 Provider Profile 创建、校验和激活。目标：关键验收场景 100% 完成。验证 FR-12 至 FR-14。
- **SM-5：失败可诊断率** — 验收集中的认证、模型、能力、参数、限流、超时、网络和服务端失败均映射为明确类别及修复动作。目标：100%。验证 FR-7、FR-8、FR-14。

##### 10.3 反指标

- **SM-C1：不以 Provider 数量代替兼容质量。** 不通过降低模型能力门禁来提高 Supported 数量；制衡 SM-1。
- **SM-C2：不以切换速度破坏 Run 一致性。** 不通过中途换模型或静默回退来缩短恢复时间；制衡 SM-4。
- **SM-C3：不以统一参数表伪造等价性。** 不为了界面一致而静默忽略 Provider 参数差异；制衡 SM-5。

#### 11. 风险与缓解

- **Provider 级能力不等于模型级能力。** 缓解：激活前模型级协议探测；Unsupported 明确失败。
- **目标 Provider 的认证方式不同。** 缓解：Provider Profile 支持目标 Provider 要求的 API Key 或等价认证令牌，且每个 Profile 独立构造认证上下文。
- **跨 Provider 参数语义不一致。** 缓解：限制通用参数；专属参数明确标识；不支持时阻止或提示。
- **模型列表不可枚举或不完整。** 缓解：手工模型 ID 是一等路径，发现只是辅助。
- **`liter-llm` 版本和注册表变化造成行为漂移。** 缓解：锁定版本，升级时重跑 9 个目标 Provider 验收。
- **开发期替换残留旧路径。** 缓解：验收活动组合路径只使用 `liter-llm`，不增加旧配置兼容和双实现开关。
- **Provider 切换影响可复现性。** 缓解：Run 启动快照与 Provider 指纹不可变。
- **错误响应回显敏感信息。** 缓解：统一清理、凭证 canary 测试和禁止原始响应直出。

#### 12. 下游责任与决策时点

- **项目维护者** — 在目标 Provider 标记 Supported 前，提供代表性模型、测试凭证和 AC-11/AC-13 所需实证。
- **cc-sdd design owner** — 在 design 批准前选择本地安全凭证存储机制，并证明满足 NFR-1 至 NFR-3、NFR-17；不能改变 TUI 直接输入 API Key 与 ChatGPT OAuth 的产品行为。
- **依赖维护者** — 每次升级 `liter-llm` 时复核 9 个目标 Provider 的 Catalog、认证、参数、错误映射和能力门禁，再决定是否更新锁定版本。
- **UX owner** — 在 UX/cc-sdd design 中确定 TUI 布局、键盘交互和状态呈现，不扩大为通用 TUI 重构。

#### 13. 假设索引

无未解决的 `[ASSUMPTION]`。Fast path 初稿中的 Profile 作用域、通用参数、切换边界和无自动回退均已收口为正式决定。
