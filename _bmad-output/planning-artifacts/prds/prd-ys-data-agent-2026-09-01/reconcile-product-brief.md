# Product Brief 输入对账：统一 LLM Provider 管理 PRD

## 1. 对账范围

- 输入：`_bmad-output/planning-artifacts/briefs/brief-ys-data-agent-2026-09-01/brief.md`（`status: approved`）
- 目标：同目录 `prd.md` 与 `addendum.md`
- 方法：检查批准事实、范围、定性愿景、治理边界、目标用户和成功信号是否被保留、遗漏或误写；同时核对本轮已确认的开发阶段边界。
- 限制：本报告只记录差异，不修改 PRD 或 Addendum。

## 2. 总体结论

**结论：通过，但有 3 项非阻断的可追溯性缺口。**

PRD 没有把 YS 缩减为 Provider/TUI 工具，也没有扩大 v0.2 产品承诺。它明确把 LLM Provider 接入与管理作为主轴，把 TUI 限定为首期管理入口，并保留 Runtime、Policy、Completion Gate、只读 Query 和严重静默错误为 0 等核心边界。当前无存量用户、无需迁移、直接替换旧实现是本轮用户确认后的开发阶段决定，与 Product Brief 不冲突。

缺口主要不是方向错误，而是 Product Brief 的产品级成功门槛、治理约束和目标用户层级在本功能 PRD 中缺少显式的继承或验收连接。若下游只读取本 PRD 而不读取 Product Brief，可能误把功能级指标和技术配置用户当成整个产品的成功标准与目标用户。

## 3. 逐项对账

| 对账项 | Product Brief 批准内容 | PRD / Addendum 状态 | 结论 |
|---|---|---|---|
| 产品身份与定性愿景 | YS 是由明确责任人治理的完整 AI 数据团队，不是只生成 SQL 的 Copilot；Pilot 只是验证活动 | PRD §1 明确 Provider 变化不得改变产品承诺，并保留治理内核；没有把 YS 称为 Provider/TUI 工具或把 Pilot 当产品 | 基本保留，无误写；但“完整产品 / Pilot 仅为验证活动”未显式重申 |
| 当前产品范围 | v0.2 仅承诺本地、只读、受治理的 Query；不扩展到 Analysis、Build/Change、Operate、ML Data Prep 等 | PRD §1、§5 明确保留 v0.2 Trustworthy Query 边界，并将无关调用链路重构列为非目标 | 完整保留 |
| 本次功能主轴 | Product Brief 的产品承诺不应被局部界面工作替代 | PRD §1 明确主轴是 LLM Provider 接入与管理；§4.4、§5 与 Addendum §6 将 TUI 限为配置入口并排除通用 TUI 优化 | 完整保留 |
| 开发阶段与迁移 | Product Brief 未声称已部署；真实经历证明问题存在，但不证明 YS 已在原环境部署 | PRD §1.2、§2.1、§5、§8、FR-17、AC-12 和 Addendum §5 明确当前无存量用户、不做旧配置迁移或兼容、直接替换旧实现 | 与来源一致，并正确吸收本轮用户决定 |
| 目标用户 | Product Brief 的产品级目标是已有数据基础、但专业数据与治理能力不足的业务团队；业务用户、Data Steward、Accountable Data Owner 各有责任 | PRD §2.1 将本功能的直接操作用户定义为本地部署/配置/维护 YS 的技术用户 | 对功能直接用户合理，但缺少一句说明其不替代 Product Brief 的产品级目标用户与治理角色 |
| 用户价值 | 用户应在治理边界内及时获得可信、可验证、可审计的数据结果，而非仅得到模型输出 | PRD §1、FR-15、AC-10 保持 Query、Tool、Doctor、Artifact、治理门禁和显式非成功状态不回归 | 完整保留 |
| 治理边界 | 模型只能提出动作；权限、事实、正式口径和完成状态不能由模型自行决定；执行受批准的上下文、权限和成本边界约束 | PRD §1 明确保留 Runtime、Policy、Completion Gate；NFR-4/5 禁止扩大数据外发范围并要求失败关闭 | 核心原则保留；但 Provider 激活与“已批准的数据外发/成本边界”之间没有对应的功能验收 |
| 产品级成功信号 | Pilot 门槛：真实需求覆盖率 ≥60%、可信自助完成率 ≥80%、严重静默错误为 0；后续还需定义首次可信结果时间等 | PRD §10 定义 9/9 Provider、契约回归、配置闭环等功能指标，并保留严重静默错误为 0 | 功能指标合理，但没有声明它们从属于且不替代 approved Product Brief 的产品级 Pilot 门槛 |
| 长期演进 | 从 Query 扩展至 Analysis、Build/Change、Operate、ML Data Prep；语义层与 `/explore` 是近期产品演进 | PRD 将这些能力排除在本功能范围外，同时保留 Provider 扩展路径 | 无冲突；局部 PRD 无需复制完整产品路线图 |

## 4. 关键缺口

### G-1：功能成功指标与产品级成功门槛的关系未显式说明

**级别：中。** PRD §10 的 Provider 覆盖率、配置闭环和契约回归指标适合本功能，但 approved Product Brief 的真实需求覆盖率、可信自助完成率和严重静默错误门槛才是产品级 Pilot 成功标准。当前文本只继承了“严重静默错误为 0”，没有说明本功能指标不替代其余产品级门槛。

**风险：** 下游或评审者可能把 9/9 Provider Supported 误读为产品成功，而忽略 Provider 改造最终必须服务于可信 Query 的产品结果。

**建议处理：** 在 PRD §10 增加一条继承声明：本节是功能交付指标，不取代 Product Brief 的产品级 Pilot 成功门槛；本功能通过 AC-10/SM-3 证明不损害这些门槛。无需在本 PRD 重复测量真实需求覆盖率或可信自助完成率。

### G-2：Provider 激活没有可验收地连接批准的数据外发与成本边界

**级别：中。** PRD §1 与 NFR-4/5 已保留治理原则，但 FR-7 至 FR-9 和 AC-5/AC-8 主要验证配置、模型能力、错误与失败关闭，没有明确验证一个 Provider Profile 只有在符合当前批准的数据外发和成本策略时才能激活。

**风险：** 仅凭凭证有效和模型能力兼容就激活新 Provider，可能改变业务数据的外部接收方或成本行为；这与 Product Brief 的“在已批准的业务上下文、权限和成本边界内执行”存在验收空隙。

**建议处理：** 不扩大本 PRD 为完整策略系统；只需在激活验收中明确：Provider/Profile 切换不得绕过现有 Policy，若目标 Provider 不在已批准的数据外发或成本边界内，应失败关闭并给出可操作原因。具体策略模型留给 cc-sdd 设计。

### G-3：功能直接用户与产品级目标用户的层级关系未明确

**级别：低。** PRD §2.1 正确识别了本功能的直接操作用户——部署、配置或维护 YS 的技术用户；但没有明确这只是配置角色，不替代 Product Brief 中业务使用者、Data Steward、Accountable Data Owner 以及“已有数据基础但专业能力不足的业务团队”这一产品级目标。

**风险：** 单独阅读本 PRD 时，可能误以为 YS 整体转为面向技术维护者的 Provider 管理产品。

**建议处理：** 在 §2.1 增加一条范围声明：技术用户是本功能的直接操作者，Product Brief 定义的业务团队与治理角色仍是 YS 的产品级目标用户和责任主体。

## 5. 已确认无缺口的重点项

- **当前无存量用户：** PRD 多处明确说明项目仍在开发阶段，不声称已有用户或部署安装。
- **本次不做迁移：** 旧 `YSDA_LLM_*` 不导入、不兼容，不创建迁移 Profile、兼容窗口或双实现；与用户最新批准一致。
- **TUI 非主轴：** TUI 仅承担 Provider Profile 的查看、配置、验证和切换；通用导航、主题、聊天界面和快捷键优化均在非目标中。
- **ChatGPT 范围：** `chatgpt/` 明确指 ChatGPT Subscription，`openai/` 不属于本期；与用户确认一致。
- **首期 Provider 范围：** 9 个目标 Provider 被清晰枚举，165 个 Provider 只作为依赖生态能力和后续扩展空间。
- **治理内核不变：** Provider 改造不重构 Runtime、Policy、Completion Gate、Query Artifact 或无关 LLM 调用链路。
- **直接替换边界：** `liter-llm` 是唯一活动接入层，旧自有 OpenAI-compatible Provider 不作为长期并行实现。

## 6. 对账判定

Product Brief 的批准事实和本轮用户决定总体已正确进入 PRD / Addendum，没有发现会导致“迁移范围回潮”“TUI 反客为主”“把 Pilot 当成产品”或“扩大 v0.2 Query 边界”的关键冲突。G-1 与 G-2 建议在最终批准前补强，以便功能指标和激活验收可明确追溯到 Product Brief；G-3 是阅读边界澄清，不影响当前范围成立。
