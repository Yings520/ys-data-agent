# Requirements Document

## Introduction

`datasource-management` 为 v0.2 Trustworthy Query Runtime 增加真实、受治理的本地数据源连接 Profile 管理。Data Engineer 与技术型分析师可以从 TUI 查看、创建、编辑、验证、选择和删除多个连接，并明确区分当前 Session 使用的数据源与 Workspace 默认数据源。v0.2 的正式支持范围是 SQLite、PostgreSQL 与 DuckDB；三者都必须完成真实连接、只读门禁、能力验证和 Query Run 绑定，不能只出现在选择列表中。

该 Feature 参考已审核的外部数据源实现及其配置、Connector Registry、`DBManager`、`DatasourceApp`、`DatasourceCommands` 和契约测试，采用“配置驱动 + Connector 注册元数据 + Manager 生命周期 + 上层统一能力”以及 Datasource/Database 分离思路。YS Data Agent 同时保持自身的 Secret 隔离、原子持久化、不可变 Run 绑定和 fail-closed Query 契约，并修正参考实现中明文 Credential、仅按名称集合缓存、失败回滚不足和无治理运行时安装的风险。

未来新增 Connector 将存放在独立 Connector Adapter Repository，并由产品从受信 Catalog 自动发现，再经过签名、摘要、兼容性、来源和撤销门禁安全安装与激活。本 Feature 只保留可演进的数据与责任边界；远程发现、Installer、外部 Adapter Host、自动升级和撤销分发不属于本期实现。

## Upstream Product Source

- **BMAD PRD**: `docs/PRD.md`
- **Source revision**: 2026-09-03 Approved 工作树版本，基于 commit `02830a7a9535c3e5115416303a9a2b0c21fe5153`
- **Covered PRD sections**: §2.3、§5、§7.1、§12.4、§13.4、§16.3、§22、§23、§26.1、§26.3—§26.6、§26.9、§27「v0.2 当前扩展：本地数据源 Profile 管理」与「Connector Adapter Ecosystem」、§28、§29

> 产品目标、v0.2 发布边界、稳定架构或演进顺序的变化必须先回到 `docs/PRD.md` 协调。本文件只定义本 Feature 的可观察行为。

## Boundary Context

- **In scope**: SQLite、PostgreSQL、DuckDB Connector；本地多个 Datasource Profile；Workspace 默认与当前 Session 选择；配置字段与能力目录；CredentialReference；Draft/Ready/Invalid 状态；连接、只读与能力验证；原子保存和激活；Connector 生命周期；`/datasource` 键盘管理闭环；新 Run 不可变数据源绑定；Doctor、Policy、Context 与 Query 回归证据。
- **Out of scope**: SQLite/PostgreSQL/DuckDB 之外的当前支持承诺；远程 Connector Catalog；自动下载、安装、升级或撤销 Adapter；外部 Adapter Host；任意动态库或源码执行；数据写入；自动发现数据库；数据摄取；Workspace Bootstrap；Starter Data Stack；Web/API 管理入口；多用户控制平面。
- **Adjacent expectations**: `tui-interaction` 提供统一可搜索选择器和原型壳层；既有 Query Runtime、Policy、Tool Runtime、QueryBudget、Context、Artifact、Event 与 Completion Gate 保持权威。本 Feature 不重新定义业务 Workflow，也不以数据源切换扩大数据权限。
- **Future design commitment**: `design.md` 必须说明当前注册元数据、版本化配置契约、Capability、Manager 与 Profile binding 如何接入未来受信 Connector Catalog；必须区分自动发现、安装与激活，并覆盖签名、摘要、兼容性、来源、撤销、契约测试、隔离与回滚，但不得为这些延期能力创建本期实现任务。

## Requirements

### Requirement 1: Connector Catalog 与正式支持范围

**Objective:** 作为技术用户，我希望看到真实且可验证的 Connector 支持目录，以便只配置当前产品能够安全运行的数据源。

#### Acceptance Criteria

1. The YS Data Agent shall 在 v0.2 Connector Catalog 中列出且仅标记 SQLite、PostgreSQL 与 DuckDB 为正式 Supported 数据源类型。
2. The YS Data Agent shall 从真实 Connector 注册元数据取得类型标识、显示名称、配置字段、Secret 字段、能力、版本和支持状态，不得由 TUI 维护另一份静态类型清单。
3. The YS Data Agent shall 区分 Adapter 已注册、配置不完整、验证失败、Ready、版本不兼容和不受支持状态。
4. If Connector 只存在代码、依赖或本地文件但没有当前发布契约与验证证据, the YS Data Agent shall 不将其标记为 Supported 或允许激活。
5. The YS Data Agent shall 使用 Capability 声明判断 Catalog、Preflight、Query 与 Freshness 等可用行为，不得仅按数据库类型名称推断能力。
6. The YS Data Agent shall 将用户命名的 Datasource Profile 与其物理 catalog、database、schema 或本地数据库文件 Context 保持为不同概念。
7. If Catalog 无法加载或注册元数据冲突, the YS Data Agent shall fail closed、保持原活动配置并显示可执行诊断，不得回退到内置假数据。

### Requirement 2: Datasource Profile 生命周期

**Objective:** 作为技术用户，我希望管理多个有版本的连接 Profile，以便安全保存和复用不同数据库配置。

#### Acceptance Criteria

1. The YS Data Agent shall 允许用户创建、查看、编辑和删除多个本地 Workspace Datasource Profile。
2. The YS Data Agent shall 要求 Profile 使用 Workspace 内唯一且非空的名称，并记录 Connector 类型、非敏感配置、CredentialReference、Database Context、状态和 revision。
3. When 用户保存尚未完成或尚未通过连接验证的 Profile, the YS Data Agent shall 将新 revision 保存为 Draft 且禁止激活。
4. When 用户编辑已激活 Profile, the YS Data Agent shall 创建未激活的新 revision，并保持原 Ready revision 活动，直到新 revision 验证并显式激活。
5. If Profile 名称冲突、Connector 类型缺失、配置字段无效或 Database Context 不合法, the YS Data Agent shall 显示字段级可修复错误且不产生部分 revision。
6. When 用户取消创建或编辑, the YS Data Agent shall 保持已保存 Profile、当前 Session 数据源和 Workspace 默认数据源不变。
7. If 保存、编辑或删除的持久化提交失败, the YS Data Agent shall 保持操作前的完整 Profile、Credential 关联、当前选择和 Connector 生命周期状态。
8. When 用户删除当前 Session 或 Workspace 默认 Profile, the YS Data Agent shall 要求先选择替代 Profile，或明确确认进入无可用数据源且不能启动 Query Run 的状态。
9. If 非终态 Run 仍绑定某 Profile revision, the YS Data Agent shall 阻止删除该 Run 恢复所需的 revision 或 Credential，并说明等待完成或取消 Run 的操作。
10. The YS Data Agent shall 在每个 Workspace 最多维护一个默认 Datasource Profile，并允许每个 Session 最多维护一个当前 Datasource Profile。

### Requirement 3: Credential 安全与 Profile 隔离

**Objective:** 作为技术用户，我希望数据库 Credential 与普通配置分离，以便安全连接且不会在 Profile 之间泄露或串用。

#### Acceptance Criteria

1. Where Connector 配置包含密码、Token、私钥口令或完整 DSN, the YS Data Agent shall 将秘密值保存到受保护 Credential 存储，并在 Profile 中只保存 CredentialReference。
2. When Credential 已保存, the YS Data Agent shall 只显示已保存、缺失、过期或遮蔽状态，不重新显示完整秘密值。
3. The YS Data Agent shall 使每个 Datasource Profile 的 Credential 关联明确且隔离，不得因 Connector 类型或字段名称相同而跨 Profile 复用秘密值。
4. When 用户编辑非 Secret 字段但没有替换已保存 Secret, the YS Data Agent shall 保持原 CredentialReference，不使用遮蔽占位符覆盖真实 Credential。
5. When 用户替换或删除 Credential, the YS Data Agent shall 原子更新 Credential 关联、使旧验证证据失效并淘汰依赖旧 Credential 的 Connector。
6. If 受保护 Credential 存储不可用或保护级别无法确认, the YS Data Agent shall 拒绝保存秘密值且不得降级为普通配置、环境变量回写或明文 DSN。
7. The YS Data Agent shall 不把完整 Credential、DSN 或私钥内容写入普通 Profile、Run binding、Event、Artifact、日志、Telemetry、错误、TUI 列表或测试 fixture。
8. While 并发验证、Query Run 或连接重试正在发生, the YS Data Agent shall 保持不同 Profile revision 和 Credential generation 的隔离。

### Requirement 4: 元数据驱动的连接配置

**Objective:** 作为技术用户，我希望配置界面随 Connector 的真实字段和能力变化，以便未来新增 Adapter 时无需重写 TUI 流程。

#### Acceptance Criteria

1. When 用户添加 Datasource Profile, the YS Data Agent shall 先展示当前 Catalog 中可配置的 Connector 类型，再根据所选 Connector 注册元数据展示配置字段。
2. The YS Data Agent shall 为配置字段显示名称、必填状态、输入类型、Secret 状态、默认值和可用约束，并在提交前校验用户输入。
3. Where Connector 提供 Adapter-specific 字段, the YS Data Agent shall 根据该 Connector 的版本化配置契约保存和验证字段，不要求用户把字段拼进自由文本 DSN。
4. When 用户编辑 Profile, the YS Data Agent shall 显示已保存的非敏感字段和遮蔽的 Secret 状态，并保持 Connector 类型与配置契约版本明确。
5. The YS Data Agent shall 允许 SQLite Profile 选择已存在且可读的数据库文件，并不得因验证或 Query 打开而创建缺失文件。
6. The YS Data Agent shall 允许 PostgreSQL Profile 配置受支持的主机、端口、database、schema、用户名和 Credential 关联，或等价的受保护连接引用。
7. The YS Data Agent shall 允许 DuckDB Profile 选择已存在且可读的数据库文件，并明确展示只读、扩展加载、外部访问和文件写出限制。
8. If Connector 配置契约版本与已保存 Profile 不兼容, the YS Data Agent shall 将 Profile 标记为需要修复或迁移且禁止激活，不得静默丢弃未知字段。

### Requirement 5: 连接、只读与能力验证

**Objective:** 作为技术用户，我希望在激活前验证连接与实际能力，以便无效或越权数据源不会进入 Query Runtime。

#### Acceptance Criteria

1. When 用户请求本地配置验证, the YS Data Agent shall 在不建立数据库连接的情况下检查名称、必填字段、字段范围、Connector 版本、Credential 关联和 Database Context。
2. If 本地配置验证失败, the YS Data Agent shall 显示字段级错误且不发起网络或文件连接。
3. When 用户请求连接验证, the YS Data Agent shall 使用不读取客户业务行的安全探测检查可达性、认证、目标 Database Context、数据库侧只读状态和声明能力。
4. When 连接验证完成, the YS Data Agent shall 关闭临时验证连接，除非同一已验证 revision 被后续受治理激活流程安全接管。
5. If 验证遇到认证失败、目标不存在、文件不可读、权限不足、非只读身份、超时、网络错误、版本不兼容、能力不足或协议错误, the YS Data Agent shall 返回稳定分类和可执行修复动作并保持 Profile 未激活。
6. When Profile 配置、Credential generation、Connector 版本、Database Context 或相关安全 Policy 发生变化, the YS Data Agent shall 使旧验证结果失效。
7. The YS Data Agent shall 只把同时通过当前配置、连接、只读、Capability 和 Policy 验证的 Profile revision 标记为 Ready。
8. The YS Data Agent shall 为 Ready revision 保存不含秘密值的验证证据，包括被验证的 revision、Connector 身份与版本、Capability 摘要、Database Context 和验证时间。
9. If 验证无法证明只读或必要能力, the YS Data Agent shall fail closed，且不得通过减少 Tool、跳过 Preflight 或允许自由文本模型回答来规避门禁。

### Requirement 6: Connector 创建、路由与生命周期

**Objective:** 作为运行时操作者，我希望 Connector 由统一生命周期管理，以便配置变化不会误用旧连接或泄漏资源。

#### Acceptance Criteria

1. The YS Data Agent shall 按需创建 Connector，不得仅为列出 Profile 而连接全部数据源。
2. When Query Run 需要 Connector, the YS Data Agent shall 根据不可变 Profile revision、Credential generation 和 Database Context 解析对应 Connector。
3. The YS Data Agent shall 将 Connector 复用身份绑定到 Profile revision 或等价脱敏配置指纹以及 Database Context，不得仅使用 Profile 名称或名称集合作为缓存身份。
4. When Profile 配置、Credential、Connector 版本、Database Context、验证状态或安全 Policy 变化, the YS Data Agent shall 停止把旧 Connector 分配给新 Run，并在安全时关闭和淘汰它。
5. When Profile 被删除、应用退出或 Connector 被判定不可用, the YS Data Agent shall 关闭由 Manager 持有且不再被有效 Run 使用的连接或连接池。
6. While 多个 Run 并发使用同一有效 Connector revision, the YS Data Agent shall 遵守 Connector 声明的并发和连接池限制，不得跨 Workspace、Profile revision 或 Credential generation 串用连接。
7. If Connector 创建或缓存替换失败, the YS Data Agent shall 保留仍有效的旧活动 Connector 供其已绑定 Run 使用，并阻止新 revision 激活。
8. The YS Data Agent shall 通过统一 Catalog、Preflight、Query 与 Freshness 能力调用 Connector，不得让 Workflow 依赖 SQLite、PostgreSQL 或 DuckDB 的具体驱动类型。

### Requirement 7: `/datasource` TUI 管理闭环

**Objective:** 作为键盘用户，我希望在统一 TUI 内完成数据源管理，以便无需编辑环境变量或配置文件。

#### Acceptance Criteria

1. When 用户从 Slash Command 面板执行 `/datasource`, the YS Data Agent TUI shall 打开标题明确的数据源选择与管理流程。
2. The YS Data Agent TUI shall 列出已保存 Profile 的名称、Connector 类型、Ready/Needs setup/Invalid 状态、当前 Session 标记和 Workspace 默认标记，并包含新增入口。
3. When 用户输入搜索文本, the YS Data Agent TUI shall 实时过滤 Profile，并优先显示名称或 Connector 类型的前缀匹配。
4. When 用户按上移键、下移键、Page Up、Page Down、Home 或 End, the YS Data Agent TUI shall 在当前候选中移动唯一高亮并保持其可见。
5. When 用户在 Ready Profile 上按 Enter, the YS Data Agent TUI shall 发起当前 Session 数据源切换。
6. When 用户在 Profile 上进入 Actions, the YS Data Agent TUI shall 提供编辑、重新验证、删除和设为 Workspace 默认的适用动作。
7. When 用户选择新增, the YS Data Agent TUI shall 在同一交互流程内完成 Connector 类型选择、元数据驱动配置、Secret 输入、验证、保存和可选激活。
8. When 用户在存在父级的管理状态按 Esc, the YS Data Agent TUI shall 返回上一层且不提交未完成操作。
9. When 用户在 `/datasource` 顶层按 Esc, the YS Data Agent TUI shall 返回调用前视图且不改变已保存或活动状态。
10. While 配置保存、连接验证或激活正在进行, the YS Data Agent TUI shall 保持渲染可响应、显示进行中状态并允许安全取消仍可取消的动作。
11. If 操作失败, the YS Data Agent TUI shall 保留已输入的非敏感字段、显示可修复错误并提供返回编辑或重试入口。
12. The YS Data Agent TUI shall 在每个层级显示当前键盘操作，并允许不使用鼠标完成全部必需步骤。
13. The YS Data Agent shall 将现有 `/connections` 静态摘要入口收敛为 `/datasource` 的兼容别名或明确导航，不得维护第二套连接状态界面。
14. The YS Data Agent TUI shall 在自身界面内完成选择和配置，不得把必需步骤降级为 Shell、独立行输入提示或仅供阅读的静态列表。

### Requirement 8: 当前选择、默认值与 Run 不可变绑定

**Objective:** 作为技术用户，我希望切换后立即看到并真实使用新数据源，同时不改变进行中的 Run。

#### Acceptance Criteria

1. When 用户选择 Ready Profile, the YS Data Agent shall 原子更新当前 Session 数据源，并立即刷新 Header 中的 Profile、Connector 类型和 Database Context。
2. When 用户设置 Ready Profile 为 Workspace 默认, the YS Data Agent shall 在没有显式 Session 选择的新 Session 中使用该 Profile。
3. If Profile 为 Draft、Invalid、未验证、验证已失效或不满足当前 Policy, the YS Data Agent shall 拒绝选择或设为默认并保持原状态。
4. When 切换成功后启动新 Run, the YS Data Agent shall 真实绑定所选 Profile revision、Connector 身份与版本、Credential generation、Database Context 和 Capability 指纹。
5. While Run 正在进行或等待恢复, the YS Data Agent shall 保持其数据源 binding 不受 Session 切换、默认值变化、Profile 编辑或新验证结果影响。
6. If 切换持久化、Connector 解析或状态刷新失败, the YS Data Agent shall 保持 TUI、Session 选择、Workspace 默认和 Runtime 对原数据源的显示与使用一致。
7. When 应用重启, the YS Data Agent shall 恢复已保存 Profile 与 Workspace 默认，并只恢复仍可验证的当前 Session 选择。
8. If 当前没有 Ready 数据源, the YS Data Agent shall 明确显示未配置状态并阻止启动 Query Run，不得静默使用 fixture、环境默认或其他 Profile。
9. If 当前数据源调用失败, the YS Data Agent shall 返回明确失败且不得自动切换 Profile、扩大数据范围或降低只读和能力门禁。
10. When 当前 Session 的数据源切换成功, the YS Data Agent shall 保留该 Session 的对话、聚焦 Task 与已有 Artifact，只让之后启动的新 Run 使用新 binding。

### Requirement 9: SQLite、PostgreSQL 与 DuckDB 安全契约

**Objective:** 作为数据责任人，我希望每个受支持 Connector 都执行数据库特定的只读防线，以便数据源管理不会削弱 Query 安全。

#### Acceptance Criteria

1. The YS Data Agent shall 对 SQLite、PostgreSQL 与 DuckDB 应用共同的单语句 AST、Source ACL、QueryBudget、结果大小、敏感字段和超时门禁。
2. When SQLite Connector 打开数据库, the YS Data Agent shall 使用不会创建或修改目标文件的只读模式，并拒绝 Attach、扩展加载、文件写出和其他 Policy 禁止行为。
3. When PostgreSQL Connector 执行 Query, the YS Data Agent shall 使用最小权限身份和 read-only transaction，并验证目标 database/schema 与 Profile binding 一致。
4. When DuckDB Connector 打开数据库, the YS Data Agent shall 使用只读模式，默认禁止扩展自动安装或加载、外部网络访问、Attach、Copy/Export 和当前 Policy 未允许的文件系统访问。
5. If SQLite 或 DuckDB 文件不存在、不是普通可读文件、超出配置允许的路径范围或会因连接而被创建, the YS Data Agent shall 拒绝验证和激活。
6. If PostgreSQL 服务无法证明数据库侧只读状态或最小权限, the YS Data Agent shall 拒绝验证和激活。
7. If Query 尝试通过数据库函数、扩展、URI、Attach、Copy、Export 或其他方言能力绕过只读与数据范围策略, the YS Data Agent shall 在执行前拒绝并记录非敏感 Policy 原因。
8. The YS Data Agent shall 不因某 Connector 声明 SideEffect::None 而自动重试成本未知、状态未知或超出当前 QueryBudget 的调用。

### Requirement 10: 数据源关联的 Policy、Context 与 Doctor

**Objective:** 作为技术用户，我希望数据源切换同步刷新治理和上下文状态，以便查询不会混用上一数据源的权限或元数据。

#### Acceptance Criteria

1. When 当前数据源变化, the YS Data Agent shall 为之后的新 Run 重新解析该 Source 对应的 ACL、Result Policy、Connector Capability、Schema/Freshness Context 和 QueryBudget。
2. The YS Data Agent shall 使用 SourceId 与 Profile revision 关联数据源相关 Policy、Context Evidence 和 Query Artifact，不得仅依赖可变显示名称。
3. If 新数据源缺少允许范围、只读能力或 Query 所需 Context, the YS Data Agent shall 阻止受影响的 Query 或请求澄清，不得沿用上一数据源的授权或 Evidence。
4. When 用户运行 Doctor, the YS Data Agent shall 检查当前或指定 Profile 的配置、Credential、连接、只读状态、Capability、Policy、Metric、dbt 与 Freshness，并提供不含秘密的阻断项和修复动作。
5. While 旧 Run 继续执行, the YS Data Agent shall 保持其原 Source Policy、ContextManifest 和 Connector Capability 证据不变。
6. When QueryArtifact 由新数据源产生, the YS Data Agent shall 记录真实 Source、Database Context、非敏感 Connector binding、Policy 和 VerificationReport。

### Requirement 11: 原子性、错误语义与发布证据

**Objective:** 作为用户与维护者，我希望数据源管理在失败和并发情况下保持一致，并有真实回归证据证明三种 Connector 可用。

#### Acceptance Criteria

1. If 创建、编辑、验证、设为默认、选择或删除发生并发 revision 冲突, the YS Data Agent shall 拒绝陈旧写入、保留胜出状态并提示用户刷新。
2. If 进程在 Profile、Credential 或活动选择更新期间中断, the YS Data Agent shall 在恢复时得到操作前或操作后的完整状态，不得出现部分 Profile、悬空 Credential 或指向不存在 revision 的选择。
3. When Connector 错误进入 TUI、Event、日志或 Telemetry, the YS Data Agent shall 规范化错误类别并清理 Credential、完整 DSN、受限路径、业务数据和驱动回显的敏感值。
4. The YS Data Agent shall 保持数据源管理状态与 Task/Run 权威事件、业务数据库和 Artifact Store 的持久化职责分离。
5. The YS Data Agent shall 要求 SQLite、PostgreSQL 与 DuckDB 分别通过共享 Connector 契约、只读拒绝、连接生命周期、错误分类和真实 Query 集成证据后才允许发布本 Feature。
6. The YS Data Agent shall 要求 `/datasource` 的真实键盘序列、搜索、CRUD、验证、失败回滚、当前/默认标记、重启恢复和 Header 刷新通过自动化回归证据后才允许发布本 Feature。
7. The YS Data Agent shall 要求新 Run 使用新数据源、旧 Run 保持原 binding、Profile/Credential 变化淘汰旧 Connector，以及 Secret canary 不进入持久化和输出的回归证据通过后才允许发布本 Feature。
8. The YS Data Agent shall 保持数据源相关 Credential 泄露事件为 0、越权写入为 0、严重静默切换错误为 0 的发布门槛。
