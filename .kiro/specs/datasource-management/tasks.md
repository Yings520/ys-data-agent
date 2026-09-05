# Implementation Plan — datasource-management

共 9 个可执行任务，不设子任务；按依赖顺序执行，测试随实现完成。本文件是唯一任务与完成状态来源。

完成目标：用户可在 /datasource 保存、验证并选择 SQLite 本地文件、PostgreSQL 普通 TCP 连接（localhost、本机映射端口或可达远程主机）、DuckDB 本地文件，并真实运行受治理查询；重启可恢复配置。SSH、远程插件安装不在本期范围。数据库、凭证与既有授权配置由用户提供，不能用 fixture 代替实际连接能力。

实现以 design.md 的 File Structure Plan 和指定 Datus 参考映射为准。顺序任务共享契约、驱动文件和组合根，不声明并行。按用户要求合并有同一交付成果的实施与测试；跨层工作明确标为集成任务，不再拆字段、接口或单独测试任务。

- [x] 1. 固定数据源身份与统一连接契约
  - 定义 Profile/revision、DatabaseContext、Credential 引用、验证证据、Capability、管理请求/错误和 Run binding；区分 Draft 与非法输入、当前与默认，保留旧 revision 身份。
  - 声明 Service、Repository、Vault、Factory、Manager resolver 接缝及双 binding Run 创建契约；适配现有构造调用以维持编译，不填入可绕过生产门禁的默认 binding。
  - 完成态：身份、版本、秘密类型和状态不变量测试通过；上下层能针对同一契约实现，既有 Provider 契约保持有效。
  - _Requirements: 1.6, 2.2, 2.3, 2.4, 2.5, 3.4, 4.8, 5.6, 5.7, 5.8, 8.4, 11.3_
  - _Boundary: DatasourceDomain；core connector/ports/event/context；受契约影响的调用构造适配_
  - _Depends: none_

- [x] 2. 集成可恢复的配置与秘密存储
  - 实现 Profile、revision、验证、当前/默认、凭证 generation、Run binding、回执及 journal 的存储与迁移；CAS、关联完整性和建 Run/删除竞争使用同一 SQLite 事务。
  - 复用本地加密文件引擎，保持 Provider 原格式；实现 Datasource 独立命名空间、Keep/Replace/Remove、权限检查、秘密轮换与旧 Run 引用保护。
  - 完成态：保存或删除失败及各 journal 阶段中断后，重启只得到完整旧/新状态；非终态 Run 的凭证不可删除，普通存储和输出无秘密，Provider 存储回归通过。
  - _Requirements: 2.1, 2.7, 2.8, 2.9, 2.10, 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 3.7, 3.8, 11.1, 11.2, 11.4_
  - _Boundary: SqliteDatasourceRepository、RuntimeStore 命令事务与 DatasourceVault 的持久化集成_
  - _Depends: 1_

- [x] 3. 建立真实 Connector 目录并接通 SQLite
  - 先验证 DuckDB 锁定依赖、原生链接、许可和平台构建，实际驱动在任务 5 接入；三驱动开发测试使用显式受信上下文，不伪造生产授权。
  - 按 Datus Registry/Manager 模式提供真实注册项、配置字段、能力、版本及 Factory；目录不建连接，冲突 fail closed，未通过证据门禁的驱动不标记 Supported。
  - 接通已有 SQLite 文件的配置、只读打开、无业务行 probe、Catalog/Preflight/Query/Freshness 和关闭；拒绝缺文件创建、越界路径、URI/ATTACH、扩展与写出。
  - 建立三驱动共用的契约测试入口。完成态：SQLite 通过真实临时数据库查询、错误分类、预算/超时/关闭测试，注册元数据可直接供表单使用。
  - _Requirements: 1.1, 1.2, 1.3, 1.4, 1.5, 1.7, 4.1, 4.2, 4.3, 4.5, 5.1, 5.2, 5.3, 5.4, 5.5, 9.1, 9.2, 9.5, 9.7_
  - _Boundary: BuiltinConnectorCatalog、SQLite Connector、驱动依赖构建及共享 SQL/Result Policy 驱动边界_
  - _Depends: 2_

- [x] 4. 接通 PostgreSQL 普通连接
  - 支持 host、port、database、schema、username、受保护 password 和 TLS 配置；覆盖 localhost、本机映射端口及可达远程主机，不引入 SSH。
  - 用系统元数据证明目标、认证、只读与最小权限；真实查询使用只读事务、固定 Context、共享 SQL/结果门禁，连接池有界并显式关闭。
  - 完成态：真实 PostgreSQL 通过共享契约；正确参数可查询，错误密码/目标/TLS/权限、超时和连接失败稳定报错，凭证不出现在 DSN 输出中。
  - _Requirements: 4.6, 5.3, 5.4, 5.5, 5.9, 6.8, 9.1, 9.3, 9.6, 9.7, 9.8_
  - _Boundary: PostgreSQL Connector 及共享 SQL/Result Policy 驱动边界_
  - _Depends: 3_

- [ ] 5. 接通 DuckDB 本地文件
  - 锁定并验证 Rust 依赖、原生构建和实际引擎版本；接通已有文件、只读 Config、元数据 probe、关闭与单句柄串行执行，不自行编写 FFI。
  - 禁用扩展安装/加载、外部访问、ATTACH、COPY/EXPORT 和临时 spill，锁定配置；接通 DuckDB SQL 与 Metric 方言、参数/类型转换、预算及实际 interrupt。
  - 完成态：真实 DuckDB 通过共享契约及 Metric 查询，缺文件不创建，禁止行为被拒绝；超时实际结束查询且可安全关闭。
  - _Requirements: 1.1, 1.4, 4.7, 5.3, 5.4, 5.5, 5.9, 6.6, 6.8, 9.1, 9.4, 9.5, 9.7, 9.8_
  - _Boundary: DuckDB Connector、依赖构建与既有 MetricSqlCompiler 的驱动集成_
  - _Depends: 4_

- [ ] 6. 集成 Profile 管理与连接生命周期
  - 先完成 Source Policy v2 解析、物理目标精确匹配与 allowed_roots 检查及测试，作为 Ready/激活前提，不生成或扩大授权。
  - 实现元数据驱动保存/编辑/删除、本地及连接验证、Ready 判定、Session 选择和 Workspace 默认；操作完成返回统一已提交快照，支持取消、幂等回执、并发冲突与恢复。
  - Manager 按完整 binding 身份惰性创建并合并并发请求；验证连接关闭，激活先准备后提交，失败关闭候选；旧连接退休后仅供原 Run，终态释放，退出关闭。
  - 完成态：三种 Profile 均可经管理 API 完成 CRUD→验证→选择；Draft/失效配置不可激活，失败保持一致状态，不同 revision/Workspace/凭证不串用。
  - _Requirements: 2.1, 2.3, 2.4, 2.5, 2.6, 2.7, 2.8, 2.9, 2.10, 3.5, 3.8, 4.4, 4.8, 5.1, 5.2, 5.3, 5.4, 5.5, 5.6, 5.7, 5.8, 5.9, 6.1, 6.2, 6.3, 6.4, 6.5, 6.6, 6.7, 6.8, 8.1, 8.2, 8.3, 8.6, 11.1, 11.2_
  - _Boundary: DatasourceService、ConnectorManager 与 Source Policy 目标匹配的管理集成_
  - _Depends: 5_

- [ ] 7. 集成真实 Query、治理与重启恢复
  - Run 与双 binding 原子创建；Harness、Query Tools、ResultPolicy、Context 按 RunId 消费精确数据源，替换启动时固定连接；消费任务 6 的 Source Policy v2，新目标不继承旧授权。
  - 接通 Source/Context 证据过滤、Policy/预算快照、Doctor、QueryArtifact 与 VerificationReport；迁移旧 Workspace，缺 binding 的旧非终态 Run 明确不可恢复，无 Ready 时阻止新 Query。
  - 完成态：A Run 暂停后切换并启动 B，B 真实查询新源，重启恢复 A 仍查询原源；Policy/Schema/Freshness 不串源，故障不回退，产物和 Doctor 身份一致。
  - _Requirements: 6.2, 6.8, 8.4, 8.5, 8.7, 8.8, 8.9, 9.8, 10.1, 10.2, 10.3, 10.4, 10.5, 10.6, 11.4, 11.7_
  - _Boundary: RuntimeStore、Run Service、Harness、Query Tools、Source Policy、Context、Doctor 与 Artifact 的运行集成_
  - _Depends: 6_

- [ ] 8. 集成可直接使用的数据源 TUI
  - 复用 palette.rs 的 Selector 与 async_guard，以 Connector 元数据生成表单；键盘完成新增、编辑、验证、搜索、选择、默认、删除，/connections 导向同一流程。
  - 接通生产 bootstrap、异步服务动作和统一 Header/列表快照；支持逐层 Esc、全部导航键、秘密遮蔽、保留非秘密输入、忙态取消及陈旧回包隔离；切换保留对话、Task 和 Artifact。
  - 完成态：真实按键分别配置 SQLite、PostgreSQL 直连、DuckDB，验证后从同一 Session 发起真实 Query；无需编辑连接配置或另开 Shell，重启可再次使用保存的 Profile。
  - _Requirements: 3.2, 3.7, 4.1, 4.2, 4.3, 4.4, 4.5, 4.6, 4.7, 7.1, 7.2, 7.3, 7.4, 7.5, 7.6, 7.7, 7.8, 7.9, 7.10, 7.11, 7.12, 7.13, 7.14, 8.1, 8.6, 8.7, 8.8, 8.10, 11.6_
  - _Boundary: DatasourceScreen、现有 TUI 导航/选择器、bootstrap 与管理服务的产品入口集成_
  - _Depends: 7_

- [ ] 9. 验证三种连接的完整交付
  - 将三驱动共享契约、真实 TUI→验证→选择→Query→Artifact→重启链路加入发布门禁；测试环境缺 PostgreSQL 或 DuckDB 构建失败必须报失败，不能跳过后宣称完成。
  - 覆盖事务故障、并发与引用保护、连接关闭、Secret canary、旧 Workspace/Run 迁移，以及现有 Provider、Doctor、Export 和 Query eval 回归。
  - 完成态：V1–V7 与工作区 fmt/clippy/test、现有 release gate 全部通过，三驱动有实际版本及连接/查询证据，此时才代表 Feature 可交付；凭证泄露、越权写入和严重静默切换均为 0。
  - _Requirements: 1.1, 1.4, 3.7, 9.1, 11.1, 11.2, 11.3, 11.5, 11.6, 11.7, 11.8_
  - _Boundary: 三驱动及 TUI/Runtime 集成测试、现有 v0.2 发布门禁_
  - _Depends: 8_
