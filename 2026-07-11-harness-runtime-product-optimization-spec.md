# Harness 与 Runtime 产品优化方案

日期：2026-07-11
状态：Revision 2 / Engineering Review Incorporated / 待联合签署
类型：Master PRD + Engineering Delivery Baseline
参考研究样本：`/Users/carlos/Downloads/claude-code-main`
适用范围：zeronDesign Harness、Runtime、Sandbox、任务执行与结果验收

文档 Owner：待指定
联合签署：Product、Runtime、Infrastructure、Security、Data/Eval
计划评审：Phase 0 立项评审

## 1. 文档结论

zeronDesign 当前 Runtime 已经具备较强的网站与文档生成领域能力，特别是在 Workspace
隔离、确定性构建、Preview Promotion、Checkpoint、运行恢复、Typed Recovery 和安全策略方面，
其领域可靠性高于通用 Coding Agent。

当前的主要问题不是继续增加工具，而是缺少一套能够持续回答以下问题的产品闭环：

- 一次任务是否真正完成，而不只是模型声明完成；
- 完成任务花费了多少时间、Token、费用和工具调用；
- 失败发生在模型、上下文、工具、依赖、模板、基础设施还是验收环节；
- 相同任务在版本升级后，完成率和效率是提升还是下降；
- 用户是否理解 Agent 当前状态，并能在正确的时机进行干预。

本方案不主张复制 Claude Code 的完整 CLI、通用工具和多 Agent 体系，而是保留 zeronDesign
的领域确定性，吸收其成熟的上下文治理、预算控制、可观测性、会话恢复和工具调度能力。

产品演进主线为：

```text
Eval 基线
  -> Usage 与 Cost 可观测
  -> ContextManager
  -> CompletionPolicy
  -> Resource-aware Scheduler
  -> 用户可理解、可干预的任务体验
```

## 2. 背景

### 2.1 当前产品定位

zeronDesign Runtime 不是通用 Coding Agent。它服务于具有明确生命周期和验收门槛的设计生成任务：

```text
Brief
  -> Build
  -> Preview
  -> Screenshot / Fidelity Review
  -> Promote
  -> Edit
  -> Rebuild / Republish
```

Runtime 已将其中大量环节从 Prompt 建议提升为系统约束，例如：

- Build 依赖确定的项目状态和 Workspace；
- Candidate 必须绑定构建和 source snapshot；
- Preview Promotion 必须满足 Screenshot、Fidelity 和 CAS 条件；
- Run 支持 Partial、Blocked、Failed、Cancelled 和 Needs User Input 等状态；
- 工具错误支持结构化 `errorKind`、`recoverable` 和恢复建议；
- Checkpoint 可以在 Runtime 重启后恢复任务；
- Sandbox 对路径、命令、Registry、Preview 和远端 Workspace 进行约束。

### 2.2 对照 Claude Code 的目的

Claude Code 的优势主要体现在通用 Agent Harness 工程，而不是网站生成领域规则：

- 基于 Token 和上下文窗口的多级压缩；
- Prompt Cache 与工具结果裁剪；
- 模型、Token、费用、API 和工具耗时统计；
- Max Turns、Max Budget 和中断控制；
- Resume、Rewind、Compact、Context、Cost 等用户能力；
- Deferred Tool Discovery；
- 通用工具并发、后台任务和 Sub-agent；
- 更完整的终端交互和权限反馈。

因此，对照的目标是识别 Harness 能力差距，而不是把 zeronDesign 改造成另一个 Claude Code。

### 2.3 用户与核心任务

本方案服务三类主要用户：

| 用户 | 核心任务 | 需要的结果 |
| --- | --- | --- |
| 网站/文档创建者 | 从 Brief 生成、修改并发布可访问产物 | 结果正确、进度可见、失败可恢复 |
| 产品与设计评审者 | 判断产物是否满足 Brief、视觉和内容要求 | 有可追溯证据，而不是只看 Agent 自述 |
| Runtime 运维与研发 | 定位失败、控制成本、比较版本质量 | 有稳定指标、错误分类、Checkpoint 和回归基线 |

核心 Jobs to Be Done：

- 当用户提交一个 Website 或 Docs 任务时，系统应在明确预算内交付经过验证的 Artifact；
- 当任务无法继续时，系统应解释阻断原因、保留有效进度并给出可执行恢复动作；
- 当 Runtime、模型或模板升级时，团队应能判断完成率、成本、延迟和安全性是否发生回归；
- 当用户中途补充要求时，系统应保留既有约束，只中断必要工具，并从一致状态继续。

### 2.4 外部研究样本与 Clean-room 约束

`claude-code-main` 是非官方安全研究源码快照，不是 zeronDesign 的产品依赖或可直接复用实现。
本项目只允许研究其中公开可观察的产品行为和通用架构概念，并遵守以下约束：

- 不复制源码、Prompt、内部字符串、私有协议或非公开配置；
- 不把该快照加入构建、测试、发布或供应链依赖；
- 所有功能根据本项目需求独立设计、独立实现并由本项目测试证明；
- 任何需要保留的对照证据仅记录能力结论，不分发快照内容；
- Phase 0 立项前由 Security/Legal 确认研究和引用边界。

## 3. 问题定义

### 3.1 无法量化真实任务完成率

当前单元和集成测试能够证明 Harness 契约正确，但不能证明真实用户任务的成功率。

例如，测试可以证明：

- `run.complete` 的状态流转正确；
- Build 前后能够生成 Checkpoint；
- Preview 必须有 Screenshot；
- 大文件结果可以被裁剪并持久化；
- Runtime 重启后可以恢复任务。

但无法回答：

- 一个真实 Brief 是否在第一次生成时就达到可用质量；
- 第一次失败后是否能通过一次 Repair 成功；
- Edit 是否修改了目标内容，同时没有破坏旧要求；
- 任务成功是否以不可接受的 Token、费用或延迟为代价；
- 用户补充需求后是否延续了正确上下文。

### 3.2 上下文管理仍以消息数量为中心

当前 Agent Loop 使用固定消息数量触发压缩。消息数量不能反映真实上下文成本：一个短状态消息和
一个 200 KB 工具结果会被同等计数。

主要风险包括：

- 关键决策和普通日志没有分层；
- 旧内容写入 `state/context.md` 后仍可能持续膨胀；
- 压缩可能丢失失败历史、用户约束或工具调用配对；
- 无法针对模型上下文窗口动态调整；
- 无法量化 Prompt Cache 命中和压缩收益。

### 3.3 缺少任务级预算

当前任务主要依赖固定最大轮次防止无限循环，缺少：

- Token 预算；
- 费用预算；
- 总 Wall Time 预算；
- 模型调用时间预算；
- 工具执行时间预算；
- 重复失败预算；
- Repair 次数预算。

结果是系统能够阻止极端死循环，但无法对“虽然成功、成本却不可接受”的任务进行治理。

### 3.4 完成判定仍部分依赖模型主动声明

Build/Preview 已有较强的确定性门禁，但不同 Phase 的完成条件尚未形成统一的
`CompletionPolicy`。模型调用 `run.complete` 时，Runtime 应将其视为完成申请，而不是完成事实。

### 3.5 用户难以理解任务为什么停下

Runtime 已产生事件、Checkpoint 和恢复信息，但产品层尚未形成完整的任务解释能力。用户需要看到：

- 当前执行阶段；
- 正在做什么；
- 最近一次失败及恢复动作；
- 是否等待用户输入；
- 已消耗和剩余预算；
- 当前产物、截图和构建状态；
- 可以继续、暂停、取消或从何处重试。

### 3.6 Harness 与领域实现边界仍偏重

当前拆分已经取得进展，但 `agent_loop.rs`、`conversation.rs`、`http_api.rs`、
`tools/control_plane.rs` 和 `tools/sandbox/legacy.rs` 仍承担较多职责。

如果继续把预算、上下文、调度、领域规则和产品状态加入这些模块，将进一步提高变更风险和回归成本。

## 4. 产品目标

### 4.1 核心目标

1. 建立可重复、可比较的真实任务完成率基线。
2. 在不降低生成质量的前提下，降低平均 Token、费用、耗时和无效工具调用。
3. 将任务完成从模型声明升级为 Runtime 证据裁决。
4. 让用户能够理解任务状态，并在中断、等待和失败时采取明确动作。
5. 将通用 Harness 能力与 Website/Docs 领域策略解耦。

### 4.2 非目标

- 不复制 Claude Code 的全部 CLI 命令和终端 UI；
- 不在第一阶段建设通用编程 Agent；
- 不以工具数量作为能力指标；
- 不默认引入多个可写 Workspace 的并行 Agent；
- 不以单元测试通过替代真实 Provider 和真实 Artifact 验收；
- 不在本方案中重写现有 Sandbox 或 Template 系统。

## 5. 北极星指标与指标体系

### 5.1 北极星指标

**Verified Task Success Rate（VTSR）**

定义：在给定任务预算和冻结 Eval Protocol 下，通过对应版本的 Phase CompletionPolicy，并产出满足
Brief、构建、视觉和 Artifact 验收要求的加权任务占比。

```text
VTSR = sum(caseWeight * verifiedSuccess) / sum(caseWeight * eligibleTrial)
```

模型调用 `run.complete`、HTTP 200 或生成文件存在，均不能单独计为 Verified Success。

`eligibleTrial` 排除被确认无效的 Eval Fixture；Provider、Runtime 或基础设施故障默认保留在分母，
但同时单独报告 Infrastructure-adjusted VTSR，禁止只展示对结果更有利的口径。

### 5.2 一级指标

| 指标 | 定义 | 产品意义 |
| --- | --- | --- |
| Success@1 | 不经过 Repair 的首次成功率 | 衡量 Prompt、工具与模板的初始质量 |
| Final Success Rate | 在预算内最终成功率 | 衡量 Harness 收敛能力 |
| Median Time to Verified Artifact | 从任务开始到通过验收的中位耗时 | 衡量用户等待成本 |
| Median Cost per Success | 每个成功任务的模型费用 | 衡量单位产出成本 |
| User Intervention Rate | 需要用户补充、确认或人工恢复的任务占比 | 衡量自主完成能力 |
| Recovery Success Rate | Partial/Interrupted 后恢复并成功的占比 | 衡量 Runtime 恢复价值 |

### 5.3 诊断指标

- 每任务模型轮次、工具调用数和 Tool Error 数；
- Input、Output、Cache Read、Cache Write Token；
- Model Latency、Tool Latency、Build Latency、Preview Latency；
- Context Compact 次数、压缩前后 Token、压缩失败次数；
- 相同 `errorKind + tool + action` 重复次数；
- Patch miss、stale read、大文件写入失败、dependency restore 次数；
- Build、Screenshot、Fidelity、Promotion 各 Gate 的失败率；
- Partial、Blocked、Failed、Cancelled、Needs User Input 状态分布；
- 不同模型、模板、任务类型和 Runtime 版本的成功率。

### 5.4 Eval 统计协议

所有可用于版本决策或 Release Gate 的结果必须遵守同一版本化协议：

1. 每个 Case 默认至少重复 5 次；高方差 Case 可提升至 10 次；
2. 固定 Model Route、模型版本、采样参数、Runtime Image、Template、依赖锁和 Eval Fixture Hash；
3. 同时报告 Trial 数、均值、中位数、P95 和 95% 置信区间；
4. Success@1 表示单次、无 Repair 的成功，不能用多次尝试中的最好结果替代；
5. Final Success 允许在 Case Budget 内 Repair，但必须报告 Repair 次数和额外成本；
6. 固定回归集用于持续集成，隐藏 Holdout 集用于阶段验收，禁止针对 Holdout 调整 Prompt；
7. Website、Docs、Build、Edit、Recovery、Fidelity 分别计算，再按预先冻结的权重计算总 VTSR；
8. 自动 Judge 优先使用确定性规则；主观视觉结果使用盲评，至少两名评审者，分歧交由第三人裁决；
9. Judge、CompletionPolicy、Case 或权重变更必须提升 Eval Protocol Version，不能与旧结果直接混算；
10. Cancelled、Timeout、Budget Exceeded 和 Infrastructure Failure 均保留原始状态，禁止静默重跑后覆盖。

首期权重由 Product 在运行基线前签署。若暂无业务分布数据，六个 Case 分类先等权，Phase 0 结束后
根据真实任务分布调整，并以新 Protocol Version 生效。

## 6. Benchmark 产品方案

### 6.1 Case 组成

首期建立 30 至 50 个版本化任务 Case：

| 分类 | 建议数量 | 典型任务 |
| --- | ---: | --- |
| Website Build | 8–12 | Landing Page、产品站、Dashboard、内容站 |
| Docs Build | 6–8 | Fumadocs 信息架构、导航、文档内容 |
| Website Edit | 6–8 | 文案、布局、主题、组件和响应式调整 |
| Docs Edit | 4–6 | 新增页面、重组导航、样式调整 |
| Recovery | 4–6 | Runtime 重启、工具中断、依赖失败、大文件失败 |
| Fidelity | 4–6 | Design Profile、Token、截图和 computed style 验收 |

### 6.2 Case 数据结构

每个 Case 至少包含：

```yaml
id: website-theme-edit-001
caseVersion: 1
evalProtocolVersion: 1
phase: edit
template: astro-website
weight: 1.0
trials: 5
input:
  brief: fixtures/website-theme-edit-001/brief.md
  designProfile: fixtures/website-theme-edit-001/design-profile.json
fixtureHash: sha256:...
execution:
  modelRoute: deepseek-v4-pro
  runtimeImage: anydesign-runtime@sha256:...
  templateVersion: astro-website@sha256:...
budgets:
  maxTurns: 20
  maxWallTimeSeconds: 900
  maxCostUsd: 1.00
acceptance:
  build: required
  screenshot: required
  selectors:
    - selector: ":root"
      property: "--runtime-primary"
      expected: "#f97316"
  text:
    - "Expected headline"
  forbiddenText:
    - "Old headline"
```

### 6.3 Eval 输出

每次运行生成统一的机器可读结果：

```text
evalRunId
evalProtocolVersion
caseId
caseVersion/trialId
repositoryCommit
runtimeImage
model/provider/route/sampling config
fixture/template/dependency hashes
status
completionPolicyResult
turns/toolCalls/recoveries
token/cost/latency breakdown
artifact URL and hashes
screenshot and fidelity evidence
failure taxonomy
```

支持按 Commit、Runtime Image、Model、Template 和任务分类进行回归对比。

## 7. 功能需求

### FR-1 Usage Envelope

Model Gateway 每轮响应必须记录：

- Provider 和 Model；
- Input/Output Token；
- Cache Read/Write Token；
- Model Latency；
- Transport Retry Count；
- Stop Reason；
- 估算费用；
- Provider Request ID，若可用。

Usage 必须持久化到 Run 级账本，并在任务恢复后继续累计。

### FR-2 Run Budget

每个 Run 支持以下可选预算：

```text
maxTurns
maxInputTokens
maxOutputTokens
maxTotalTokens
maxCostUsd
maxWallTimeSeconds
maxModelTimeSeconds
maxToolTimeSeconds
maxRepeatedFailure
maxRepairAttempts
```

预算达到 80% 时产生 Warning。达到硬限制时，Runtime 根据唯一状态映射保存 Checkpoint 并停止继续
申请新的模型或工具工作：

| 原因 | 目标状态 | 恢复条件 |
| --- | --- | --- |
| Token、Cost、Wall Time 耗尽且已有有效进度 | Partial | 用户或策略提高预算后 Resume |
| 需要用户确认是否提高预算 | NeedsUserInput | 用户明确批准新预算 |
| 组织策略或租户策略禁止增加预算 | Blocked | 管理策略发生变化 |
| 用户主动取消 | Cancelled | 创建新 Run，不复用旧预算 |
| Usage/预算账本损坏或无法持久化 | Failed | 修复账本后从可信 Checkpoint 新建 Run |

预算执行语义：

- 模型调用和工具调用前执行软、硬预算检查，并为本次调用预留最大估算消耗；
- 已开始且不可安全中断的副作用工具允许完成，但结果计入实际预算并禁止启动后续动作；
- Transport Retry、Fallback、Repair、Compact 和 Specialist 调用均计入总预算；
- Resume 默认继承累计消耗，只有新 Run 才能获得新预算；
- Parent Run 为 Repair/Specialist 分配子预算，子预算消耗同时计入 Parent；
- Provider 未返回精确费用时使用版本化 Price Table 估算，并标记 `costAccuracy=estimated`；
- 每次预算变更记录操作者、原因、旧值、新值和 Audit Event。

### FR-3 ContextManager

ContextManager 必须：

1. 基于 Token 而非消息数量计算上下文压力；
2. 始终保留最近完整的 Assistant Tool Use / Tool Result trajectory；
3. 将超大工具结果替换为摘要和 Artifact URI；
4. 分别持久化目标、决策、Workspace 事实、失败、待办和约束；
5. 支持主动压缩和 Provider overflow 后的响应式压缩；
6. 对连续压缩失败进行熔断；
7. 记录压缩前后 Token、保留范围和摘要版本；
8. 允许用户查看当前上下文摘要。

Runtime Store 是 Context 的唯一权威数据源。Workspace 中只允许存在可重新生成、供模型读取的只读
投影；模型工具不得直接修改权威 Context。Context 更新必须通过 ContextManager Command，并与
Checkpoint 使用同一事务或 WAL。

建议权威状态结构：

```text
runtime-store/context/{runId}/
  objective.json
  decisions.jsonl
  workspace-facts.json
  failures.jsonl
  constraints.json
  task-state.json
  compact-history.jsonl
```

Workspace 可选投影：

```text
state/context.md
state/context-manifest.json
```

Manifest 必须包含 `contextVersion`、`contextHash`、`checkpointId`、`generatedAt` 和投影范围。恢复时
只信任 Runtime Store；若投影 Hash 不一致，由 Runtime 重建，不从 Workspace 反向覆盖权威状态。

Context 中的 Secret、Credential、内部策略和不可向模型暴露的错误字段必须在投影前完成分类和脱敏。

### FR-4 CompletionPolicy

模型调用 `run.complete` 后，Runtime 执行 Phase-specific policy：

| Phase | 必要证据 |
| --- | --- |
| Brief | Schema 完整、关键字段有效、必要时已经用户确认 |
| Build | Build 成功、source snapshot、Screenshot、Fidelity、Promoted Artifact |
| Edit | 目标差异已实现、Build/Promotion 成功、旧约束未回归 |
| Review | Finding 有证据、严重级别与规则有效、范围完整 |
| Repair | 目标 Finding 已关闭、未触发相同失败循环、产物重新验证 |

Policy 失败时返回结构化缺口，允许 Agent 修复；达到预算后进入 Partial，而不是伪造成功。

每次判定输出版本化 `CompletionDecision`：

```text
decisionId
policyId/policyVersion
runId/projectId/phase
briefVersion/designProfileHash
sourceSnapshotHash/buildId
candidateId/screenshotId/artifactManifestHash
status: verified | rejected | needs_user_input
missingEvidence[]
evaluatedAt
evidenceExpiresAt
override: actor/reason/auditEventId | null
```

全部证据必须绑定同一 Run 和同一 Source Snapshot。Policy 评估必须幂等且默认无副作用；Promotion
等副作用由独立命令执行。Policy 或 Judge 版本升级不追溯修改历史结果，重新评估时产生新的 Decision。
人工 Override 只允许授权角色执行，并且不能删除原始拒绝结论。

### FR-5 Resource-aware Tool Scheduler

工具声明以下执行元数据：

```text
readResources
writeResources
exclusiveResources
dependencies
timeout
retryPolicy
idempotency
sideEffectClass
interruptBehavior
```

调度规则：

- 无冲突 Read 工具可以并行；
- 对同一路径或项目状态的 Write 必须串行；
- Build 必须等待所有 Source Mutation；
- Screenshot、Diagnostics 可以在 Build 后并行；
- Promotion 必须等待所有 Gate；
- Shell Failure 不应无条件取消无依赖、无冲突的只读工具；
- Cancellation 必须为所有已发出的 Tool Use 生成匹配结果。

Scheduler 还必须提供：

- `ResourceResolver`：在输入校验与路径规范化后生成 canonical resource key；
- 统一资源命名：`workspace:path`、`project:state`、`build:id`、`preview:id`、`artifact:id`；
- 多资源锁的全局排序和死锁检测；
- 全局、租户、Run 和工具类型四级并发上限；
- Fair Queue，避免长 Build 永久阻塞短只读任务；
- `ExecutionJournal` 记录 queued、started、side_effect_committed、result_persisted；
- Retry 复用稳定 idempotency key，非幂等工具不得自动重试；
- 工具成功但结果持久化失败时，通过 Journal 恢复，不能盲目重复副作用；
- Build、Promotion、Project Init 等跨资源动作使用事务、WAL 或补偿动作。

### FR-6 Task Explainability

产品层提供统一 Task View：

- 当前 Phase、状态和 Completion Gate；
- 当前动作与最近工具；
- 任务已运行时间；
- Token、费用和预算；
- 最近失败、是否可恢复及建议动作；
- Checkpoint、Artifact、Build Log、Screenshot；
- 用户可执行动作。

用户动作包括：

```text
补充要求
继续
暂停
取消
从最近 Checkpoint 重试
修改预算后继续
查看失败证据
打开当前 Artifact
```

### FR-7 Failure Taxonomy

统一失败分类：

```text
model.transport
model.timeout
model.rate_limit
model.context_overflow
model.invalid_tool_input
tool.permission
tool.path
tool.patch
tool.timeout
dependency.registry
dependency.install
build.compile
preview.start
fidelity.failed
promotion.conflict
runtime.recovery
budget.exceeded
user.input_required
```

所有失败必须包含：

- `errorKind`；
- `recoverable`；
- `fingerprint`；
- `suggestedAction`；
- 已尝试次数；
- 相关 Tool、Path、Build、Artifact 或 Checkpoint 引用。

### FR-8 Deferred Tool Discovery

在现有 Tool Loading Policy 基础上扩展：

- Eager：当前 Phase 高频且 Schema 小；
- Always Load：完成、用户输入和必要控制工具；
- Deferred：低频、Schema 大或外部 MCP 工具；
- Phase Restricted：仅在指定 Phase 可发现；
- Budget-aware：根据剩余上下文预算决定是否暴露完整 Schema。

首期不需要为小型内置工具引入额外发现轮次，避免为了节省少量 Token 增加模型调用。

### FR-9 Specialist Agent

仅在 Eval 数据证明有收益后，引入两个受控 Specialist：

- Visual Review Agent：只读 Artifact、Screenshot、DOM、computed style 和 Brief；
- Repair Agent：接收 Finding、相关 Source 和失败证据，只修改限定范围。

约束：

- 主 Agent 是唯一默认 Workspace Writer；
- Specialist 必须有独立 Token 和时间预算；
- Specialist 输出必须结构化；
- 不建设通用 Team/Coordinator 作为首期依赖。

## 8. 非功能需求

### NFR-1 Security 与多租户隔离

- 所有 Run、Context、Checkpoint、Artifact、Screenshot、Metric 和 Eval Result 必须带 Organization、
  Project 和 Principal Scope；
- Runtime Store、Workspace Channel、Preview 和 Artifact 访问均执行服务端授权，不能信任模型参数；
- Secret、Credential、Authorization Header、Signed URL 和环境变量在进入 Event、Trace、Context、
  Eval Result 前必须脱敏；
- Metrics Label 禁止包含 Prompt、文件内容、完整 Path、用户文本、Token 或高基数 Request ID；
- Completion Override、预算调整、Checkpoint Resume 和 Artifact Promotion 必须写入 Audit Log；
- Eval Fixture 不得包含未经批准的真实客户数据；
- Security 必须为 Context Projection、Tool Result Artifact 和 Eval 数据建立 Threat Model。

### NFR-2 Privacy 与数据生命周期

每类数据必须声明分类、存储位置、保留时间、访问角色和删除方式：

| 数据 | 默认分类 | 默认保留 | 要求 |
| --- | --- | --- | --- |
| Usage/Cost 聚合 | Internal | 180 天 | 不包含 Prompt 和文件内容 |
| Run Event/Failure | Confidential | 30 天 | Secret Redaction、项目级授权 |
| Context/Checkpoint | Confidential | 项目策略决定 | 支持项目删除和级联删除 |
| Screenshot/Artifact | Customer Content | 项目策略决定 | Signed Access、不可公开猜测 URL |
| Eval Result | Internal | 365 天 | Fixture Hash、无真实客户 Secret |
| Audit Log | Restricted | 依组织策略 | Append-only、授权查询 |

具体保留期可由组织策略覆盖，但必须有上限、删除任务和删除审计。

### NFR-3 Reliability

- Budget、Usage、Checkpoint 和 CompletionDecision 必须具备幂等写入和崩溃恢复；
- Run 状态变化采用单向状态机，非法转换必须拒绝；
- Event 至少一次投递，消费者按 Event ID 去重；
- Runtime 重启不得重复 Promotion、Project Init 或其他不可逆副作用；
- 所有外部调用有明确 Timeout、Retry、Jitter 和 Circuit Breaker；
- Cancellation 后必须回收 Process、Preview、Channel Lease、Sandbox Claim 和临时 Credential；
- Control-plane 数据不可依赖 Workspace 存活才能恢复。

### NFR-4 Performance 与容量

Phase 0 先采集基线，再冻结首批 SLO。至少覆盖：

- Run 状态和 Event 查询延迟；
- 首个 Progress Event 延迟；
- Model、Tool、Build、Preview 分段耗时；
- 单 Runtime 并发 Run 和 Tool 上限；
- Event、Metric、Context 和 Artifact 存储增长；
- Backpressure 和 Queue Wait Time；
- P50、P95、P99 与 Error Budget。

### NFR-5 Compatibility 与版本治理

- Public API、Event、Eval Protocol、CompletionPolicy、Context 和 Failure Taxonomy 均有显式版本；
- 新字段默认向后兼容，破坏性变更通过新版本和迁移器发布；
- Checkpoint 必须记录创建时的 Runtime、Context、Policy 和 Tool Contract Version；
- 旧 Checkpoint 无法安全恢复时，返回明确迁移错误并保留原始数据；
- Feature Flag 必须有 Owner、默认值、退出条件和删除日期。

## 9. API 与事件契约

### 9.1 最小 API 能力

产品面至少需要：

```text
GET  /runs/{runId}
GET  /runs/{runId}/timeline
GET  /runs/{runId}/budget
PATCH /runs/{runId}/budget
POST /runs/{runId}/pause
POST /runs/{runId}/continue
POST /runs/{runId}/cancel
POST /runs/{runId}/resume-from-checkpoint
GET  /runs/{runId}/completion-decisions
GET  /runs/{runId}/failures
GET  /runs/{runId}/artifacts
GET  /runs/{runId}/context-summary
```

Mutation API 必须支持 Idempotency Key、Principal Authorization、Expected Run Version 和 Audit Reason。
Budget 提高、Completion Override 与跨 Checkpoint Resume 需要更高权限。

### 9.2 事件类型

新增或统一以下事件：

```text
run.budget_updated
run.budget_warning
run.budget_exceeded
context.compaction_started
context.compacted
context.compaction_failed
completion.requested
completion.rejected
completion.verified
completion.overridden
tool.retrying
tool.side_effect_committed
run.recovery_available
run.recovered
run.needs_user_input
```

事件必须包含 `eventId`、`schemaVersion`、`runId`、`projectId`、`sequence`、`timestamp`、`visibility`
和去敏后的 metadata。`sequence` 在单 Run 内单调递增；SSE 重连支持 Last-Event-ID；Event Store
至少一次投递，消费者负责幂等。

### 9.3 状态与事件一致性

- 状态更新与权威 Event 使用同一事务或 Outbox；
- UI Timeline 由 Event 投影生成，不直接解析日志文本；
- Metric、Audit、Conversation 和 Event 是独立数据视图；
- 用户可见错误只暴露安全摘要，内部诊断通过受控权限查询；
- `run.completed` 只能在 `completion.verified` 或明确的非成功终态后产生。

## 10. Harness 与 Runtime 目标架构

```text
services/runtime/src/
  harness/
    loop.rs
    context_manager.rs
    budget.rs
    scheduler.rs
    completion.rs
    recovery.rs
    observability.rs
    failure_taxonomy.rs

  domain/
    brief/
    website_build/
    docs_build/
    edit/
    review/
    repair/
    fidelity/
    promotion/

  adapters/
    model/
    workspace/
    sandbox/
    artifact/
    event_store/

  api/
    runs.rs
    conversations.rs
    projects.rs
    previews.rs
    internal.rs
```

边界原则：

- Harness 不包含 Astro、Fumadocs、CSS Token 或具体页面规则；
- Domain 不直接实现模型 Transport、Checkpoint 持久化或工具并发；
- Model Adapter 必须返回统一 Usage Envelope；
- Workspace 与 Command 必须通过 Port/Adapter；
- API 只做校验、授权、映射和调度，不承载领域实现；
- CompletionPolicy 是 Runtime 权威，不由 Model 或 UI 绕过；
- Event、Metric、Audit 和 Conversation 是不同数据视图，不能混为一个日志列表。

### 10.1 增量迁移策略

目标架构不得通过一次性目录重写落地。迁移顺序如下：

1. 冻结当前 Tool、Event、API、Checkpoint 和 Preview 行为，补齐 Contract Tests；
2. 定义 Usage、Budget、Failure、CompletionDecision 和 Context DTO；
3. 保留现有 Public Module Path 与 API 作为 Compatibility Facade；
4. 先抽取纯逻辑的 Usage、Budget 和 Failure Taxonomy，不改变运行语义；
5. ContextManager 以 Shadow Projection 运行，对比新旧上下文但不影响模型；
6. CompletionPolicy 先做旁路评估，验证与现有 Gate 一致后再成为权威；
7. Scheduler 先记录 Resource Graph，再逐步开启安全并发和资源锁；
8. Observability 使用 Outbox 双写并比对丢失、重复和顺序；
9. 每个阶段通过 Benchmark、Contract Tests、Provider E2E 和 Recovery Gate；
10. Strict Architecture Gate 开启后删除 Legacy Facade 和旧实现。

每个 PR 只能选择“机械迁移”或“语义变化”之一。每个阶段必须定义 Feature Flag、Rollback 条件、
数据回滚方式和旧 Checkpoint 兼容策略。

## 11. 产品交付阶段

### Phase 0：建立可信基线

目标：先回答“当前到底有多好”。

交付：

- 30–50 个 Versioned Benchmark Case；
- Eval Runner 和统一结果 Schema；
- Usage Envelope；
- Run Budget 基础字段；
- 按 Case、Model、Template、Commit 的结果面板；
- 修复并保持本地 Gate 与真实 Provider Gate 为 Green。

退出标准：

- 所有 Benchmark 可以一条命令运行；
- 每次结果可追溯至 Commit、Model 和 Artifact；
- 能给出 VTSR、Success@1、成本和耗时基线；
- 失败能够归入稳定分类；
- Eval Protocol、Case Weight、Judge Version 和 Holdout 管理方式完成签署；
- Security 完成数据分类、Redaction 和 Retention 评审；
- 连续两次相同配置运行的指标差异处于协议定义的置信区间内。

### Phase 1：提高完成率与效率

目标：解决上下文浪费、错误完成和粗粒度调度。

交付：

- Token-aware ContextManager；
- Phase CompletionPolicy；
- 重复失败和压缩失败熔断；
- Resource-aware Tool Scheduler；
- 超大 Tool Result 摘要与 Artifact 引用；
- Budget Warning 和硬停止。

退出标准：

- VTSR 的 95% 置信区间下界不低于基线下界减 1 个百分点；
- Median Token per Verified Success 相对基线降低至少 15%，或 Median Time to Verified Artifact
  降低至少 10%，具体主目标在 Phase 1 开始前冻结；
- `run.complete` 无法绕过 Phase Gate；
- Context Overflow 100% 能够自动恢复或进入带 Checkpoint 的明确 Partial/NeedsUserInput；
- 相同 `tool + errorKind + normalizedAction` 的无效连续重复 P95 不超过 2 次；
- Budget、Completion、Context 和 Scheduler 的崩溃恢复 Contract Tests 全部通过。

### Phase 2：提升用户可用性

目标：用户能够理解和控制长任务。

交付：

- Task Timeline；
- Budget、失败和 Gate 可视化；
- Checkpoint 恢复入口；
- 补充要求、暂停、继续、取消和重试；
- Artifact、Screenshot、Build Log 快速访问。

退出标准：

- 用户能够在不查看底层日志的情况下解释任务为何停止；
- Needs User Input 有明确问题和恢复入口；
- Partial Run 可以从指定 Checkpoint 恢复；
- 取消任务后不存在遗留 Preview、Process 或 Workspace Lease；
- 90% 以上的可用性测试用户能够在不查看底层日志时正确判断任务状态和下一步；
- Timeline 首个 Progress Event 的 P95 延迟满足 Phase 0 冻结的 SLO。

### Phase 3：受控 Specialist

目标：在明确收益场景中提高视觉验收和 Repair 成功率。

启动条件：

- Eval 证明 Visual Review 或 Repair 是主要失败来源；
- 单 Agent 优化已经进入收益递减；
- Specialist 的额外 Token 成本可被成功率提升抵消。

## 12. 验收标准

### 12.1 产品验收

- 产品能够展示真实 VTSR，而不是仅展示 Run Completed 数量；
- 用户能够看到当前阶段、证据、失败、预算和下一步；
- 每个成功任务都能回溯到 Build、Screenshot、Fidelity 和 Artifact 证据；
- 每个失败任务都有稳定失败分类和恢复结论；
- Runtime 重启后预算、Usage、Checkpoint 和上下文状态保持一致。

### 12.2 工程验收

- Harness 核心模块不依赖具体 Website/Docs Framework；
- CompletionPolicy 有 Phase Contract Tests；
- ContextManager 有 Token Boundary、Tool Pair 和 Overflow Recovery Tests；
- Scheduler 有读写冲突、依赖、取消和超时测试；
- Model Adapter 有 Usage 与 Retry Contract Tests；
- 本地 Gate、真实 Provider Gate、Website 和 Docs Lifecycle 全部通过；
- Benchmark 结果可用于版本间回归阻断。

### 12.3 效率验收

Phase 0 建立统计基线并冻结 Phase 1 主目标。若签署时没有更严格目标，采用以下默认 Release Gate：

- VTSR 95% 置信区间下界不低于基线下界减 1 个百分点；
- Median Token per Verified Success 降低至少 15%，或签署的主时间指标降低至少 10%；
- 相同动作连续重复 P95 不超过 2 次；
- Context Overflow 不产生无 Checkpoint 的不明确终态；
- P95 Cost per Verified Success 不超过签署预算；
- 所有结果使用相同 Eval Protocol Version、Judge Version、Case Weight 和重复试验数。

## 13. 风险与取舍

| 风险 | 影响 | 应对 |
| --- | --- | --- |
| 过度复制 Claude Code | 产品边界失焦、工程复杂度上升 | 只吸收可验证的 Harness 能力 |
| Eval Case 过少或过拟合 | 指标提升但真实用户体验无变化 | 保留固定集与滚动真实任务集 |
| 压缩损失关键上下文 | 完成率下降 | 结构化状态、Tool Pair 保留、回放测试 |
| 并发写冲突 | Workspace 或项目状态损坏 | Resource Lock、依赖 DAG、单 Writer 原则 |
| Completion Gate 过严 | 可用任务被错误阻断 | 分级 Gate、可解释缺口、按 Phase 配置 |
| 指标采集增加延迟 | 影响任务效率 | 异步事件写入、聚合与热路径分离 |
| Multi-agent 成本失控 | 成本增加但收益有限 | 仅在 Eval 证明收益后启用 Specialist |
| Context 或 Trace 泄露客户数据 | 安全与合规事故 | Runtime 权威存储、分类、脱敏、授权和保留策略 |
| 外部研究快照被误用 | 合规与供应链风险 | Clean-room、禁止依赖和 Security/Legal 评审 |
| 终态架构一次性迁移 | 大面积回归且难以回滚 | Facade、Shadow Mode、Feature Flag 和阶段 Gate |

## 14. Ownership、依赖与签署

### 14.1 建议 Ownership

| 工作流 | DRI | 必须参与 |
| --- | --- | --- |
| Eval Protocol 与 Case | Data/Eval Lead | Product、Design、Runtime |
| Usage 与 Budget | Runtime Lead | Finance/Platform、Provider Owner |
| Context 与 Completion | Harness Lead | Product、Security、Domain Owner |
| Scheduler 与 Recovery | Runtime/Infrastructure Lead | Sandbox、Workspace Owner |
| Task View 与用户动作 | Product Lead | Frontend、Runtime、UX |
| Security 与 Privacy | Security DRI | Legal、Infrastructure、Product |

具体人员、里程碑和资源承诺必须在 Phase 0 Kickoff 前填写。没有 DRI 的条目不得进入开发中状态。

### 14.2 关键依赖

- Provider Usage 和 Request ID 可用性；
- 版本化 Model Price Table；
- Runtime Store 的事务、Outbox 和查询能力；
- Workspace Channel、Artifact Store 和 Audit Store；
- Browser、Screenshot、DOM 和 computed style 验收能力；
- Design/Product 对主观 Case 的 Judge Rubric；
- Security 对数据分类、Redaction 和 Retention 的签署；
- CI/Kubernetes 对重复 Eval 的容量与费用预算。

### 14.3 发布与回滚责任

每个 Phase 必须指定 Release Owner 和 Rollback Owner。若 VTSR、安全 Gate、Checkpoint Recovery 或
成本 Gate 回归，Release Owner 有权关闭 Feature Flag；数据迁移无法回滚时必须先停止新写入，再按
Migration Runbook 恢复，禁止依赖手工修改 Workspace 修复控制面状态。

## 15. 决策日志

| ID | 决策 | 状态 | 理由 |
| --- | --- | --- | --- |
| DEC-001 | zeronDesign 保持 Website/Docs 领域 Runtime 定位 | Proposed | 领域确定性是当前核心优势 |
| DEC-002 | `run.complete` 是申请，CompletionPolicy 是权威 | Proposed | 防止模型自证完成 |
| DEC-003 | Runtime Store 是 Context 权威，Workspace 只存投影 | Proposed | 避免篡改、双写和恢复歧义 |
| DEC-004 | Phase 0 先建立 Eval、Usage 和 Budget | Proposed | 没有基线无法判断优化收益 |
| DEC-005 | 不在首期建设通用 Multi-agent Coordinator | Proposed | 成本和写冲突收益尚未被证明 |
| DEC-006 | 外部研究快照只做 clean-room 能力研究 | Proposed | 控制合规和供应链风险 |

签署后将状态更新为 Accepted，并记录签署人、日期和替代决策。架构或指标口径变化必须新增决策，
不得静默修改已接受决策。

## 16. 决策建议

建议立即批准 Phase 0，将“继续增加 Agent 能力”的投入暂时收敛到以下五项：

1. Eval Benchmark；
2. Usage Envelope；
3. Run Budget；
4. 当前 Gate 与真实 Provider Baseline Green；
5. Security/Privacy 数据治理与 Clean-room 签署。

只有在 Phase 0 建立可信数据后，再决定 ContextManager、Scheduler、Model、Prompt、Template 或
Specialist Agent 中哪一项是任务完成率的真实瓶颈。

## Appendix A：当前评审证据快照

证据时间：2026-07-11
Git Branch：`main`
Git Commit：`d0e735621a66422e2f377979e186090a5d6061ce`
工作树：Dirty
工作树状态摘要 Hash：`e377a082c71bb81a655adf4352ec717f8686d1090470e18f22c1d846582e61ae`

本次静态对照和本地验证得到以下基线：

- zeronDesign Runtime：约 70 个 Rust Source 文件、3.7 万行 Runtime Source；
- claude-code-main：约 1900 个 Source 文件、51 万行 TypeScript Source；
- Claude Code 快照缺少可运行 Package Manifest 和测试，不能用于直接完成率对测；
- 当前 zeronDesign 核心测试结果：
  - Agent Loop：28 passed，1 ignored；
  - Sandbox Tools：89 passed，1 ignored；
  - Checkpoint：22 passed；
  - Preview Promotion：19 passed；
  - Streaming Tool Executor：9 passed；
- 当前完整本地 Gate 在 `cargo fmt --check` 阶段因 `http_api.rs` 格式差异退出，因此尚不能标记为
  完整 Green Baseline；
- Sandbox 模块拆分正在当前工作树中进行，后续优化应继续采用兼容 Facade、行为冻结、机械迁移、
  语义优化分离的方式，避免一次性重写。

该快照只用于说明本次评审依据，不作为持续更新的产品事实。后续验证必须生成独立 Evidence Record，
记录命令、时间、Runtime Image、Provider/Model、忽略测试原因和产物路径。
