# PRD Quality Review — YS Data Agent 统一 LLM Provider 管理

## Overall verdict

**有条件通过。** 这是一份与开发阶段技术能力建设相匹配的 PRD：统一接入层的产品边界清楚，TUI 被正确放在管理入口的位置，17 项 FR 基本都有可验收后果，且对 Provider 能力差异、无静默回退和 Query 治理不回归有明确立场。进入 cc-sdd 前仍需解决三类高优先级问题：当 9 个目标 Provider 中存在无法通过能力门禁者时如何决策、四项核心假设/开放问题如何正式收口，以及“本地安全凭证存储”的最低可验证安全标准；否则下游可能在范围、持久化边界和完成定义上做出不同解释。

## Decision-readiness — thin

PRD 已明确作出多项真实决定：只覆盖 9 个目标 Provider、`chatgpt/` 不等于 `openai/`、直接替换旧实现、不迁移旧配置、不做自动回退，并坚持模型级能力门禁而非只相信 Provider 注册表。这些决定足以防止 cc-sdd 把项目扩展成 165 Provider、通用 TUI 改造或双实现迁移工程。

但当前最重要的产品张力还没有被裁决：文档同时要求 9/9 Provider 全部标记 Supported，又要求任何缺少 Tool Calls、Tool Call IDs、多轮 Tool Result 或上下文能力的模型不得激活。若某个目标 Provider 没有能通过门禁的代表性模型，PRD 没有说明是阻断整个版本、将该 Provider 从首期移出，还是允许 Catalog 中存在非 Supported 目标项。另有多个直接影响配置域和运行语义的 `[ASSUMPTION]`，而文档已进入“批准/交付 cc-sdd”阶段，不能继续让下游替产品做决定。

### Findings

- **high** 9/9 支持承诺缺少失败决策路径（§4.1 FR-1/FR-2、AC-1、AC-11、SM-1）— “9/9 标记 Supported”与“模型能力校验失败则不得激活”各自合理，但组合后可能形成无法完成的发布门槛；没有规定某一 Provider 经真实探测不兼容时的范围变更权和发布判定。*Fix:* 明确 9/9 是不可降级的 release blocker，还是“9 个候选目标、仅通过者标记 Supported”；若前者，写明失败即不发布及范围变更需产品重新批准；若后者，同步修改 AC-1 与 SM-1。
- **high** 已批准文档仍把核心产品行为留作假设（§4.2、FR-5、FR-10、FR-11、§13）— 多 Profile/单活动项、切换只影响新 Run、无自动回退均直接塑造状态模型与验收，不应由 cc-sdd 自行确认。*Fix:* 将用户已认可的项改为正式决定；未确认项移入开放问题并在进入 requirements 前逐项裁决。
- **high** 配置作用域尚未决定（§12 OQ-1）— 全局本地配置还是 Workspace 隔离会改变 Profile 唯一性、Credential 关联、活动指针、TUI 列表和 Run 复现语义，是下游设计的分叉点。*Fix:* 在 PRD 中选定首期作用域，或明确默认值、决策 owner 和必须在 cc-sdd design 前触发的裁决条件。

## Substance over theater — strong

内容总体是“能力约束”而非模板家具。两条用户旅程分别承担首次配置与运行中切换的关键行为，没有堆砌 persona；愿景直接关联当前自有 OpenAI-compatible 实现和 `liter-llm` 的统一路由能力；NFR 也围绕本项目特有的凭证泄露、Profile 原子性、Provider 指纹、能力探测失效和版本漂移展开。Addendum 将依赖源码事实、重试叠加风险、凭证上下文复用风险和模型发现边界留给下游，避免把实现细节伪装成产品需求。

未发现明显的 persona theater、innovation theater 或无关章节填充。性能与可用性部分虽有少量抽象措辞，但不是通用“高性能/高可用”口号，均能追溯到 TUI 网络校验和模型发现降级路径。

## Strategic coherence — adequate

PRD 的 thesis 清楚：以 `liter-llm` 取代 YS 自有 Provider 协议维护成本，同时在 YS 自己的模型能力与治理门禁之上提供一致的 Provider 管理体验。首期范围、非目标、FR、验收标准与 Addendum 基本沿着这条主轴展开；TUI 没有喧宾夺主，旧配置迁移也因“开发阶段、无存量用户”被诚实移除。成功指标还配有反指标，明确禁止用 Provider 数量、切换速度或参数表面一致性牺牲兼容质量。

不过主要成功指标几乎都是交付测试指标，能证明“功能做完且未回归”，尚不能直接证明 PRD 开头所称的“减少重复适配、降低升级成本”。对于当前无用户的开发阶段，这不构成发布阻塞，但会让依赖替换的核心投资理由无法在完成后复盘。

### Findings

- **medium** 降低维护成本的产品目标没有对应成功定义（§1.1、§10）— SM-1 至 SM-5 衡量覆盖、安全、契约与 TUI 闭环，却没有验证厂商专属 YS 代码/配置分支是否真正减少，或新增 Provider 是否只需 Catalog/配置而无需新协议实现。AC-2 与 AC-12 只部分覆盖。*Fix:* 增加一个可审计的结构性成功条件，例如活动路径中厂商专属 YS 协议实现为 0，且新增一个 `liter-llm` 原生 Provider 不要求新增 YS 请求/响应协议 adapter；避免使用难以基线化的工时百分比。

## Done-ness clarity — adequate

17 项 FR 均附带可验收结果，12 项 AC 能覆盖主要闭环：Provider 范围、Profile、凭证隔离、能力门禁、原子切换、失败分类、TUI 闭环、Query 回归和旧路径退出。相较一般技术 PRD，完成定义已经相当具体，尤其“不能静默丢弃参数”“不重新显示完整凭证”“Run 指纹不被后续编辑覆盖”等结果可直接转化为 requirements 与测试。

主要不足在安全和模型兼容边界：`本地安全凭证存储`、`最短生命周期`、`安全取消`仍缺少可判定的最低标准；“已知上下文限制”也没有定义兼容阈值。Addendum 把具体机制正确留给 design，但产品层仍需说明什么证据足以宣告安全与兼容，否则不同平台或不同测试人员可能得出不同结论。

### Findings

- **high** 凭证安全要求缺少最低验收基线（FR-6、NFR-1 至 NFR-3、AC-4）— “本地安全”“最短生命周期”“一并安全删除”和“等价威胁模型”没有定义支持平台、攻击边界、降级规则或验收证据；仅证明磁盘上不是明文，并不足以证明安全。*Fix:* 在 PRD 定义产品最低线：首期支持的平台及首选系统凭证库、凭证库不可用时必须失败关闭还是允许经批准的加密后备、日志/崩溃转储 canary 测试、删除后的不可访问语义；具体库与 schema 仍留给 design。
- **medium** 上下文能力门禁没有可计算阈值（FR-8、AC-5）— “已知上下文限制”被列为激活前必须覆盖的能力，但没有说明模型上下文至少要满足什么，或如何与 QueryBudget/最大工具轮次关联。*Fix:* 定义一个产品规则，例如模型声明/探测的有效上下文必须覆盖当前 QueryBudget 的最大请求窗口；若无法可靠验证，规定状态和能否激活。
- **medium** 取消、超时与重试的完成语义不足（FR-5、NFR-11、Addendum §3.2/§7）— PRD 要求网络校验可安全取消，并暴露 timeout/retry，但没有说明取消后 Profile 状态、在途请求处理、库级与 YS 级重试叠加上限。*Fix:* 增加用户可观察结果：取消不得激活或写入成功探测结果，超时/重试采用单一有效预算且向 TUI 报告最终类别；精确机制交由 design。

## Scope honesty — adequate

非目标和 MVP/后续范围做了大量实质工作：明确排除 165 Provider 全覆盖、OpenAI API、非 Chat 模态、自动路由、多租户控制平面、通用 TUI 改造、旧配置迁移和 v0.2 Query 扩张。开发阶段直接替换而无兼容窗口与上游已确认事实一致，没有为了显得“生产化”而编造用户迁移工作。

假设索引的 roundtrip 完整，但开放问题没有 owner、截止点或默认方案，其中至少作用域与代表性模型/测试凭证会影响 cc-sdd 能否安全完成 requirements、design 与 validation。文档既然已获批准，应把这些内容分成“阻塞下游的产品决策”和“可由 design 决定的执行项”，而不是保留一个平面问题列表。

### Findings

- **high** 开放问题没有阻塞级别、owner 或回访条件（§12）— OQ-1 改变配置域，OQ-3 决定 9/9 Provider 验收是否可执行；OQ-2/OQ-4 则更适合设计或发布策略。平铺四项会让 cc-sdd 不知道哪些可以合理默认。*Fix:* 在最终化时逐项选择“现在决定”或“明确延期”；延期项记录 owner、最迟决策阶段、默认行为和触发条件，至少在 requirements 前关闭 OQ-1，在 validation 计划前关闭 OQ-3。
- **medium** “目标 Provider”与“Supported Provider”的集合语义不够稳定（术语表、FR-1、FR-2、AC-1）— 术语表说目标 Provider 是本期“承诺实现和验收”的 9 个，AC-1 又要求全部标记 Supported，但 FR-2 允许不兼容/暂不可验证状态。*Fix:* 在术语表分别定义 Candidate/Target 与 Supported，或者明确目标 Provider 可以处于 Unsupported，只有兼容模型通过证据门禁后才获得 Supported 状态。

## Downstream usability — adequate

本 PRD 明确以 cc-sdd 为下游，FR-1 至 FR-17、NFR-1 至 NFR-16、AC-1 至 AC-12 与 SM 编号均连续且唯一；交叉引用均能解析。术语表覆盖 Provider 领域的核心名词，两个 UJ 都有具名主角 Chen，并能单独提取配置与切换流程。Addendum 对当前代码位置和需要验证的技术风险给出了清晰入口，适合 requirements/design 做 source extraction。

下游可用性的主要摩擦是少量名词与字段模型不够稳定：`模型` 已是 Provider Profile 的独立字段，FR-5 又把“模型 ID”列入通用参数；`Task/Run`、`Doctor`、`QueryBudget`、`Provider Profile 版本`等跨域词未在本 PRD 术语表定义或明确引用上游定义。后者对熟悉代码库的人不难，但会降低单独抽取章节时的自洽性。

### Findings

- **medium** Provider Profile 字段对“模型 ID”重复建模（§4.2 字段表、FR-4、FR-5）— 模型字段已定义为 `provider/model`，通用参数又包含“模型 ID”，可能导致 cc-sdd 设计出两个来源或不清楚哪个字段参与版本/指纹。*Fix:* 将模型标识只保留为 Profile 核心字段；通用参数只列 temperature、max tokens、timeout、retry，或明确“模型 ID”只是 UI/请求映射别名而非独立参数。
- **low** 跨域术语缺少本地定义或权威引用（§1、FR-8 至 FR-10、FR-15/FR-16）— `Task/Run`、Runtime、Doctor、QueryBudget、Completion Gate、Query Artifact 等对当前团队可能熟悉，但单节 source extraction 时语义不完整。*Fix:* 在术语表补充最小定义，或明确引用 approved Product Brief 中的权威 glossary/章节；统一使用 `Run` 还是 `Task/Run`。

## Shape fit — strong

文档采用“技术能力 PRD + 精简用户旅程 + 独立技术 Addendum”的形状，适合一个仍处开发阶段、面向本地技术操作角色、即将进入 cc-sdd 的能力替换项目。它没有为单一操作角色制造大量 persona 或旅程，也没有退化成纯架构说明：Provider Profile 的用户可见状态、激活语义、错误反馈和 TUI 闭环仍在主 PRD 中，adapter、存储 schema 和组件设计则留在 Addendum/下游。

两条 UJ 足以承载首配和切换这两个真正有交互意义的场景；删除、凭证替换、无活动 Profile 与校验取消由 FR 覆盖，暂不需要为每个 CRUD 操作再造旅程。开发阶段无存量用户的事实也正确改变了形状：没有迁移阶段、弃用计划或用户兼容矩阵。

## Mechanical notes

- **ID continuity：** FR-1…FR-17、NFR-1…NFR-16、AC-1…AC-12、SM-1…SM-5 连续且无重复；反指标 SM-C1…SM-C3 单独编号，引用可解析。
- **Assumptions Index roundtrip：** 4 个 inline `[ASSUMPTION]` 均在 §13 索引，索引没有悬空项。但若这是批准后的最终稿，应将已确认内容转为正式决定，并只保留真正未决的假设。
- **UJ protagonist naming：** UJ-1 与 UJ-2 均使用具名主角 Chen，且角色上下文足够；无悬空 UJ。
- **Glossary drift：** `模型`与`模型 ID`在 Profile 字段/通用参数间重复；`Task/Run`和`Run`混用；`Supported`、`支持状态`、`兼容`之间的集合关系需要固定。
- **Cross-references：** 未发现不存在的 FR/NFR 引用；AC-4 的 NFR-1 至 NFR-3、AC-11 的 NFR-14/NFR-15 均存在。AC 并非逐一反向覆盖所有 NFR，尤其 NFR-16 主要由 AC-1/AC-11 间接覆盖，cc-sdd 可在 requirements 中补直接验证。
- **Required shape：** 对“开发阶段、无存量用户、链顶进入 cc-sdd 的技术能力 PRD”而言，愿景、用户流程、术语、FR、非目标、MVP、NFR、交付边界、AC、指标、风险、开放问题和假设索引齐备；没有不必要的迁移或多 persona 章节。
