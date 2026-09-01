# 安全与 Provider 专项评审

## 评审结论

**Verdict：PASS WITH NOTES（所有 Critical/High findings 已在 PRD 层关闭；实施能否启动继续受 ChatGPT 依赖门禁约束）。**

2026-09-01 复核确认：PRD 已把 ChatGPT Subscription 保持为强制 `chatgpt/` 目标，新增独立 OAuth Connection、FR-18、NFR-17 与 AC-13，并在 cc-sdd design 前设置失败关闭的依赖门禁；无合格 `liter-llm` 版本时不得进入实施，也不得自建协议、改成 `openai/` 或未经重新批准缩减为 8 个 Provider。后续更新又通过 FR-6/AC-4 覆盖并发 Run、校验、OAuth 刷新、重试、失败路径的凭证隔离及原子 create/replace/delete，通过 FR-7 与 Non-Goals 禁止 Base URL、认证 origin、redirect 覆盖，并通过 NFR-1/FR-6/AC-4 规定受支持平台安全存储或等价威胁模型及不可用时失败关闭。因此 SEC-PROV-01 至 SEC-PROV-05 均已在产品需求层充分处理。

## Findings 汇总

| ID | 严重度 | 主题 | 状态 |
|---|---|---|---|
| SEC-PROV-01 | Critical | `chatgpt/` 在引用的 `liter-llm` 固定提交中只有目录条目，没有可用路由/认证实现 | 已关闭（PRD 层；实施受依赖门禁约束） |
| SEC-PROV-02 | High | Provider Credential 被统一为“粘贴 API Key/令牌”，没有认证类型与生命周期模型 | 已关闭 |
| SEC-PROV-03 | High | 凭证隔离验收没有覆盖并发 Run、Client 复用、重试与失败切换 | 已关闭 |
| SEC-PROV-04 | High | 可覆盖 Base URL 会形成凭证外发通道，当前无 host/TLS 绑定要求 | 已关闭 |
| SEC-PROV-05 | High | “本地安全凭证存储”缺少平台、失败模式和原子生命周期验收 | 已关闭（PRD 层） |
| SEC-PROV-06 | Medium | 模型能力门禁证据的新鲜度、探测稳定性与上下文限制判定未定义 | 建议修复 |
| SEC-PROV-07 | Medium | `retry`/`timeout` 被当作通用模型参数，但总调用预算与重试叠加未定义 | 建议修复 |
| SEC-PROV-08 | Medium | “安全删除”容易被误解为远端撤销或介质级擦除 | 建议修复 |
| SEC-PROV-09 | Low | 错误脱敏要求缺少结构化 allowlist 原则 | 建议修复 |

## 详细 Findings

### SEC-PROV-01 — `chatgpt/` 目录条目不等于可调用实现

**严重度：Critical**

**状态：已关闭（PRD 层）。** PRD §8 已要求 cc-sdd design 获批前锁定并验证真正实现 `chatgpt/` 路由与 OAuth 的 `liter-llm` 版本；无版本时实施保持 BLOCK。FR-18、NFR-17 与 AC-13 同时禁止自建 ChatGPT 协议、静默改成 `openai/` 或未经重新批准缩减为 8 个 Provider。该处理没有假称固定提交已经支持 ChatGPT，而是把真实依赖变成明确、失败关闭且不可绕过的下游准入门禁。

**证据：**

- PRD 把 ChatGPT Subscription（`chatgpt/`）列为 9/9 必须支持的 Provider，并把 9/9 设为成功门槛（PRD §4.1、AC-1/AC-2/AC-11、SM-1）。
- 引用的 `liter-llm` 固定提交 `302dc4f` 中，`chatgpt` 注册表项只有名称、endpoint、model prefixes 和能力元数据，**没有 `base_url` 或 `auth` 配置**：[providers.json L577-L592](https://github.com/xberg-io/liter-llm/blob/302dc4fc5e2c9d4d0418336e1244af20cd54f2d6/schemas/providers.json#L577-L592)。
- 同一提交把 `chatgpt` 列为 `complex_providers`：[providers.json L3652-L3669](https://github.com/xberg-io/liter-llm/blob/302dc4fc5e2c9d4d0418336e1244af20cd54f2d6/schemas/providers.json#L3652-L3669)。`detect_provider()` 明确排除 complex provider，且代码中没有 ChatGPT 专用分支；无法识别时返回 `None`：[provider/mod.rs L818-L895](https://github.com/xberg-io/liter-llm/blob/302dc4fc5e2c9d4d0418336e1244af20cd54f2d6/crates/liter-llm/src/provider/mod.rs#L818-L895)。随后 Client 会回落到构造时 Provider/默认 OpenAI，而不是获得 ChatGPT Subscription 认证语义。
- 因此，注册表的 `function_calling: true` 只能视为目录元数据，不能证明 `chatgpt/` 在该 Rust client 版本中能认证、路由或完成 YS Doctor。

**影响：** 当前 PRD 同时要求“只使用 `liter-llm`”“不新增 YS 厂商专属协议”“9/9 必须 Supported”。在固定依赖证据下，这三项对 ChatGPT Subscription 无法同时成立。直接实现会落入错误 OpenAI 路由、私自补协议，或伪造 Supported 状态之一。

**具体修复：** 在 final 前做明确产品决策，二选一：

1. **推荐：** 将 ChatGPT Subscription 标为 `Candidate/Blocked by dependency`，从首期 9/9 release gate 中移除；首期变为 8 个 API-key Provider，待 `liter-llm` 的已发布 Rust 版本提供并验证 ChatGPT Subscription 路由、OAuth、刷新与 Tool Call 后再加入；或
2. 保留 9/9，但 PRD 必须先锁定一个确实实现 `chatgpt/` 的 `liter-llm` 发布版本/提交，并增加真实端到端证据：官方登录流程、access/refresh token 生命周期、刷新/撤销/过期、目标 endpoint、Chat Completions bridge 或 Responses 语义、Tool Call/Tool Result、失败脱敏。若需要 YS 自己实现 ChatGPT 协议/OAuth，则必须显式修改“只使用 liter-llm / 不新增厂商协议”的范围，而不能把它当成普通 adapter 细节。

### SEC-PROV-02 — Credential 模型没有区分静态 API Key 与交互式 OAuth

**严重度：High**

**状态：已关闭。** PRD 已明确区分 8 个 API Key Provider 与 ChatGPT OAuth Connection；FR-18/NFR-17/AC-13 覆盖 Pending、Connected、Expired、Revoked、Failed、跨重启恢复、Access/Refresh Token 安全持久化、刷新/轮换、重新授权、登出、远端撤销失败和失败关闭，不再把 ChatGPT 认证伪装成可粘贴的静态 API Key。

**证据：** UJ-1、Provider Profile、FR-6、FR-13 和 AC-3 都以“用户粘贴 API Key 或等价认证令牌、跨重启无需重新输入”为单一路径。Anthropic 和其余静态 key Provider 已有不同 header 语义；例如 `liter-llm` 的 Anthropic 实现使用 `x-api-key` 而非 Bearer：[anthropic.rs L87-L106](https://github.com/xberg-io/liter-llm/blob/302dc4fc5e2c9d4d0418336e1244af20cd54f2d6/crates/liter-llm/src/provider/anthropic.rs#L87-L106)。ChatGPT Subscription 若未来可用，通常还需要交互登录、短期 access token、refresh token、到期刷新和登出/撤销，而不是让用户粘贴一个等价 API Key。

**影响：** 单一 `Provider Credential: string` 会把 header 格式、token 类型、过期/刷新和登出语义推给 adapter 临时决定；TUI 也无法准确展示“已保存 API Key”“会话已过期”“需要重新登录”等不同状态。把 refresh token 当普通 API Key 保存还会低估其权限与生命周期风险。

**具体修复：** 把 Provider Catalog 的“认证要求”升级为可验收的 **Credential Kind**：至少区分 `StaticApiKey` 与 `OAuthSession`。Profile 只关联不透明 `Credential Handle`，不存原值；认证构造由 Provider metadata/adapter 决定，TUI 按 kind 呈现“粘贴 API Key”或“登录/重新认证/登出”。为 OAuth 定义 access/refresh token 分离、expiry、自动刷新、刷新失败、撤销和账号标识脱敏；若按 SEC-PROV-01 暂缓 ChatGPT，则明确首期 8 个 Provider 均为 StaticApiKey，禁止用“等价令牌”模糊扩张。

### SEC-PROV-03 — Profile 隔离没有覆盖并发与 Client 复用

**严重度：High**

**状态：已关闭。** FR-6 已要求隔离覆盖并发 Run、兼容性校验、请求重试和失败路径，明确禁止共享可变的跨 Profile Client 认证状态；Credential create/replace/delete 必须原子化且不得产生 orphan 或悬空关联。AC-4 进一步把并发 Run、校验、OAuth 刷新、重试、失败和 Profile 切换纳入“不得复用、串用、记录或泄露”的测试门禁。具体 canary、mock endpoint 与 header-host 配对属于 cc-sdd 测试设计，不再是 PRD 缺口。

**证据：** Addendum §3.4 已识别 `liter-llm` Client 的 key 是构造时配置；请求级 Provider 解析会按新 Provider 重建 header 形态，但仍读取同一个 `config.api_key`：[client/mod.rs L787-L827](https://github.com/xberg-io/liter-llm/blob/302dc4fc5e2c9d4d0418336e1244af20cd54f2d6/crates/liter-llm/src/client/mod.rs#L787-L827)。PRD FR-6/AC-4 只描述两个 Profile 之间“切换后不串用”，AC-7 只校验 Provider 指纹；没有覆盖旧 Run 与新 Run 并发、Client pool/cache 复用、retry、取消、验证失败和凭证替换时的真实 outbound credential。

**影响：** UI 和 Provider 指纹都可能正确，但并发请求仍携带另一 Profile 的 key。该错误会把凭证发送给错误 Provider，属于跨信任域泄露，而不是普通认证失败。

**具体修复：** 在 FR-6/NFR-7/AC-4 增加以下不变量和验收：

- 每个 Run 在启动时捕获不可变的 `{profile_version, credential_handle_version, provider endpoint, model, params}`；
- Client/adapter 实例只能绑定一个认证上下文，不允许仅改 `provider/model` 在 Profile 间复用 construction-time key；
- 使用两个 canary key、两个 mock endpoint 并发执行旧/新 Run，覆盖 retry、取消、失败激活、key 替换和 Profile 删除；逐请求断言正确 header（Anthropic `x-api-key`，其余相应 Bearer）、host 与 key 配对，且 canary 不出现在日志/错误/快照；
- 正在运行的 Run 使用的 credential handle 被替换/删除时，明确“Run 完成前保留受控引用”或“立即取消”的产品语义，不能悬空或偷偷换 key。

### SEC-PROV-04 — Base URL 覆盖可能把 API Key 发送到非预期主机

**严重度：High**

**状态：已关闭。** FR-7 规定 9 个目标 Provider 只能使用锁定 `liter-llm` 版本定义的端点，首期 TUI 禁止覆盖 Base URL、认证 origin 或 redirect 目标；Non-Goals 同步排除该能力。MVP 已移除本 finding 所依赖的可配置外发通道。

**证据：** Provider Profile 把 Base URL 作为条件字段，FR-7 只要求“端点格式”检查。依赖 Client 在设置 `base_url` 后会固定 construction-time provider/自定义 endpoint；这意味着保存的 credential 会被发送到该地址。PRD 没有规定 scheme、host allowlist、重定向、DNS rebinding、TLS 或 credential-host 绑定。

**影响：** 配置错误、恶意配置文件修改或不安全重定向可将 API Key 外发至攻击者控制的 endpoint。对本期 9 个已知云 Provider 而言，自由 Base URL 也会悄然变成被非目标声明排除的“自定义 Provider”入口。

**具体修复：** 推荐从 MVP Profile 中删除 Base URL 覆盖，9 个目标 Provider 使用随锁定 `liter-llm` 版本审核过的 canonical endpoint。若产品必须保留，则加入独立安全要求：仅 HTTPS、禁止 userinfo、禁止自动跟随到不同 origin、Provider/region allowlist、credential 与 `{provider, origin}` 强绑定、origin 改变即使能力结果失效并要求重新确认/重新输入 credential、禁止向 loopback/link-local/private IP 发送云 Provider key，并用恶意 redirect/DNS/host 测试验收。

### SEC-PROV-05 — “本地安全凭证存储”仍无法形成跨平台 release gate

**严重度：High**

**状态：已关闭（PRD 层）。** NFR-1 要求使用受支持平台提供的安全凭证存储，或经等价威胁模型验证的机制，并规定不可用时失败关闭；FR-6 禁止降级到明文文件、普通数据库字段或环境变量回写，且要求 create/replace/delete 原子化；AC-4 将静态保护、隔离和安全存储不可用纳入验收。具体平台选型和故障注入矩阵可由 cc-sdd design/tasks 展开。

**证据：** FR-6/NFR-1 给出正确原则，但 Addendum 仍把实现留为 “OS Keychain、平台 Credential Store 或等价加密本地存储”。PRD 没有说明 v0.2 支持哪些 OS、vault 不可用/锁定时怎么办、主密钥在哪里、Profile 元数据与 credential 写入如何原子提交，以及取消/崩溃/替换失败后如何处理 orphan secret。

**影响：** 实现可以用“本地加密文件 + 同目录 key”形式表面通过“非明文”，却不满足实际静态保护；或者在 headless/locked keychain 场景静默回退明文。创建、替换、删除跨两个存储时也会产生 orphan、丢 key 或引用错位。

**具体修复：** 在 PRD/NFR/AC 明确：

- 首期支持平台及各平台 approved secure store；安全存储不可用/锁定/拒绝访问时失败关闭，禁止回退普通文件或 SQLite 明文；
- Profile 通过稳定、不可猜测、与显示名称解耦的 credential handle 关联；rename/copy 不隐式共享 key；
- create/replace/delete 是可恢复的两阶段或等价原子流程，定义取消、进程崩溃、磁盘满、vault locked 后的行为和 orphan 清理；
- 通过实际存储检查、应用重启、OS 用户隔离、backup/export 不含 secret、vault locked、replace rollback、delete 等验收；
- 内存值使用 secret wrapper/zeroize，禁止 `Debug`/`Display`/serde，测试使用运行时注入 canary 而非提交 fixture。

### SEC-PROV-06 — 模型级门禁证据的判定和新鲜度不足

**严重度：Medium**

**证据：** FR-8 要求探测 Tool Calls、非空 Tool Call IDs、多轮 Tool Result和“已知上下文限制”，NFR-9 只在 Profile 或 Credential 变化后使结果失效。未覆盖模型 alias 在 Provider 侧漂移、`liter-llm` 升级、probe contract 版本变化、时间过期、非确定性 Tool Call 不触发，以及上下文上限究竟来自可信元数据还是网络压力测试。

**影响：** Ready/Compatible 状态可能在底层模型或 adapter 变化后长期陈旧；一次非确定性失败也可能误判兼容性。用超长请求探测 context limit 还可能带来成本、泄露或限流。

**具体修复：** 定义 evidence key：`provider + canonical model ID + endpoint origin + profile version + credential version + liter-llm version + probe suite version + observed_at`。依赖/endpoint/model alias/credential/probe version 变化即失效；可增加时间 TTL。把“上下文限制”拆成可信发布元数据/用户显式配置的已知上限，不用业务数据或破坏性压力探测推算。Tool probe 使用固定无业务数据、强制 tool choice（若 Provider 支持）和稳定的通过/失败/暂不可验证状态，并明确 transient error 不等于 incompatible。

### SEC-PROV-07 — retry/timeout 总预算与依赖重试叠加未定义

**严重度：Medium**

**证据：** FR-5 把 `timeout`、`retry` 列为通用参数；Addendum §3.2 和 §7 已承认 `liter-llm` 内建重试与 YS retry 可能叠加，但 PRD 没有可验收的总 attempts、总 elapsed time、取消传播和非幂等行为边界。

**影响：** 一次 Doctor 或 Query 可能实际执行 `(YS retries + 1) × (library retries + 1)` 次，增加费用、限流和不一致风险；TUI 取消后底层请求仍可能继续。

**具体修复：** 将 retry/timeout 定义为传输策略而非模型语义；指定唯一重试所有者或严格总预算（最大总 attempts、总 wall-clock、退避、可重试错误集）。明确 Tool/流式请求何时不可重试以及取消必须中止未开始的 retry。为每个目标 Provider 的认证失败、429、5xx、timeout 和 cancel 验证实际 request count。

### SEC-PROV-08 — 本地删除不等于远端撤销或介质级安全擦除

**严重度：Medium**

**证据：** FR-6 使用“安全删除”措辞，但 API Key 仍可能在 Provider 端有效，也可能存在于 OS vault backup/journal 或 SSD wear-leveling 中。

**影响：** 用户可能误以为删除 Profile 已使凭证失效；失窃后的 key 仍可远程使用。实现也无法诚实承诺介质级不可恢复擦除。

**具体修复：** 将产品语义改为“从 YS credential store 删除并使 handle 不可解析”，不要承诺物理安全擦除；删除确认明确“不会自动撤销 Provider 侧 key”，给出到 Provider 控制台 rotate/revoke 的操作提示。若未来支持 OAuth，再单独定义远端 revoke 的 best-effort 结果。

### SEC-PROV-09 — 错误清理应以结构化 allowlist 为主

**严重度：Low**

**证据：** NFR-2 和 FR-14 要求清理原始错误，这是必要但偏“黑名单替换”的表述。Provider 可能在 message、headers、URL query、nested JSON 或 debug source chain 中回显 secret/request body。

**影响：** 单纯正则 redact 已知 key 格式容易漏掉无固定前缀的 token、Bearer header、query credential 或客户数据。

**具体修复：** 要求用户可见/日志错误从内部稳定错误枚举和批准字段重新构造；原始 body/header/URL query/source chain 默认不出进程边界。Canary 测试覆盖嵌套 JSON、header、URL、Unicode/编码变体和 `Debug` source chain，并验证 TUI、日志、Telemetry、panic/crash path。

## 剩余 Critical/High

无。SEC-PROV-01 至 SEC-PROV-05 均已关闭；Medium/Low findings 保留为后续质量收敛建议。
