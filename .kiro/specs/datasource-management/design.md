# Design Document — datasource-management

## Overview

为已批准的 requirements.md 提供技术设计。用户在同一 TUI 中管理 SQLite、PostgreSQL、DuckDB Profile；每个 Query Run 使用启动时已验证的数据源与治理快照。上游为 docs/PRD.md，覆盖章节沿用 requirements.md 的 Upstream Product Source。

采用 Full discovery：本 Feature 涉及三个数据库驱动、凭证与 SQLite 的恢复协议、Run 持久化契约和跨层路由。实现沿用现有 Rust 分层，只新增本期所需的数据源契约、管理服务和 DuckDB Adapter。

### Goals / Non-Goals

- 完成真实的配置、验证、选择、Query 和恢复闭环；三种 Connector 共用契约与发布门禁。
- 简化依据：名称用于显示，revision 用于配置身份，binding 用于 Run 身份；所有入口执行同一组不变量。
- 不实现新的业务 Workflow、数据写入、远程 Connector 发现或安装。未来接入边界见 Design Decisions。

## Boundary Commitments

### This Spec Owns

- Datasource Profile revision、验证证据、Credential 关联、Session 选择及 Workspace 默认。
- Connector 元数据、按 binding 解析与关闭连接、三个数据库的只读能力验证。
- 数据源管理 TUI，以及既有 Query、Context、Doctor、Artifact 消费数据源 binding 的接缝。

这些是一个垂直闭环：选择成功必须改变新 Run 的真实数据库。凭证引擎、通用选择器和 Query 状态机仍由既有模块负责。

### Out of Boundary

- Provider Profile/OAuth/模型选择行为，通用 TUI 导航与选择器设计。
- Policy 的授权规则、Metric 激活、Context 检索算法、Task/Run 状态机与 Completion Gate。
- Workspace Bootstrap、摄取、动态库加载、远程 Catalog/Installer/Host、升级与撤销分发。
- 不重构其他 Feature，不建立额外规划文档或完成状态。

### Allowed Dependencies

编译依赖方向：core 无本项目上层依赖；runtime、store、adapters 分别依赖 core；apps 作为组合根依赖四者。runtime 通过 core Ports 调用 store/adapters，禁止直接导入具体驱动。已有 Adapter 内的 SQL 方言与 Tool 实现留在 adapters，纯身份和安全判定契约放在 core。

复用 runtime 的 Query Harness、ToolRuntime、ContextAssembler、Doctor；复用 store 的命令事务和迁移；复用 adapters 的本地加密引擎、SQL AST 与结果策略。仅为本 Feature 扩展这些接缝。

### Revalidation Triggers

- CreateRunCommand、ToolExecutionContext、binding、Capability 或 ContextEvidence 的形状改变：验证所有下游和全工作区。
- 凭证引擎抽取：验证既有 Provider 文件格式、AAD、路径、权限与恢复行为不变。
- SQL 方言、数据库版本或配置契约变化：重跑三种 Connector 契约与 Query eval。
- TUI 服务返回值或选择器接口变化：重跑 Provider、Model、Datasource 键盘回归。
- 新增迁移或启动前提：验证旧 Workspace 升级、未配置启动、重启恢复与旧 Run 读取。

## Architecture

### Existing Architecture Analysis

| 已核对代码 | 事实与设计影响 |
|---|---|
| apps/ysda/src/bootstrap.rs | 当前按环境配置创建一个 SQLite/Postgres Connector，并固定 data_scope；生产组装改为注入管理服务及 Run resolver |
| crates/ys-agent-adapters/src/tools/mod.rs | ConnectorRegistry 以 SourceId 字符串保存活动句柄；不能区分同 Source 的不同 revision，生产 Tool 改为按 RunId 解析 |
| crates/ys-agent-core/src/ports.rs；store/src/sqlite.rs | CreateRunCommand 已要求 Provider binding，且与初始事件原子写入；在同一命令增加 Datasource binding |
| crates/ys-agent-runtime/src/harness.rs | data_scope、query_budget、connector_tools 来自固定 HarnessConfig；改为每个 Run 的只读执行上下文 |
| adapters/src/credential/local.rs；store/src/provider.rs | 已有 owner-only 本地加密、不可变 generation 和恢复日志；复用底层加密及事务模式，不让数据源依赖 Provider 领域类型 |
| adapters/src/data/{sqlite,postgres,sql_policy,result_policy}.rs | 已有只读执行和结果治理；补足目标校验、权限证明、关闭与 DuckDB 方言 |
| apps/ysda/src/tui/{provider_management,async_guard,navigation}.rs | 已有管理 reducer、OperationId 防陈旧回包和导航；复用交互机制，不复制 Provider 业务状态 |

### Architecture Pattern & Boundary Map

~~~mermaid
flowchart TD
    UI[Datasource TUI] --> Service[DatasourceService]
    Doctor[Doctor] --> Service
    Service --> Repository[DatasourceRepository]
    Service --> Vault[DatasourceVault]
    Service --> Manager[ConnectorManager]
    Manager --> Catalog[ConnectorCatalog]
    Catalog --> Drivers[SQLite PostgreSQL DuckDB]
    Runtime[Run Service] --> Service
    Runtime --> Store[RuntimeStore]
    Harness[Query Harness] --> Manager
    Tools[Query Tools] --> Manager
    Manager --> Store
~~~

图中箭头表示调用；跨 crate 调用均经 core Port。DatasourceService 是管理状态的唯一协调者；ConnectorManager 是连接资源的唯一持有者。Catalog 只保存元数据和 Factory，不持有数据库连接。

### Technology Stack

| 范围 | 选择 | 约束 |
|---|---|---|
| TUI / async | 现有 ratatui 0.30.2、crossterm 0.29、Tokio 1 | 沿用 Cargo.lock；阻塞数据库工作不在渲染线程执行 |
| Profile / Run store | 现有 rusqlite 0.39 bundled | 在现有 Workspace runtime SQLite 内分表；共享事务，不共享领域权威 |
| PostgreSQL | 现有 sqlx 0.9 postgres + rustls | 使用结构化 PgConnectOptions；不把 DSN 放入可序列化类型 |
| DuckDB | 新增 duckdb =1.10505.0，default-features=false，仅 bundled | 设计时核对的 Rust API 版本；锁定 Cargo.lock，使用安全 Rust wrapper，不自写 FFI |
| SQL / Secret | 现有 sqlparser 0.62、ring 0.17.14、secrecy 0.10.3、zeroize 1.9.0 | 不自建解析器、加密算法或通用插件框架 |

DuckDB 原生构建依赖 C++ 工具链。该精确版本的 API 已核对，尚未在本仓库编译验证；实现的首个驱动任务必须验证依赖解析、原生链接、许可及平台构建，并记录实际 engine version。若需换版，更新此表及版本对应证据后继续，不以未验证的替代驱动宣称 Supported。

## File Structure Plan

路径相对于仓库；花括号表示同目录的具体文件集合。新目录内 mod.rs 仅导出模块，不另设协调层。

| 动作 | 文件或模块 | 单一责任 / 对应组件 |
|---|---|---|
| 新建 | crates/ys-agent-core/src/datasource.rs | Datasource 类型、错误、字段元数据、Service/Repository/Factory/Resolver/Vault Ports |
| 修改 | crates/ys-agent-core/src/{lib,connector,ports,event,context}.rs | 导出类型；凭证引用；Run 原子绑定；Tool/Context 数据源身份 |
| 新建 | crates/ys-agent-runtime/src/datasource/{mod,service,manager}.rs | DatasourceService；ConnectorManager 与 Run resolver；管理事务协调及恢复 |
| 修改 | crates/ys-agent-runtime/src/{lib,service,harness,context_assembler,doctor}.rs | Run 创建、binding 上下文、Evidence 来源过滤与 Doctor 复用 |
| 修改 | crates/ys-agent-runtime/src/tools/{runtime,view}.rs | Tool binding 一致性与能力门禁 |
| 修改 | crates/ys-agent-runtime/src/workflow/query/{artifact,verifier}.rs | 产物记录和验证数据源 binding；不改变 Workflow 状态图 |
| 新建 | crates/ys-agent-store/src/datasource.rs | SqliteDatasourceRepository、凭证日志、选择事务与 binding 读取 |
| 新建 | crates/ys-agent-store/migrations/0007_datasource_management.sql | 本期新增表、约束、索引；实施时如编号已被占用仅顺延 |
| 修改 | crates/ys-agent-store/src/{lib,sqlite}.rs | 注册迁移；扩展既有 Run 创建事务和数据源 CAS 验证 |
| 新建 | crates/ys-agent-adapters/src/credential/{encrypted,datasource}.rs | 共享无领域加密文件引擎；DatasourceVault 适配 |
| 修改 | crates/ys-agent-adapters/src/credential/{mod,local}.rs | Provider 包装现有格式，调用抽取的加密引擎 |
| 新建 | crates/ys-agent-adapters/src/data/{catalog,duckdb}.rs | BuiltinConnectorCatalog 与 Factory；DuckDB Connector |
| 修改 | crates/ys-agent-adapters/src/data/{mod,sqlite,postgres,sql_policy,result_policy}.rs | 驱动配置元数据、probe、关闭、只读与方言治理 |
| 修改 | crates/ys-agent-adapters/src/tools/{mod,query_data,inspect_schema,read_freshness}.rs | 用 core Run resolver 替换生产 SourceId 句柄查找；DuckDB Metric 编译 |
| 修改 | crates/ys-agent-adapters/src/context/{dbt_manifest,metric_registry}.rs | 既有 Context Adapter 按绑定 Source/Context 筛选证据，防止跨源召回 |
| 新建 | apps/ysda/src/tui/datasource_management.rs | DatasourceScreen 展示状态与表单，不包含管理业务逻辑 |
| 修改 | apps/ysda/src/{bootstrap,cli}.rs；apps/ysda/src/tui/{mod,app,event_loop,input,navigation,palette,ui}.rs | 服务组装、Doctor 指定 Profile、命令别名、异步动作与 Header |
| 修改 | Cargo.toml、Cargo.lock、crates/ys-agent-adapters/Cargo.toml | DuckDB 最小依赖与锁定 |
| 新建 | crates/ys-agent-core/tests/datasource_contracts_test.rs | V1：身份、字段、状态与类型契约 |
| 新建 | crates/ys-agent-store/tests/datasource_store_test.rs | V2：事务、CAS、恢复及迁移 |
| 新建 | crates/ys-agent-adapters/tests/{datasource_connector_contract_test,datasource_secret_test}.rs | V3：三驱动共享契约；V4：凭证隔离与 canary |
| 新建 | crates/ys-agent-runtime/tests/datasource_runtime_test.rs | V5：绑定、生命周期、Policy/Context/Doctor |
| 新建 | apps/ysda/tests/datasource_tui_test.rs | V6：真实键盘闭环 |
| 修改 | scripts/v0.2-release-gate.sh、apps/ysda/tests/{doctor_test,query_eval_test,tui_test}.rs | V7：三驱动发布门禁、原有启动及 UI 回归 |

既有 tests 中 CreateRunCommand、HarnessConfig、ConnectorRegistry 的构造调用随契约迁移，只修改受影响的 fixture/builder，不改变其他 Feature 的断言。共享 selector 已落地在 apps/ysda/src/tui/palette.rs 的 Selector<T>/SelectionItem，复用它及现有 async_guard，不依赖尚不存在的独立 selection.rs，也不新建第二套选择器。

## Components and Interfaces

### Component Summary

| 组件 | 层 / 职责 | Requirements | P0 依赖 | 契约 |
|---|---|---|---|---|
| DatasourceDomain | core；身份与有效性 | 1.6, 2.2, 5.6, 8.4 | 无 | State |
| DatasourceService | runtime；管理命令和一致快照 | 2.1, 3.5, 5.7, 8.1, 10.4, 11.1 | Repository、Vault、Manager | Service |
| ConnectorManager | runtime；按绑定创建、租用和关闭 | 6.1, 6.2, 6.6, 8.5 | Catalog、Vault、binding store | Service / State |
| BuiltinConnectorCatalog | adapters；元数据与 Factory | 1.1, 1.2, 4.3, 9.1 | 三驱动、既有 SQL/Result Policy | Service |
| SqliteDatasourceRepository | store；管理状态与恢复日志 | 2.7, 2.9, 11.2, 11.4 | 既有 SQLite | Service / State |
| DatasourceVault | adapters；不可变秘密 generation | 3.1, 3.3, 3.6 | 共享加密引擎 | Service |
| Query 接缝 | 既有 runtime + tools；消费绑定 | 8.4, 10.1, 10.6 | Manager、既有 Context/Policy | Service / Event |
| DatasourceScreen | apps；键盘与脱敏视图 | 7.1, 7.14 | Service、现有选择器与 async_guard | State |

### DatasourceDomain：最小数据模型

所有 ID 使用强类型；序列化结构含 schema_version，未知配置版本不得静默转为当前版本。秘密输入无 Serialize/Debug 明文输出能力。

| 类型 | 必需内容与不变量 |
|---|---|
| DatasourceProfile | workspace_id、profile_id、来自受信 Source Policy 的稳定 SourceId、唯一显示名称、head_revision、deleted_at；名称 trim 后非空，Workspace 内采用明确的 ASCII 大小写折叠唯一键，非 ASCII 保留精确字符 |
| DatasourceRevision | profile_id、revision、adapter_id/version、config_version、非敏感字段、DatabaseContext、可选 CredentialReference/generation；保存后内容不可变 |
| DatabaseContext | catalog/database/schema 的结构化值，或已规范化文件定位；不等同于 Profile 名称。目标改变必须重新匹配 Source ACL |
| ValidationEvidence | revision、adapter/engine version、Credential generation、Context 指纹、Capability 摘要、Policy 指纹、验证时间、probe 结果 |
| RevisionState | Draft、Ready、Invalid(code)；状态与验证证据单独保存，Ready 必须证据仍匹配。未完成配置可为 Draft；已给字段非法不可保存 |
| SelectionSnapshot | workspace_id、session_id、current revision、default revision、selection_version、脱敏 Header；选择指向精确 revision |
| RunDatasourceBinding | schema_version、run_id、workspace_id、SourceId、profile/revision、adapter/version、generation、Context、Capability、验证证据摘要、Policy 快照引用及摘要 |
| RunDatasourceContext | binding + 该 Source 的 AllowedDataScope、类型化结果策略快照、QueryBudget、可用 Tool、Context 证据命名空间；不含秘密或驱动类型 |

验证状态变化只改变资格，不重写 revision 内容或历史 binding。保存普通配置 Draft 不使仍被显式选择的旧 Ready revision 自身失效；真正变更该身份的 Credential、安全 Policy 或验证资格时才停止旧身份的新分配。

Profile、验证状态和选择是不同事实：编辑只创建 head revision，旧选择仍引用原 Ready revision。新 revision 通过验证不自动替换任何 Session 或默认选择。更换 Secret 创建新 generation，旧 generation 从新 Run 候选中退休；仍被旧 Run 引用时保留，直到终态后才回收。这统一实现 2.4、3.5、6.4 与 8.5，不建立可变的“全局当前 Connector”。

CredentialReference 从仅 env:NAME 演进为显式解析的 Env 与 DatasourceVault 两种引用；生产数据源保存只接受后者，含 Workspace/Profile/generation，拒绝内嵌 URL。移除 environment_variable_name 的无条件 expect，旧环境调用改为显式匹配。ProviderCredentialReference 的格式不变。

### DatasourceService：管理 API

Inbound：TUI、Doctor、Run Service（P0）。Outbound：Repository、Vault、Manager、既有 Source Policy/Context 读取（P0）。没有远程管理 API。

~~~rust
// 签名表示异步 Port 契约；具体 async-trait 风格沿用现有代码。
trait DatasourceManagementApi: Send + Sync {
    async fn view(&self, scope: DatasourceScope) -> DsResult<DatasourceView>;
    async fn save(&self, request: SaveDatasource) -> DsResult<DatasourceDetail>;
    async fn validate(&self, request: ValidateDatasource) -> DsResult<ValidationReport>;
    async fn select(&self, request: SelectDatasource) -> DsResult<SelectionSnapshot>;
    async fn delete(&self, request: DeleteDatasource) -> DsResult<SelectionSnapshot>;
    async fn doctor(&self, request: DatasourceDoctorRequest) -> DsResult<DatasourceDoctorReport>;
}
~~~

- DatasourceScope = WorkspaceId + SessionId。所有写请求包含 CommandId、expected_version；涉及 Profile 时额外包含 expected_head_revision。
- SaveDatasource = scope + 可选 ProfileId + name + adapter/config version + NonSecretFields + DatabaseContext + SecretEdit。SecretEdit 是 Keep / Replace(SecretValue) / Remove，遮蔽字符串永不参与保存。
- ValidateDatasource = scope + 精确 revision + Local/Connection + OperationId；Local 只校验配置和本地引用状态，不打开数据库或发网络请求。Connection 先完整本地校验，再 probe，最后 CAS 写验证结果；本地失败不连接。
- SelectDatasource = scope + 精确 Ready revision + Session/WorkspaceDefault + expected_version。两种选择分别修改；Workspace 默认仅供没有显式选择的新 Session 初始化。显式 None 表示用户选择无数据源，不能再次继承默认。
- DeleteDatasource = scope + ProfileId + expected_head_revision + expected_version + Replacement(Ready revision)/ConfirmUnconfigured。事务枚举所有受影响 Session 和默认引用；任何非终态 Run 引用阻止删除。删除不影响会话内容和 Task。
- view 返回单一已提交快照，字段包含 current/default/head 各自 revision 与状态，避免新 Draft 遮盖仍活动的旧 revision。
- 配置保存可以允许缺少连接字段的 Draft，但必须有类型和合法名称；已提供的字段、Context 格式或字段版本非法则整个保存失败。Connection 模式必须齐全。
- save、delete、select 的成功响应由提交前验证的完整 DTO 构造；不存在“提交后再远程读取 Header 才算成功”的步骤。取消仅在提交前生效，越过提交点返回真实结果。响应丢失时按 CommandId 查询原回执，不补偿覆盖已提交状态。

### ConnectorManager 与 Catalog

Inbound：Service（验证/激活），Query Harness/Tools（Run 解析）（P0）。Outbound：Catalog Factory、DatasourceVault、Run binding repository（P0）；外部数据库由 Factory 隔离。

~~~rust
trait ConnectorCatalog: Send + Sync {
    fn descriptors(&self) -> DsResult<Vec<ConnectorDescriptor>>;
    fn factory(&self, id: &AdapterId, version: &AdapterVersion)
        -> DsResult<Arc<dyn ConnectorFactory>>;
}
trait ConnectorFactory: Send + Sync {
    fn validate_config(&self, input: &DatasourceRevision) -> Vec<FieldIssue>;
    async fn open(&self, input: ConnectorOpenInput) -> DsResult<Arc<dyn ManagedConnector>>;
}
trait ManagedConnector:
    CatalogReader + QueryPreflightReader + SqlQueryExecutor + FreshnessReader + Send + Sync
{
    async fn probe(&self) -> DsResult<ProbeEvidence>;
    async fn close(&self) -> DsResult<()>;
}
trait RunDatasourceResolver: Send + Sync {
    async fn resolve(&self, run_id: RunId) -> DsResult<ResolvedRunDatasource>;
}
~~~

ConnectorOpenInput = 精确 revision、短生命周期 SecretLease、已匹配的治理上下文。ResolvedRunDatasource = RunDatasourceContext + ManagedConnector 租约；所有 Tool 通过 ToolExecutionContext.run_id 获取它，并核对请求 SourceId。关闭为幂等操作，Adapter 负责等待自己的 I/O 结束和释放池。

ConnectorDescriptor 包含 adapter_id、显示名称、adapter/config/contract version、支持状态、字段列表、Capability、运行资源限制和发布证据标识。字段采用 Text/Integer/Boolean/Choice/ExistingFile/Secret 类型及 required/default/range/choices；只声明三驱动实际需要的约束，不引入表单 DSL。Secret 字段没有秘密默认值。Catalog 元数据和 Factory 来自同一注册项，重复 ID/version 使目录失败；不得跳过坏条目并继续激活。

CapabilityDescriptor 增加明确 preflight 能力和只读机制 FileReadOnly/TransactionReadOnly，保留已有能力字段兼容解释；SQLite 无只读 transaction 不代表无法只读。Registered/Supported/Incompatible/Unsupported 属于 Adapter，Draft/Ready/Invalid 属于 revision。Supported 依赖与当前二进制 adapter version 对应的发布证据；最终发布必须包含且通过三驱动。

缓存键 = Workspace + Profile/revision + generation + DatabaseContext 指纹 + adapter version + Policy/Capability 指纹及 validation_id。相同键的并发创建合并为一次；失败不缓存成功句柄。不按名称、SourceId 或数据库类型复用。连接上限采用 descriptor 与 Policy 的较小值；队列等待计入预算。

验证连接在 probe 完成后关闭，不做验证连接接管优化。激活另外准备并 probe 一个候选句柄，通过 CAS 后才可分配给新 Run。旧句柄在成功提交后退休，失败则只关闭候选。Run 终态释放租约；WaitingForInput 可以释放物理连接，但 durable binding 仍保护 revision/generation，恢复只能重开同一身份。应用退出停止分配并关闭剩余连接。跨进程缓存不要求广播：新 Run 每次通过持久状态验证资格，旧 Run 仅按已存 binding 解析。

### SqliteDatasourceRepository 与 DatasourceVault

Inbound：Service 和 RuntimeStore 创建事务（P0）；External：同一 Workspace SQLite、本地受保护凭证目录（P0）。Repository 不自行发网络请求。

~~~rust
trait DatasourceRepository: Send + Sync {
    async fn load(&self, scope: DatasourceScope) -> DsResult<DatasourceSnapshot>;
    async fn commit(&self, change: DatasourceCommit) -> DsResult<DatasourceReceipt>;
    async fn pending_secret_mutations(&self, workspace: WorkspaceId)
        -> DsResult<Vec<SecretMutation>>;
    async fn load_run_binding(&self, run: RunId) -> DsResult<RunDatasourceBinding>;
}
trait DatasourceVault: Send + Sync {
    async fn protection(&self) -> DsResult<ProtectionStatus>;
    async fn write(&self, reference: DatasourceSecretRef, value: SecretValue) -> DsResult<()>;
    async fn read(&self, reference: DatasourceSecretRef) -> DsResult<SecretLease>;
    async fn remove(&self, reference: DatasourceSecretRef) -> DsResult<()>;
}
~~~

DatasourceCommit 是 SaveRevision / Validation / Selection / Delete / SecretJournal 转移的封闭枚举；公共字段为 CommandId、scope、expected versions，payload 对应上面的 API。禁止通用任意 SQL 或可变 JSON patch。Receipt 含提交版本和完整脱敏结果，可幂等重放。

| 表 | 关键列 / 约束 |
|---|---|
| datasource_profiles | (workspace_id, profile_id) 主键；可见 name_key 唯一；SourceId 稳定；head revision 与删除标记 |
| datasource_revisions | (workspace_id, profile_id, revision) 主键；版本化非秘密配置、Context、generation；不可变 |
| datasource_validations | revision 外键；validation_id 主键；state、脱敏 evidence、依赖指纹、validated_at；CAS 替换当前验证指针 |
| datasource_selections | (workspace_id, selection_kind, owner_id) 主键；owner 为 Workspace 或 Session；目标复合外键、version；显式空选择可持久化 |
| datasource_credential_generations | (workspace_id, profile_id, generation) 主键；opaque reference、available/retired 状态，无秘密 |
| datasource_secret_journal | mutation_id 主键；expected revision、旧/新 generation、Prepared/VaultWritten/Committed、命令指纹；无秘密 |
| datasource_command_receipts | CommandId 主键；非秘密请求指纹、结果；同 ID 不同请求拒绝 |
| run_datasource_bindings | run_id 主键及 Run 外键；不可变 binding；revision/generation 外键；索引支持非终态引用检查 |

表属于独立管理模型，Task/Run 事件仍是运行事实权威。所有 Profile/选择/验证的关联更新使用同一 SQLite 事务；PRAGMA foreign_keys=ON。Profile 删除采用 tombstone 保留历史解释；仅无非终态引用时回收秘密，历史 binding 保留非敏感 identity。

Vault 采用已有 ring AEAD + owner-only 文件保护模型，抽取无 Provider 类型的内部引擎。Datasource 使用独立目录、key 文件和 AAD 命名空间，AAD 绑定 Workspace/Profile/generation/envelope version；保留 Provider 现有布局和字节契约。不把同目录密钥包装成 OS Keychain 等级的保护；权限无法确认、符号链接或异常所有权时拒绝操作。

秘密跨文件和数据库的更新采用已有 journal 模式：Prepared 持久化 → 写入不可变加密 generation 并同步 → VaultWritten → 单事务提交新 revision、凭证关联、验证失效与回执 → Committed。旧 generation 在提交和引用检查之前不删除。启动先恢复未完成 journal：无已提交指针则清理新 generation 并回到旧状态；已提交则完成清理。清理失败保留可重试 journal，阻止相关 Profile 新操作，不丢弃记录。并发写以数据库唯一 generation/CAS 决胜，Vault 写必须 create-new。加密载荷可暂存但未提交前不可被 Profile 解析。

### Query、Policy、Context 与 Doctor 接缝

Run 创建在读取 Session 的精确选择后构造 binding 候选；RuntimeStore.commit_command 在同一事务重验选择版本、revision 可用性、generation 状态、验证证据和 Policy 指纹，然后同时写 Run、Provider binding、Datasource binding 及初始事件。与删除/替换竞争时，先提交的一方胜出，另一方返回 Conflict，不存在“已删凭证随后仍创建 Run”。

CreateRunCommand 必须同时提供两个 binding。新 DatasourceBound 事件 payload 仅为 schema_version、RunId 和 binding digest，仍由命令构造器唯一生成。持久绑定保留安全的 Context 标识，不记录 DSN 或受限路径；完整文件定位仅保存在受保护访问的 Profile 配置，binding 通过 revision 解析，Context 对外使用非敏感逻辑标识和指纹。

Harness 每次加载 RunDatasourceContext，替换固定 data_scope/query_budget/connector_tools；Tool 也使用相同 RunId resolver，不另读当前选择。结果策略在 core 中表示为现有 ColumnPolicy 等类型的不可变快照，由 Adapter 构造其 ResultPolicy 实例；runtime 不导入 adapters::ResultPolicy。ResultPolicy 与 Source ACL 在新 Run 启动时解析并保存不可变版本或受治理快照；按 SourceId + revision + Context 指纹过滤 Metric、Schema、dbt 和 Freshness Evidence。外部 Context 文件改变不替换旧 Run 已持久化 ContextManifest；缺少原证据返回缺失错误。

现有 ResultPolicy 的 schema_version=1 只有 allowed_sources 和关系/列规则，没有物理目标或允许路径。生产数据源管理采用该文件的 v2：每个 SourceRule 在原 relations 外新增 target（Adapter 类型及精确 host/port/database/schema 或规范化文件路径），文件目标另含 allowed_roots。预算继续来自现有 Workspace 配置，不新建 Policy 服务。该契约扩展只把已有授权绑定到实际目标，不能产生额外关系/列权限。

新 Profile 从目标精确匹配的受信 SourceRule 取得 SourceId；有多个匹配时 TUI 显式选择授权 Source，列表仅展示已有授权，Draft 可暂缺匹配。重命名不改 SourceId；目标变化必须重新匹配同一 Source 的授权目标，否则拒绝 Ready，要求在既有授权流程修复 Policy 或创建另一个 Profile。v1 可继续解释历史产物，不能据其缺失的目标信息为新 Profile 生成验证证据；Doctor 明确报告需由责任人补充目标约束。数据源 TUI 不写授权文件。V2/V5/V7 必须覆盖该配置升级及缺目标时 fail closed。

Doctor 指定 Profile 时使用同一 Service 验证路径，报告配置、Credential、连接、只读、Capability、Policy、Metric、dbt、Freshness，区分缺失/不适用/失败。连接验证只做元数据 probe；Doctor 如需业务 Freshness 数据，走既有受治理 Tool。QueryArtifact 和 VerificationReport 核对 SourceId、revision、Context、binding digest、Policy 与 Run 一致。

### DatasourceScreen

采用 Browse → ConnectorSelect → Edit → Validate/Save → Result 的局部状态，Actions 提供编辑、验证、删除、默认。Browse 的 Ready 项 Enter 选择当前 Session；尚未 Ready 项进入修复。新建可先保存在 Draft，再验证并可选激活，所有步骤留在 TUI。

复用现有可搜索 selector，名称/类型前缀优先，稳定排序；支持 ↑/↓、Page Up/Down、Home/End、Enter、Esc，保持唯一高亮可见。每层显示键位；Esc 返回父级，顶层恢复调用前视图与 Composer。/connections 为 /datasource 的别名。

异步结果携带 OperationId、请求 revision 和 selection_version。只接纳当前操作结果；忙态保持渲染与可取消提示。提交前可取消，提交中等待确定结果；不把 UI 离开页面等同事务回滚。失败保留非秘密输入，Secret 在发送/取消时清空，要求重输替换值。

Header 与列表使用同一 SelectionSnapshot，一次 reducer 更新，不再读取 bootstrap 的静态连接摘要。UI 发布快照后才放行本 Session 后续 Query 输入。回包丢失时显示“确认状态中”，查询命令回执；禁止仍显示旧 Header 却提交使用新选择的 Query。

## System Flows

~~~mermaid
sequenceDiagram
    participant UI
    participant Service
    participant Manager
    participant Store
    UI->>Service: Select Ready revision
    Service->>Manager: Prepare candidate and probe
    Manager-->>Service: Candidate and verified context
    Service->>Store: CAS selection and receipt
    alt Commit succeeds
        Store-->>Service: Committed snapshot
        Service->>Manager: Publish candidate and retire old allocation
        Service-->>UI: SelectionSnapshot
    else Conflict or failure
        Service->>Manager: Close candidate
        Service-->>UI: Safe error and unchanged selection
    end
~~~

Manager 的 publish 不执行新的外部 I/O；候选在提交前已经可用。数据库提交是持久选择的线性化点；进程中断后按持久选择重建，不能声称磁盘提交后仍可用内存回滚。若提交前 view 构造/解析失败，完全不提交。网络在之后断开是数据源调用失败，保持选择并明确报错。

## Security and Driver Contracts

共有门禁顺序为：绑定一致性 → 当前操作授权/预算 → 单语句只读 AST 与函数/关系范围 → 驱动只读事务或文件模式 → 有界结果与脱敏。Capability 不是授权，SideEffect::None 不是重试许可。预算未知或调用结果不确定时复用既有确认/恢复契约，不自动重试。

| 驱动 | 配置及验证 | 执行与拒绝 |
|---|---|---|
| SQLite | 元数据字段为已有 database_path；DatabaseContext 指向规范化普通文件，检查配置允许根目录、可读和文件身份；SQLITE_OPEN_READ_ONLY，不接受 memory/URI 或隐式创建 | query_only；禁止 ATTACH、扩展、文件函数及写语句；单次连接在阻塞任务内结束，提供中断处理；保留既有 ResultPolicy |
| PostgreSQL | host、port 默认 5432、database、schema、username、Secret password；支持结构化 TLS 配置并遵循现有 rustls 策略。probe 用系统目录与常量查询确认身份、当前 database/schema、只读状态和权限 | 每次 Query 开启 READ ONLY transaction，固定 search_path，核对 Context；最小权限与 SQL 门禁共同保障；池显式 close，错误只保留 SQLSTATE 分类 |
| DuckDB | 已有 database_path；ReadOnly Config；Config/Connection 在阻塞工作线程内构建；probe 读取配置和 catalog，不读取业务行 | 禁止自动安装/加载扩展、外部访问、ATTACH、COPY/EXPORT、文件表函数；初始化后锁定配置；线程/内存/输出受限，超时调用驱动 interrupt 并等待结束 |

PostgreSQL 只设置 default_transaction_read_only 不构成最小权限证明。probe 检查有效身份及可继承角色，拒绝 superuser、BYPASSRLS、CREATEROLE、CREATEDB、复制/危险预定义角色权限；检查授权范围内对象的所有权、列/表/sequence 写权限、schema CREATE 与 database CREATE/TEMP 权限。自定义函数、SECURITY DEFINER、foreign table、view 依赖必须能证明处于当前 Policy 允许范围，否则该查询拒绝。只允许明确审核的内置只读函数集合，不能通过一般 VOLATILE/STABLE 标签推断无副作用。检查无权完成时验证失败；不通过实际写客户表做探测。

SQLite/DuckDB 每次打开均重新检查允许路径、文件身份和只读模式，拒绝符号链接跳转及校验期间替换；允许根目录必须来自上文受信 Policy v2，无配置时不默许任意目录。文件身份变化使验证失效。对并发修改文件的特权本地用户不声称提供进程沙箱；本期读取用户受信的数据库文件，不能把内嵌 DuckDB 配置当作恶意文件解析隔离。未来不受信 Adapter 的隔离单独设计。

DuckDB 固定 enable_external_access=false、autoload_known_extensions=false、autoinstall_known_extensions=false，禁止允许路径例外；设置资源预算与 temp_directory 为空以禁止临时 spill 后 lock_configuration=true。参数不支持或不能确认时拒绝激活。SQL 使用 DuckDbDialect，MetricSqlDialect 增加 DuckDb；参数绑定、日期/时区、NULL、decimal 及结果上限须有真实 Query 证据。驱动无法表达某结果类型时返回稳定 UnsupportedType，不损失精度后声称验证通过。

## Error Handling and Performance

DsError = {code, field: Option<FieldId>, remediation, operation_id}，全部为类型化字段；不透传驱动 message/source。代码覆盖 InvalidField、DuplicateName、ConfigIncompatible、CredentialMissing/Expired/ProtectionUnavailable、AuthenticationFailed、TargetMissing、FileUnreadable、PermissionDenied、ReadOnlyUnproven、Timeout、Network、Protocol、CapabilityMissing、PolicyDenied、ValidationStale、Conflict、InUse、Storage、Cancelled。

字段错误回到编辑；Conflict 刷新；InUse 给出等待或取消 Run；验证错误保持未激活；存储错误保持提交前状态或进入明确的回执确认。日志只记操作类型、非敏感 ID、code、耗时与 binding digest，不记字段值、SQL、驱动回显、完整文件路径或 secret hash。

列表只查本地元数据，不建立连接。连接 probe 默认总截止 10 秒，可由受信 Policy 收紧；连接池 acquisition 与执行时间均计入预算。SQLite/DuckDB 使用有界阻塞任务，DuckDB 每个句柄初始并发为 1；同键排队避免一条查询的 interrupt 影响另一条。达到总截止时必须中断实际驱动，不能仅丢弃 Future。关闭等待有界，超时句柄隔离并拒绝复用。资源目标由真实超时、饱和和关闭测试验证，不预先承诺未测吞吐量。

## Migration Strategy and Rollout

1. 增量建表与契约迁移，保留已有 Provider 和 Run 数据；同一 Workspace SQLite 保证 binding 与 Run 的原子性。Source Policy v2 的目标约束必须在新 Profile 激活前由责任人提供，不能从旧启动环境默认为授权。
2. 生产 bootstrap 先打开管理存储和恢复 journal，再构建无连接的 Catalog/Service。无 Ready 数据源时仍可进入管理界面；Query 必须阻止。
3. 不把环境变量或旧静态 Connector 静默转换为 Ready。旧 Workspace 在 TUI 显式创建/验证 Profile；现有环境值不回写。测试/demo 组装显式创建测试 Profile 和完整 binding，不成为生产 fallback。
4. 已有历史终态 Run 可读取、导出；缺少 Datasource binding 的旧非终态 Run 标记为不可恢复并提供取消后新建的诊断，禁止从当前选择猜测绑定。普通新 Query 和 retry 新 Run 均重新绑定当前显式选择；resume 保留旧 binding。
5. 先验证三驱动及迁移，再接通统一键盘流程与全发布门禁；Catalog 在开发期间可显示 Registered，但三者缺任一个正式证据均不能发布 Feature。失败时保留 Workspace 备份与新格式数据，不做破坏性降级。

## Testing Strategy

| 验证点 | 可观察断言 |
|---|---|
| V1 Domain | 唯一名称、Draft 缺字段与非法字段区别；revision 不变；SecretEdit Keep 不改 generation；配置版本未知拒绝；Capability 与只读机制无类型名称推断 |
| V2 Store | 两客户端抢写仅一方成功；选择/删除/建 Run 竞争无悬空引用；在 journal 每阶段及 SQLite commit 前后注入中断，重开后仅完整旧/新状态；响应丢失重放同一回执；非终态阻止删除秘密 |
| V3 Connectors | SQLite/PostgreSQL/DuckDB 运行同一 suite：真实 catalog/preflight/query/freshness、零业务行 probe、错误分类、只读和不创建文件；拒绝嵌套函数/URI/ATTACH/扩展/写出；超时实际中断、池限额和 close；缺少权限证明必须失败 |
| V4 Secrets | 测试运行时生成 canary；覆盖密码替换、并发 generation、AAD 跨 Profile/Workspace 调换、权限错误、崩溃清理；扫描普通 SQLite 表/WAL、Event、Artifact、日志、Telemetry、TUI，只有加密 Vault 可含密文，不提交秘密 fixture |
| V5 Runtime | Run A 绑定旧 revision，切换/编辑/默认更改后 Run B 用新 revision；暂停重启 A 仍访问原数据库；Policy/Schema/Freshness 不串源；无 Ready 拒绝 Run；故障不切换；终态后旧 Connector 关闭；Doctor 和 Artifact 身份一致 |
| V6 TUI | 真实按键完成三驱动新增、编辑、验证、选择、默认、删除；搜索、全导航键与 Esc 层级；秘密遮蔽、非秘密输入保留、错误重试、忙态取消、陈旧回包；Header/列表同版本，切换不丢对话/Task/Artifact；重启和 /connections 别名 |
| V7 Release | 迁移含旧 Provider、旧 Run、有/无 Profile Workspace；原 Query eval、Doctor、Export、Provider/TUI 回归；三驱动门禁不因缺 PostgreSQL 服务或 DuckDB 构建失败而跳过 |

每个驱动契约使用测试临时创建的真实数据库，生产路径永不调用 fixture 工厂。发布执行现有 fmt、clippy、workspace test 和 scripts/v0.2-release-gate.sh，并扩展该脚本收集三驱动 engine/adapter version 与测试结果。门槛为 Credential 泄露 0、越权写入 0、静默切换严重错误 0；设计通过不代表这些运行证据已经通过。

## Requirements Traceability

每个 N.M 均来自已批准需求。相同实现与验证点合并一行，明确枚举以便机械检查。

| Requirement | Summary | Components | Interfaces | Flows / Verification |
|---|---|---|---|---|
| 1.1, 1.2, 1.3, 1.4, 1.5, 1.7 | 真实目录、能力及失败关闭 | Catalog、Service | descriptors / view | V1, V3, V7 |
| 1.6, 2.2 | 身份、名称与物理 Context 分离 | Domain、Repository | Revision / save | V1, V2 |
| 2.1, 2.3, 2.4, 2.5, 2.6, 2.7 | CRUD、Draft、不可变编辑与回滚 | Service、Repository | save / delete | V1, V2, V6 |
| 2.8, 2.9, 2.10 | 删除替代、Run 保护、当前与默认 | Service、Repository | delete / select | V2, V5, V6 |
| 3.1, 3.2, 3.3, 3.4, 3.6, 3.7 | Secret 隔离、遮蔽与保护 | Vault、Domain、Screen | SecretEdit / Vault | V1, V4, V6 |
| 3.5, 3.8 | 原子轮换与并发隔离 | Service、Repository、Manager | journal / resolve | V2, V4, V5 |
| 4.1, 4.2, 4.3, 4.4, 4.8 | 元数据表单、版本和字段校验 | Catalog、Screen、Service | descriptors / save | V1, V6 |
| 4.5, 4.6, 4.7 | 三驱动实际配置与限制 | Catalog、三驱动 | open / probe | V3, V6 |
| 5.1, 5.2, 5.3, 5.4, 5.5 | 本地检查、无业务行 probe 与关闭 | Service、Manager、三驱动 | validate / probe / close | V3, V5 |
| 5.6, 5.7, 5.8, 5.9 | 证据有效性、Ready 与 fail closed | Domain、Service、Repository | ValidationEvidence | V1, V2, V5 |
| 6.1, 6.2, 6.3, 6.4, 6.5, 6.6, 6.7, 6.8 | 按需创建、绑定缓存与生命周期 | Manager、Catalog、Query 接缝 | resolve / open / close | V3, V5 |
| 7.1, 7.2, 7.3, 7.4, 7.5, 7.6, 7.7 | 命令、搜索、选择和表单 | Screen、Service | view / save / select | V6 |
| 7.8, 7.9, 7.10, 7.11, 7.12, 7.13, 7.14 | 返回、取消、错误、键盘与别名 | Screen、Service | OperationId / receipt | V2, V6 |
| 8.1, 8.2, 8.3, 8.6, 8.7, 8.8, 8.10 | 原子当前/默认、恢复和界面一致 | Service、Repository、Screen | SelectionSnapshot | V2, V5, V6 |
| 8.4, 8.5, 8.9 | Run 原子绑定、不变与不回退 | Query 接缝、Manager、Repository | CreateRunCommand / resolve | V2, V5 |
| 9.1, 9.2, 9.3, 9.4, 9.5, 9.6, 9.7, 9.8 | 数据库与共用安全门禁 | 三驱动、SQL/Result Policy、ToolRuntime | probe / preflight / query | V3, V5, V7 |
| 10.1, 10.2, 10.3, 10.5, 10.6 | Source Policy、Context 与产物绑定 | Query 接缝、Service | RunDatasourceContext / Artifact | V5, V7 |
| 10.4 | 指定数据源 Doctor | Service、Doctor | doctor | V3, V5, V7 |
| 11.1, 11.2, 11.4 | CAS、崩溃恢复与职责分表 | Repository、Service、RuntimeStore | commit / journal | V2, V7 |
| 11.3 | 稳定错误与脱敏 | Domain、三驱动、Screen | DsError | V3, V4, V6 |
| 11.5, 11.6, 11.7, 11.8 | 三驱动、键盘与零严重事故门禁 | 全闭环、发布脚本 | V7 | V2, V3, V4, V5, V6, V7 |

## Design Decisions

| 决策 | 第一性原理与取舍 |
|---|---|
| 一套 revision / binding 身份贯穿 UI、存储和 Query | 可变名称无法证明查询访问了哪个数据库；拒绝名称缓存和在每次 Tool 调用读取当前选择 |
| 一个管理服务、一个资源 Manager | 管理事务与连接资源生命周期不同；不再增加 Controller、Facade、独立验证微服务 |
| 同 SQLite 分表 + Secret journal | Run 与 binding 需要同一提交点；秘密需要独立保护。拒绝普通 JSON 配置加多文件覆盖，也不自建分布式事务 |
| 复用加密引擎和 Query Ports | 成熟能力已存在；只抽取被 Provider 与 Datasource 实际共享的文件加密机制，不创建通用 Profile 框架 |
| 内建 Catalog + 三个 Factory | 现在需要的是可靠注册和能力路由；不创建空的远程 Registry、Installer 或 Host 接口 |
| validation 不接管连接 | 多一次激活连接换取清晰关闭语义，避免复杂连接转移状态；后续测量证明有必要再优化 |

未来 Connector Adapter Repository 接入时，发现只刷新受信元数据；Catalog 中的 adapter_id/version、config_version、Core Contract 与 Capability 对应未来发布清单，Profile 和 Run binding 继续固定已激活版本。本期只保存这些有现实用途的身份，不添加空插件类型。

未来安装必须验证签名、发布物摘要、兼容范围、平台、来源证明、有效期及撤销状态，再把固定版本放入隔离区；激活必须通过共享契约测试、无业务数据健康探测和显式信任策略，失败保留旧版本并原子回滚。外部 Host 需经独立 ADR/Spike 选择版本化进程外协议或受沙箱组件，证明 Secret broker、文件/网络权限、资源与崩溃隔离。离线发现仅使用有效且未撤销的已验证缓存。上述内容是延期边界，不得进入本 Feature 的实现任务。

## Supporting References

本设计核对日期：2026-09-05。代码事实见 Existing Architecture Analysis；外部依赖结论来自以下官方资料，实施以锁定版本重验。

### 指定参考仓库的实现映射

参考仓库：/Users/ysc/Documents/Data_Engineering/projects/Datus-agent-opencode-go，核对 HEAD 为 11f327a074dc770e9f4a2444ff13221bc21458e4。首先阅读 docs/adapters/datasource_connector_design_analysis.zh.md，再核对以下实际代码；该仓库只读，不作为本 Feature 的修改目标。

| 参考实现 | 本 Feature 的采用方式 |
|---|---|
| datus/configuration/agent_config.py 的 DbConfig | 统一 Profile 配置与 Adapter 专属字段；采用版本化字段校验，Secret 只存引用 |
| datus/tools/db_tools/__init__.py 的注册元数据 | 同一注册项提供 Factory、配置字段、显示名称与能力；本期仅内建三种受支持驱动 |
| datus/tools/db_tools/db_manager.py 的 get_conn、_build_conn、close | Manager 统一创建、路由与关闭；保留 datasource/database 分离，缓存加强为不可变 revision/binding 身份 |
| datus/cli/datasource_app.py 的配置类字段读取 | 从 Connector 元数据生成 TUI 表单，不维护第二份数据库字段清单 |
| datus/cli/init_util.py 的 detect_db_connectivity | 统一创建临时 Connector 并测试；补上所有退出路径关闭及稳定脱敏错误 |
| datus/tools/db_tools/sqlite_connector.py | 已有文件、只读打开与连接测试路径；保留 YS 的禁止创建及 Query 门禁 |
| datus/tools/db_tools/duckdb_connector.py | 惰性连接、共享连接串行化及显式关闭；采用 YS 的固定安全配置与预算 |
| .venv/lib/python3.12/site-packages/datus_postgresql/{config,connector}.py，已安装包 0.1.7 | 普通 PostgreSQL 主机/端口/用户名/密码/database/schema/sslmode 配置与驱动封装；Python 包仅作为行为参考，不加入 Rust 运行依赖 |
| tests/unit_tests/tools/db_tools 与 tests/integration/tools/db_tools | 复用共享契约、Manager 与真实数据库测试的组织方式，加入不可变绑定和 Secret canary 断言 |

本期采用 PostgreSQL 普通 TCP 直连，包括 localhost、本机映射端口和可达远程主机；SQLite/DuckDB 使用本地文件。用户已明确将 SSH 移出范围，不创建 SSH 配置、依赖或执行任务。

不采用参考实现的运行时 pip 安装、明文密码字段、仅按名称缓存及自动选择首个连接。任务按可观察连接成果合并，测试随实现收口；不得把每个字段、接口或检查单独拆成任务，也不得以减少任务数量为由遗漏任一必需连接路径。

### 官方依赖资料

- [DuckDB Rust Config](https://docs.rs/duckdb/1.10505.0/duckdb/struct.Config.html) 与 [Connection](https://docs.rs/duckdb/1.10505.0/duckdb/struct.Connection.html)：只读、外部访问配置、安全 wrapper 和中断接口；Config 不跨线程共享。
- [DuckDB crate features](https://docs.rs/crate/duckdb/1.10505.0/features)：选择 bundled 最小构建，不启用扩展全集。
- [DuckDB security configuration](https://duckdb.org/docs/current/operations_manual/securing_duckdb/overview)：外部文件/网络限制、配置锁定。
- [DuckDB security boundary](https://github.com/duckdb/duckdb/blob/main/SECURITY.md)：数据库文件与内嵌执行的信任限制；本期配置门禁不是不受信代码沙箱。
- [PostgreSQL SET TRANSACTION](https://www.postgresql.org/docs/18/sql-set-transaction.html) 与 [权限检查函数](https://www.postgresql.org/docs/current/functions-info.html)：只读事务及有效权限检查；设计据此增加最小权限证明，不能仅检查一个 session setting。
- [SQLite open flags](https://www.sqlite.org/c3ref/open.html)：READONLY 缺文件报错、URI 与 NOFOLLOW 语义；拒绝默认可创建的打开方式。

## Design Review Gate

- 需求覆盖：所有数字验收项逐项映射到组件与 V1–V7；机械检查无遗漏。
- 边界与可执行性：每个新增组件均有文件归属；凭证引擎、共享 selector、三驱动构建和 Run 契约迁移前提明确。
- 本地修正（两轮内）：审查补齐 Source Policy v1 缺少目标/允许路径约束、共享 selector 已定位到 palette.rs、提交后响应丢失、建 Run 与删除竞争、旧非终态 Run 缺 binding、旧 Credential 被 Run 引用以及 SourceId 不应自动继承新目标授权。
- Verdict：PASS（设计审查）。用户已批准三驱动设计并明确 PostgreSQL 采用普通连接；需求与设计恢复为已批准范围。实施仍需通过依赖构建、数据库集成及发布门禁。
