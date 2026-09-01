# Addendum：统一 LLM Provider 管理技术与设计移交

## 1. 文档角色

本 Addendum 保存不应进入 PRD 产品叙事、但需要移交给 cc-sdd requirements/design 或后续 UX 设计的技术事实、研究结论和实现风险。它不覆盖 PRD 中的产品范围与验收标准。

## 2. 当前实现基线

当前代码事实：

- `crates/ys-agent-core/src/ports.rs` 定义 `ModelProvider`，对 Runtime 暴露 `capabilities()` 与 `complete()`。
- `crates/ys-agent-adapters/src/model/openai_compatible.rs` 是当前自有 OpenAI-compatible 实现。
- `crates/ys-agent-adapters/src/model/mod.rs` 的配置包含 Base URL、API Key、模型、Tool 能力声明、上下文限制、Schema 限制与超时。
- `apps/ysda/src/bootstrap.rs` 从 `YSDA_LLM_BASE_URL`、`YSDA_LLM_API_KEY`、`YSDA_LLM_MODEL` 读取唯一活动模型配置。
- 当前 Doctor 会用无业务数据的两轮 Tool Call / Tool Result 流程探测 Tool Call ID 和多轮兼容性，并在不兼容时失败关闭。
- 当前 TUI 的 `/model` 只显示 Provider/模型；没有 Provider Profile 的创建、编辑、验证和切换流程。

上述行为只用于开发期契约回归，不形成旧配置或用户迁移承诺，也不要求下游设计保持当前文件结构。

## 3. `liter-llm` 官方研究摘要

研究基于官方仓库的固定提交视图：

- [`liter-llm` README](https://github.com/xberg-io/liter-llm/blob/302dc4fc5e2c9d4d0418336e1244af20cd54f2d6/README.md)
- [Provider 注册表](https://github.com/xberg-io/liter-llm/blob/302dc4fc5e2c9d4d0418336e1244af20cd54f2d6/schemas/providers.json)
- [Client 路由](https://github.com/xberg-io/liter-llm/blob/302dc4fc5e2c9d4d0418336e1244af20cd54f2d6/crates/liter-llm/src/client/mod.rs)
- [配置模型](https://github.com/xberg-io/liter-llm/blob/302dc4fc5e2c9d4d0418336e1244af20cd54f2d6/crates/liter-llm/src/client/llm_config.rs)
- [错误模型](https://github.com/xberg-io/liter-llm/blob/302dc4fc5e2c9d4d0418336e1244af20cd54f2d6/crates/liter-llm/src/error.rs)
- [重试行为](https://github.com/xberg-io/liter-llm/blob/302dc4fc5e2c9d4d0418336e1244af20cd54f2d6/crates/liter-llm/src/http/retry.rs)
- [自定义 Provider 边界](https://github.com/xberg-io/liter-llm/blob/302dc4fc5e2c9d4d0418336e1244af20cd54f2d6/crates/liter-llm/src/provider/custom.rs)

官方资料声明其注册表覆盖 165 个 Provider，并使用 `provider/model` 前缀进行路由。用户明确把本期实现收敛为 9 个目标 Provider；165 个 Provider 是依赖生态能力与后续扩展空间，不是本期产品承诺。

### 3.1 首期目标 Provider

| Provider | 前缀 | 本期说明 |
|---|---|---|
| ChatGPT Subscription | `chatgpt/` | 用户已确认；使用 OAuth，不包含 `openai/` |
| OpenCode Go | `opencode-go/` | 目标 Provider |
| OpenCode Zen | `opencode/` | 目标 Provider |
| DeepSeek | `deepseek/` | 目标 Provider |
| xAI | `xai/` | 目标 Provider |
| Z.AI | `zai/` | 目标 Provider |
| OpenRouter | `openrouter/` | 目标 Provider |
| MiniMax | `minimax/` | 目标 Provider |
| Anthropic | `anthropic/` | 目标 Provider |

### 3.2 能力边界

- Provider 注册表中的 Chat 能力是 Provider 级上界，不能证明具体模型支持 YS 要求的 Tool Calls、Tool Call IDs、多轮 Tool Result 或结构化输出语义。
- `liter-llm` 的统一 Chat 请求包含 temperature、top_p、max_tokens、tools、tool_choice、parallel_tool_calls、response_format、reasoning_effort 等字段，但不同 Provider 会转换、忽略、降级或拒绝不同字段。
- structured output 的严格程度在 OpenAI-compatible、Gemini/Vertex 与 Anthropic 之间不同，不能仅凭统一字段存在就声称语义等价。
- 统一错误覆盖认证、限流、Bad Request、上下文溢出、内容策略、Not Found、服务端错误、超时、网络和 Endpoint Not Supported 等；YS 仍需映射到自身稳定错误和安全呈现。
- 默认重试与超时不能直接取代 YS 产品策略；需要明确避免库重试与 YS 上层重试叠加。

### 3.3 模型发现边界

Provider 注册表能列举 Provider 元数据，但没有保证完整模型名目录。`list_models()` 依赖具体 Provider 的模型端点，可能不可用、不完整或权限受限。因此 TUI 的手工模型 ID 不是异常兜底，而应是一等配置路径。

### 3.4 凭证隔离风险

基于源码的风险判断：单个默认 Client 在请求级切换 Provider 时可能复用构造时的 API Key，只改变认证 Header 形态。下游设计不得把“改变 `provider/model` 字符串”当成完整的多 Provider 凭证切换。每个 Provider Profile 必须拥有独立认证上下文，或采用经验证的等价隔离机制。

ChatGPT Subscription 与其余 8 个目标 Provider 的认证模型不同：其余 Provider 由用户在 TUI 输入 API Key；`chatgpt/` 必须使用 OAuth Connection，并管理 Access/Refresh Token 的刷新、过期、撤销与登出。

### 3.5 自定义 Provider 边界

`liter-llm` 的运行时自定义 Provider 只覆盖其支持的 OpenAI-compatible 配置形态，例如名称、Base URL、认证 Header 和模型前缀；不提供任意协议或任意请求/响应转换。本项目不得在此能力之外发明新的自定义 Provider 协议。

### 3.6 版本风险

研究时官方 main、Changelog 与不同分发包之间存在版本时差。下游设计必须锁定实际可获取、经过审核的 Rust 依赖版本或精确提交，并以该版本的 Provider 注册表和行为作为验收基线，不能引用浮动 main 的声明。

## 4. Provider Profile 设计约束

以下是 PRD 要求形成的设计约束，不是存储 Schema 决定：

- Provider Profile 需要稳定身份与版本，以便 Run 记录不可变 Provider 指纹。
- 一个 Provider Profile 的认证上下文不能被另一个 Provider Profile 复用。
- 活动 Provider Profile 的变更需要原子发布，读者要么看到旧版本，要么看到新版本。
- Draft/Invalid Provider Profile 可以持久化，但 Runtime 不得把它用于新 Run。
- Profile 关键字段变化后，旧能力探测结果必须失效。
- Provider 专属参数应保留命名空间或等价隔离，避免跨 Provider 污染。
- 用户必须能为 8 个目标 Provider 在 TUI 直接输入 API Key，并为 ChatGPT Subscription 建立 OAuth Connection；YS 必须在本地安全持久化 API Key 或 OAuth Token。普通 Provider Profile 只关联不透明 Credential Handle；具体使用 OS Keychain、平台 Credential Store 或满足等价威胁模型的加密本地存储，由安全设计决定。
- 本地安全凭证存储必须支持跨重启读取、替换、删除、访问隔离和静态数据保护；不得把可直接解密材料与密文以等价明文保护级别共同存放。
- 本地安全凭证存储不可用时必须失败关闭；create/replace/delete 需原子化，并覆盖并发 Run、校验、重试和失败路径的凭证隔离测试。
- 首期 9 个目标 Provider 使用锁定 `liter-llm` 版本的固定端点，不暴露 Base URL、认证 origin 或 redirect 覆盖。

## 5. 开发期直接替换注意事项

- 当前没有存量用户或已部署安装，不需要读取、导入或兼容旧 `YSDA_LLM_BASE_URL`、`YSDA_LLM_API_KEY`、`YSDA_LLM_MODEL`。
- 不创建迁移 Provider Profile、Custom OpenAI 兼容入口、弃用周期或双实现开关。
- 自有 OpenAI-compatible Provider 从活动组合路径直接移除；现有相关测试只用于证明模型调用契约不回归。
- 当前实现使用 Chat Completions 风格 Tool 调用；`liter-llm` 的 Responses API 不属于跨 Provider 抽象，不应在本次替换中顺带改用。

## 6. TUI/UX 移交

PRD 只要求用户可观察行为。UX 设计需要进一步决定：

- Provider Profile 列表、详情、编辑和校验状态如何组织；
- 键盘导航、返回、取消、确认、重试和删除活动 Profile 的交互；
- API Key 的粘贴、即时遮蔽、已保存状态、替换和删除，以及 ChatGPT OAuth 的 Pending/Connected/Expired/Revoked/Failed、重新授权和登出如何安全呈现；
- 网络校验期间的进度、取消和超时反馈；
- Provider 参数差异如何渐进展示，避免把高级参数一次性堆给用户；
- 进行中的 Run 存在时，切换生效边界如何解释。

通用 TUI 主题、聊天界面、整体导航重构和与 Provider 无关的快捷键不属于本次移交。

## 7. 留给 cc-sdd 的验证重点

- 现有 `ModelProvider` 契约是否保持，或需要最小、明确、向下游可验证的扩展。
- 9 个目标 Provider 的认证构造、模型前缀、Tool Call 增量/非增量响应、Tool Call ID、多轮 Tool Result 和错误映射。
- 验证锁定的 `liter-llm` 版本中 `chatgpt/` OAuth 登录、刷新、撤销和 Tool Call 行为。
- `liter-llm` 内建重试与 YS 重试、超时和取消语义的组合。
- Profile 状态、活动指针和 Run Provider 指纹的持久化一致性。
- 凭证 canary 不出现在 Debug、Display、错误、日志、Telemetry 或测试快照中。
- 模型发现失败与手工模型 ID 的完整路径。
- 活动组合路径不再引用自有 OpenAI-compatible Provider，且不存在旧配置兼容或迁移路径。
- 全部 Provider 变更必须继续通过项目 canonical gates；Docker 等前置条件缺失时按项目规则报告 `MANUAL_VERIFY_REQUIRED`。
