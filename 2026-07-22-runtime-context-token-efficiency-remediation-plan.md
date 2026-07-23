---
date: 2026-07-22
status: implementation-in-progress
type: remediation-plan
scope: runtime-agent-loop-token-budget-prompt-cache-context-compaction-docs-preview-validation
topic: runtime-generation-reliability-and-context-efficiency
evidence_readiness: provisional-replay-bundles-required
source:
  - 2026-07-20-generation-context-and-prebuild-exploration-optimization-plan.md
  - 2026-07-16-generation-reliability-remediation-plan.md
  - 2026-07-22-provider-model-service-simplification-optimization-plan.md
evidence:
  - run-9606014
  - run-9606068
  - run-9606159
  - run-9606259
  - run-9606395
  - run-9606552
  - run-8
  - run-7
revision: 54
---

# Runtime 生成可靠性与上下文效率修复实施方案

> 实施快照（2026-07-23）：Commit 0A～0D 的主体代码路径已落地。当前包含唯一 Route Manifest、Local/Remote/
> Published Resolver 同构、Fumadocs p7、Entry Route Probe、Owner Fail-closed、≤16 KiB Repair Context、完整
> Validation/Candidate Manifest 模型读取禁用、一次 Source Repair、单调 Substantive Progress Ledger，以及
> Runtime-managed Serving 的一次确定性自动重启。Runtime、Agent Loop、Sandbox、HTTP Contract、Shared、Web
> Typecheck、Route Replay 与 Generation Reliability Contract 的本地回归已通过；真实 Provider、跨进程
> Restart Replay 与 Release Smoke 仍按第 9～10 节执行。Commit 1 的低敏感度 Prompt Composition、
> `RunPromptEfficiency@1` 和 Web 用量展示，以及 Commit 2 的 Legacy/Split Shadow/Split Enforced 代码路径也已
> 落地；Commit 3 已将 Workflow Progress 移到尾部 Ephemeral Message，并稳定 Tool Set 排序/Hash。生产默认
> 仍为 Legacy；Commit 4 已加入成功大 Tool Pair 的成对 Microcompact、Token 驱动 Full Compaction 与
> Checkpoint Projection Hash 门禁。Commit 5 已加入 Next Greenfield/Cold Dev/Warm HMR 的 Runtime Workflow
> Driver、独立 Lifecycle Event、首错即停、typed Greenfield fallback、Checkpoint 幂等恢复与失败 blocker；部署清单
> 仅配置为 `shadow`，未应用到集群。Commit 6 已先落地 `operationId`、跨 Run Operation Usage API 和 Operation
> Budget Shadow 参数；`RunContinuationSnapshot@2` 已复用 Runtime-owned Source Snapshot 的不可变 Manifest 与
> Hash 校验，并只为具备最终 Checkpoint、Workflow Progress、Substantive Progress Ledger 和已验证 Build Snapshot
> 的 Partial Run 冻结续跑输入；`ContinuationEligibilityDecision@1` 会重新验证 Source、Workspace Revision、冻结
> 身份、进度账本、剩余预算和一次自动续跑上限。单一 Runtime Continuation Coordinator 已接入标准 RunLifecycle，
> Successor 使用新 Run/Session/GenerationContext Binding，从不可变 Source Snapshot 恢复工作区，并以
> `(operationId, predecessorRunId, snapshotId)` 幂等；并发 reconcile 定向回归已通过。部署清单仍为 `shadow`，未应用
> 到集群；Build 前尚无 Source Snapshot 的预算耗尽仍失败关闭。Phase Budget Profile 已以
> `RunBudgetProfile@1` 冻结实际执行上限、Phase Shadow 目标、Token/Operation Mode 与 Profile Hash；Session 启动
> 不再为已创建 Run 重新读取这些预算，部署清单仍为 `shadow`。真实 Provider runner 已补采集低敏感度
> Prompt Composition Hash 与 `RunPromptEfficiency@1`，并新增 `ProviderCacheSmokeAudit@1`，明确区分已证明缓存、
> Provider 未报告和前缀不稳定。Cache Audit 现已强制绑定 accepted Suite、源码 Commit、`modelResourceId`、
> Provider Resource Revision、Provider Config SHA-256 与 `toolSetHashVersion=tool-definition-set@1`；Release 聚合
> 和最终 Validator 会对同一组身份再次交叉校验，最终 Validator 还会重读原始 Cache Audit 并核对原始字节 SHA
> 与 Canonical 全对象。旧模型、旧配置、旧 Hash 语义、不同 Commit 或 Dirty Source 的缓存证据均不能用于发布。尚未执行新的真实 Provider
> Cache Smoke、真实跨进程 Restart Replay、真实 Sandbox 恢复证据
> 与生产 Shadow Evidence。真实 Runner 的落盘事件现使用白名单投影：Agent Message、Tool Output/Error 与
> Terminal Summary 只保留 SHA-256/bytes，Lifecycle Prompt 只保留 Hash，Provider Request ID 只保留 presence；
> `generation-real-provider-edit/repair-evidence@2` 明确承载该破坏性脱敏升级，collector 仍兼容旧 `@1`。
> `RuntimeEvidenceBundle@1` 已实现 `real_provider_terminal` 最终 Build/已发布 Edit/Repair 与
> `runtime_restart_terminal` 组装、全文件校验和无网络
> 离线回放，且会拒绝
> 未进入稳定 Cache Audit 样本集、缺失 Source Snapshot 或身份不一致的 Build。仓库当前只提交脱敏合成
> Replay Fixture；真实 `run-7`
> Bundle 尚未满足持久化与脱敏要求，因此不得用于 Release Gate。
>
> Revision 27 根据 Harness Review 补齐 Budget Evidence 闭环：Release 不能只相信
> `budgetProfileId/budgetProfileHash`，必须冻结每个 Run 的完整 `RunBudgetProfile@1`、按 Runtime 相同的
> Canonical JSON 规则重算 Profile Hash，并用逐 Turn Provider Usage 重新验证单轮、Run 和 Operation 上限。
> Shadow Profile 只可作为 Readiness Evidence，不能凭自报 `passed` 升级为 Release Evidence。该契约已写入
> 本文。Revision 28 已将该契约落地到只读 Budget Profile API、真实 Runner/Restart Probe、Terminal
> Assembler、离线 Replay、Release Aggregator/Validator 和版本化 Production Policy；Rust/Node Canonical Hash
> Golden Test、Profile 篡改、单轮 Prompt 超限、跨 Run Operation 超限与 Shadow 冒充 Release 的失败关闭回归
> 均已通过。真实 Provider Bundle、跨 Pod Restart 与 Production Release Evidence 仍待执行，不能据此宣称
> 发布门禁已满足。
>
> Revision 29 补上跨 Attempt 完整性：`budget-profiles.json` 现在冻结 `operationId/operationAttempt`，Replay
> 要求同一 Operation 的 Attempt 从1开始连续且不重复；Build 与 Runtime Restart Bundle 会收集同一 Operation
> 的全部 Run/Event Stream，缺失前驱 Attempt 即失败关闭。Release Evidence 内嵌完整 Policy 与 Profiles，最终
> Validator 会再次执行 Profile Hash/Mode/Limit Policy，而 CLI 还会比较实际 Policy 文件的原始字节 SHA-256。
>
> Revision 30 补齐本地验收口径：完整 Runtime Library 与 HTTP API 逻辑回归在显式 8 MiB Rust 测试线程栈下
> 分别为 311/311 与 109/109 通过。默认 2 MiB test harness 会在部分完整 Router/Session 用例中发生栈溢出；
> 这不是 Provider、Budget Profile 或断言失败，但属于待清理的 Harness 工程债，不能被写成“默认全量测试通过”。
> 当前临时复现命令必须显式固定 `RUST_MIN_STACK=8388608`；后续应拆小重型 Router future 或提供统一大栈
> Test Runner，并在 CI 中冻结同一执行契约。真实 Provider、跨 Pod Restart、镜像身份与生产 Policy Evidence
> 仍未执行，文档状态继续保持 `implementation-in-progress`。
>
> Revision 31 已关闭上述 test harness 工程债：HTTP Suite 新增统一
> `run_with_http_test_stack`，只为真正启动完整 Router/Session/SSE 的重型测试创建固定 8 MiB、单线程 Tokio
> Test Runner；拒绝、鉴权和纯 Contract 等轻量测试继续运行在默认测试线程。标准
> `cargo test --lib --test http_api` 无需全局 `RUST_MIN_STACK` 即可通过 311 个 Library 与 109 个 HTTP API
> 测试。该边界只属于测试进程，不修改 Runtime Pod、Agent Budget 或 Provider 配置。
>
> Revision 32 完成只读 Budget Profile API 的恢复与鉴权闭环：持久化旧 Run 缺失 Profile 返回 409 且不按
> 当前 Pod 环境补造；持久化 Profile Hash 被篡改返回 500；不存在 Run 返回 404；未认证、权限不足与跨项目
> 请求分别按 401/403 失败，合法项目读权限返回创建时冻结的完整 Profile。标准 HTTP API 回归更新为
> 111 passed、4 ignored。
>
> Revision 33 落地统计 Benchmark Harness：`runtime-efficiency-benchmark-session@1` 冻结 Prompt Set、Design
> Profile、Template、Model Resource Revision 与 Provider Parameters Hash；append-only SHA-256 Ledger 强制
> Attempt 序号连续、ID 唯一并保留 failed/partial/timeout/rejected 终态；Assembler 从原始 Ledger 计算 SHA，
> 不能由 Cohort 自报。`runtime-efficiency-benchmark-cohort@1` 要求每个 Profile 的 Baseline/Candidate 各至少
> 30 个 Accepted Attempt 且覆盖至少 10 个 Prompt ID；不足时结果和分布均为 `insufficient_sample`，P50/P95
> 与区间保持 `null`。样本就绪后输出 Bootstrap 区间、Baseline Effect Size 与 Greenfield/Edit 阈值门禁。
> 12 个 Ledger/Evaluator 回归已通过；当前没有真实 30×2 Cohort，因此不宣称实际 Benchmark 通过。
>
> Revision 34 将 Benchmark 接入发布链路：Release Aggregate 强制要求
> `RUNTIME_RC_EFFICIENCY_BENCHMARK_LEDGER`，从原始 Ledger 执行 verify/assemble/evaluate 后嵌入完整 Cohort 与
> Evaluation，并绑定 clean Commit、Model Resource Revision、Provider Config SHA 和 Greenfield/Edit 两类
> Workload。最终 Validator 必须再次取得同一原始 Ledger，重算 raw SHA、Cohort 和 Evaluation；缺失 Ledger、
> `insufficient_sample`、阈值失败、身份漂移或内嵌统计被修改均失败关闭。当前本地合成 Contract 通过不代表真实
> Cohort 已采集，Release 仍不具备资格。
>
> Revision 35 关闭 Benchmark 样本手工拼装缺口：真实 `run-efficiency-metrics@1` 与
> `run-prompt-efficiency@1` 现在进入 hashes-only Paired Cohort Sample；
> `collect-runtime-efficiency-benchmark.mjs` 必须先验证原始 Generation Context Paired Cohort Hash-chain，
> 再把同一 Pair 的 Control/Candidate 原子导入 Benchmark Ledger。导入过程重新绑定 Workload、Prompt ID、
> Template、Model Resource/Revision/Version 与 Provider Parameters Hash，并从 Runtime 指标生成 turns、
> Gross/Uncached、单轮最大 Input、Cache Hit Rate、首次 Source Mutation、重复 Full Read、越界 Mutation 和
> Required Fidelity；任一 Accepted 样本缺指标时整批不落盘。Generation Context 大小采用 Runtime
> `generationContextEstimatedTokens × 4` 的保守字节上界，Control 为 0 合法。手工 `append` 仅保留给
> Fixture/Contract 调试，不能作为真实 Release Cohort 的采集路径。Benchmark Pair 还必须固定
> `GENERATION_COHORT_MAX_CASE_ATTEMPTS=1`；Collector 拒绝把“失败后重试成功”的多个 Case Attempt 折叠成一个
> Accepted 样本。当前 17 个 Benchmark Evaluator/Ledger/Collector Contract 与完整 Generation Reliability Gate 已通过，
> 但仍没有真实 30×2 Cohort，Release 资格不变。
>
> Revision 36 关闭“Collector 只靠约定、Release 无法证明来源”的缺口：Release mode 除 Benchmark Ledger 外，
> 现在强制要求 `RUNTIME_RC_EFFICIENCY_SOURCE_LEDGER` 和 `RUNTIME_RC_EFFICIENCY_IMPORT_MAPPING`。Aggregate 与
> Final Validator 都会验证原始 Paired Cohort Hash-chain，并用 Mapping、冻结 Benchmark Session 和相同派生规则
> 重新生成全部 Attempt，再与 Benchmark Ledger 逐字段比较；手工 append 即使 Schema、Hash-chain 和统计阈值均
> 合法，只要不等于源 Ledger 的确定性派生结果也失败关闭。`runtime-efficiency-benchmark-source-binding@1`
> 内嵌 Paired Session、原始 Ledger SHA/Head Hash、Mapping Hash 与 Attempt 数。当前 18 个 Benchmark Contract
> 与完整 Generation Reliability Gate 已通过；真实 Cohort 和发布证据仍未采集。
>
> Revision 37 补齐 Design Profile 数据血缘：真实 Build/Edit/Repair Runner 会从只读
> `/runs/{runId}/design-context-manifest` 投影 `run-design-profile-identity@1`，只冻结 Profile ID/Version 与
> `effectiveProfileHash`；Paired Collector 要求 Control/Candidate Hash 相同并写入 Pair Identity，Rollout
> Ledger/Evaluator 将其作为必需 SHA-256 字段，Benchmark Import 再与 Session Profile 的
> `designProfileHash` 精确比较。这样“同 Prompt/Template/Provider、但实际用了不同 Design Profile”的样本不能
> 混入同一分布。相关 Runner、Pair、Ledger、Rollout 与 Benchmark 负向回归已通过。
>
> Revision 38 将 Paired Cohort 的源码和 Prompt Corpus 从旁路元数据提升为 Hash-chain 身份：
> `generation-context-paired-cohort-session@1.source` 冻结 Commit/Dirty，且 Pair Runner 要求其与
> `session-meta.json` 一致；Benchmark Import 要求 Session Source 完全一致，并要求
> `promptSet.sha256 == fixtureManifestSha256`。Source Binding 将该 Source 写入 Release Evidence，Aggregate/
> Final Validator 再与 clean Release Commit 交叉验证。不同 Commit、Dirty Paired Source 或另一个 Prompt
> Manifest 不能复用现有 Benchmark。当前 19 个 Benchmark Contract 与完整 Generation Reliability Gate 通过。
>
> Revision 39 补齐真实 Cohort 的可执行入口：保留原五用例 Functional Canary 不变，新增独立的
> `runtime-efficiency-benchmark-cases.json`，冻结 10 个不同的 Design System Website Prompt；
> `run-runtime-efficiency-benchmark-cohort.sh` 默认规划每个 Prompt × Greenfield/Style Edit × 3 repetitions，共
> 60 个 Pair、120 个 Baseline/Candidate Attempt，强制 clean Paired Session、同一 Corpus SHA 和单次 Case
> Attempt，并支持只读 Dry Run 与已完成 Pair 跳过。`prepare-runtime-efficiency-benchmark.mjs` 在每个 Workload
> 覆盖 10 个 Prompt 后，从 Paired Ledger 自动派生 Profile、Prompt Set、Mapping 和空 Benchmark Ledger，拒绝
> 混合 Design Profile/Template/Provider 或混合 Cache Capability。新增 4 个 Preparation/Planner Contract 后，
> Benchmark 本地 Contract 共 23 个；未发起任何付费调用。
>
> Revision 40 收紧“可恢复”的 Harness 语义：真实 Cohort 末端改用幂等 `sync`。若存在新 Pair Side，则先全批
> 验证再原子追加；若没有新增 Attempt，则必须重新验证 Benchmark Ledger 与完整 Paired Source Binding 后成功
> 退出。这样已完成 Cohort 的重复启动不会进入无意义诊断，也不会为了让 Collector 产生输出而重复付费调用；
> 底层无新增即失败的严格 Collector API 仅保留给 Fixture 与审计测试。
>
> Revision 41 将付费安全校验前移：Cohort Planner 在生成执行计划或调用 Pair Runner 前，必须先验证 Paired
> Ledger 的完整 Hash-chain、Sample Schema 和同 Pair 身份一致性。篡改记录不能仅凭存在 Control/Candidate 两侧
> 就被当作“已完成”跳过；Preflight 失败时 Provider Runner 调用数必须为0。
>
> Revision 42 将 Corpus 语义检查从仓库 Contract 下沉到付费 Runner：即使 Session 中冻结的 SHA 与传入文件
> 一致，仍必须验证版本、10 个唯一 Prompt/ID/Expected Text、Website Kind、`/` Route 以及每条 Prompt 的
> Design System 主题；错误 Corpus 在 Provider 调用前失败关闭。
>
> Revision 43 补齐 Full Compaction 的实现口径：除消息数、Conversation Token 和最大消息 Token 外，Runtime
> 现在独立计算总序列化字节、最大消息字节和下一轮完整 Model Request Token；超过 14k 的下一轮请求会在发送
> 前先尝试压缩。Compaction Event 冻结 `triggerReasons`、Byte 与 Next-request 指标，便于解释触发原因。新增异步
> 路径使用 boxed future，并将重型 Compaction 集成用例接入有界 8 MiB Test Runner。完整 Runtime Library
> 314 passed（另2个环境 Canary ignored）、AgentLoop 62 passed（另1个真实 Provider ignored）通过；AgentLoop
> 测试二进制仍需要命令级 8 MiB 栈，
> 因此本地 Harness、Phase A 全量入口和 CI AgentLoop Steps 已显式冻结该值，不影响 Runtime Pod 或其他测试。
> `next_request_tokens` 仅在超额可归因于可压缩 Conversation 时触发；若 Tool/System 固定开销自身已超过目标，
> 由 Prompt Budget/Tool Policy 失败关闭，禁止在无法降到目标时反复压缩制造新的诊断轮次。
>
> Revision 44 修正 Source Restore 上限与身份：Build 最多恢复 8k Token，Edit/Review/Repair 最多4k，其他 Phase
> 不自动恢复；每个文件仍不超过4k、总数不超过5。候选项冻结 Observation 的 Path/Content SHA/Token Estimate，
> 读取后重新计算并逐项匹配，Source 已变化、估值漂移或实际总量超限都会跳过并记录低敏感度计数，不能把未观察
> 的当前文件内容借 Compaction 重新注入模型窗口。
>
> Revision 45 修复 Bounded Repair Context 的边界口径：Runtime 现在分别验证 `serializedBytes ≤ 16 KiB` 与
> `estimatedTokens ≤ 4,000`，不再让 16,001～16,384 bytes 仅凭字节条件通过；Recoverable Evidence 同时返回
> 实际 bytes/tokens，Repair Cycle 仍固定最多一次，完整 Validation/Candidate Evidence 继续禁止模型读取。
>
> Revision 46 收紧 Stable Tool Set 身份：`toolSetHash` 不再只哈希工具名称，而是哈希按名称稳定排序后的完整
> Eager/Deferred Tool Definitions，包括 Input/Output Schema、Loading Policy 与 MCP Identity。仅改变枚举顺序 Hash
> 不变，任一 Schema/Policy 变化 Hash 必变；因此旧名称级 Cache Evidence 不再具备新 Release 的身份资格。
>
> Revision 47 完成本轮当前态审计：Runtime Library 314 passed/2 ignored、AgentLoop 62 passed/1 real-provider
> ignored、完整 Generation Reliability Gate passed。只读外部审计仍显示 HEAD `9185b1fe...`、192 个 dirty
> entries、Provider 凭据缺失，主/Control/Candidate 均为旧 dirty 镜像；因此仅关闭本地条目，真实 Evidence 与
> Release Gate 保持未完成，未执行集群写入、Pod Restart 或付费调用。
>
> Revision 48 对完整 Tool Definition Hash 做证据版本化：`prompt.composition`、脱敏 Event、Case Evidence、Cache
> Audit 和 Terminal Bundle 均冻结 `toolSetHashVersion=tool-definition-set@1`，缺失版本的名称级旧证据失败关闭。
> Final Validator 新增必需原始输入 `--provider-cache`，重新计算原始文件 SHA-256，并将 Canonical 全对象与
> Aggregate 内嵌 Cache Audit 比较；同步篡改 `status/releaseEligible/Hash` 不能替代原始证据。为保持历史数据
> 可读，Runtime 与 Shared Event Schema 允许反序列化缺少该字段的旧 `prompt.composition`；兼容只发生在读取边界，
> Cache Audit、Terminal Bundle、Aggregate 与 Final Validator 仍要求精确版本，旧证据绝不具备 Release 资格。
> 本地完整回归更新为 Runtime Library 315 passed/2 ignored、AgentLoop 62 passed/1 real-provider ignored，完整
> Generation Reliability Gate passed。
>
> Revision 49 校正 Definition of Done 的本地/外部边界：Terminal Bundle 已在本地冻结场景涉及的全部 Run
> Profile，Rust 与 Node 都以仓库 Production Policy Hash 验证同一 Canonical Profile；Replay 已从脱敏事件逐
> Turn 重算并独立执行 Per-turn、Run、Operation 和 Release Policy 校验；Repair Bundle 已强制包含 Setup Edit、
> Review、Repair 三个 Run。上述三项改为本地完成，但真实 Bundle、Provider 身份和生产终态 Gate 仍保持未完成。
>
> Revision 50 冻结 Legacy 预算默认边界：新增回归明确要求默认仍为 `legacy`、20 模型轮次和 200,000
> Input Token，不允许通过无审计地抬高旧 Gross 上限掩盖重复上下文；显式环境变量和冻结 Run Profile 的可配置
> 能力保持不变。Runtime Library 本地基线相应更新为 316 passed/2 ignored。
>
> Revision 51 修复旧 `RuntimeEvidenceBundle@1` 的 Usage 回放弱绑定：Replay 不再只比较 Aggregate，而是按 Turn
> 采用“同 Turn 最新事件生效”语义重算完整 `runtime-evidence-usage@1`，同时验证 Cached 不大于 Input，并将
> `schemaVersion/turns/aggregate` Canonical 全对象与 Bundle 比较。新增“逐 Turn 数值被重分配但总量不变”的篡改
> 用例，更新 Checksums 后仍必须失败关闭。
>
> Revision 52 收紧 Evidence 原文脱敏别名：除 `prompt/sourceCode` 外，`systemPrompt`、`userPromptText`、
> `promptContent`、`sourceText`、`sourceContent`、`fullSource`、`rawSource` 等字段也统一转换为 Hash/Bytes，Redaction
> Validator 对未经脱敏的同义字段失败关闭；Hash、Bytes、Source Snapshot URI/Fingerprint 等低敏感度身份字段不受
> 影响。真实日志/API/Evidence 的零泄露结论仍必须由终态 Secret Scan 证明。
>
> Revision 53 将 `RunModelUsage@1` 变成旧 Replay Bundle 的直接受校验输入：Bundle 必须包含
> `run-model-usage.json`，Replay 以同 Turn 最新 `model.usage` 为准，重算 Run/Model 身份、Input/Output/Cached/
> Total、Estimated 和 Turn Count 后做 Canonical 全对象比较。只修改投影并重签 Checksums 仍失败关闭，不再依靠
> 字段相似性间接声称 API 兼容。
>
> Revision 54 将同一 `RunModelUsage@1` 约束扩展到所有终态 Bundle：Website、Docs、Edit、Repair 与 Restart
> 都生成 `runtime-evidence-run-model-usage@1`，包含 Bundle 内每个 Run 的完整 API 投影；Manifest、Case Summary、
> Checksums、Replay Expectations、Terminal Index、Aggregate 和 Final Validator 共同冻结其 SHA-256。即使同步
> 篡改投影、Manifest、Case Summary 与 Checksums，事件重算不一致仍失败关闭。

## 1. 文档结论

当前作品生成和修改链路的 Token 消耗在统计上是可信的，但在产品效率上不符合预期。

问题不在 `GenerationContext@1` 过大。本次真实运行中，Build 的 GenerationContext 为 3,862 bytes，
约 966 Token；Edit 为 3,446～3,622 bytes，约 862～906 Token，只占累计 Input Token 的约 6%。
真正造成消耗膨胀的是：

1. Runtime 将每轮完整 Prompt 的 `inputTokens` 累加为 Run 用量，同一静态上下文会随模型轮次重复计算；
2. 动态 `Runtime Workflow Progress` 被拼接在 System Prompt 前部，阶段变化会破坏 Provider 前缀缓存；
3. 完整 `fs.write`、`fs.patch` 等 Tool Call 参数在执行成功后仍长期保留在活动消息窗口；
4. 当前压缩仅按消息条数触发，短轮次但大消息的 Run 不会及时压缩；
5. 依赖恢复、Build、Dev、Snapshot 等确定性生命周期动作仍需要模型逐轮驱动；
6. Run 级 200,000 Input Token 预算把 Cached Input 按全额累计，混淆了用量观测、成本保护和单轮上下文安全；
7. Partial 后的新 Run 没有复用前一 Attempt 已验证的源码与 Workflow Progress，导致完整任务从头执行。
8. Fumadocs 模板导出 `out/docs.html`，本地 Preview 却用 Python 静态服务器验证 `/docs/`，该请求命中
   `out/docs/` 数据目录而不是文档 HTML；
9. Runtime 将 Preview Serving/Route Resolution 错误统一归类为 Source Repair，要求模型修改本来正确的源码；
10. 不同的 `fs.read/fs.search` 观察会改变 Progress Fingerprint，导致诊断读取被误判为有效进展，No-progress
    Fuse 无法及时终止循环。

因此本方案不以提高 `RUNTIME_AGENT_MAX_INPUT_TOKENS` 为修复。目标是将以下三件事拆开治理：

```text
Provider Known Usage
  = Provider 返回的 Gross Input / Cached Input / Output 事实

Run Economic Guard
  = 非缓存 Input、Gross Input、Output 和模型轮次的独立上限

Context Safety Guard
  = 单次模型请求的 Prompt Token 上限
```

修复完成后，Runtime 应保持 Generation Context、Design Profile、Style Contract、Mutation Lease、
DraftSnapshot、WorkVersion 和 PublishWorkflow 的既有安全边界，同时显著减少重复 Prompt、无效模型轮次和
跨 Attempt 重做。Docs 还必须保证 Template、Build Artifact、Preview Server 和 Validator 对同一个
Artifact Entry Route 使用完全一致的解析规则；平台层错误不得伪装成用户源码错误。

本文是跨模块 Master Plan，不以自然语言描述代替可执行 Harness 规范。Route Resolution、Failure
Classification、Transcript Projection 和 Benchmark 分别通过版本化 Contract、Fixture Corpus、Replay
Bundle 和 Gate Manifest 固化；实现与测试不得各自维护第二套隐式规则。

### 1.1 当前落地边界

| 能力 | 当前状态 | 本轮证据/限制 |
|---|---|---|
| Route Oracle、p6/p7 兼容 Resolver | 已实现 | Candidate Build 生成并绑定 Route Manifest；歧义路由失败关闭 |
| Entry Route Probe 与 Failure Owner | 已实现 | Source 仅在 Probe 通过后可成为 Owner；平台 Owner 不进入模型 Repair |
| Bounded Repair | 已实现 | 模型只可读 `state/repair-context.json`；最多一次 Source Repair/Republish |
| Serving 自愈 | 已实现 | 仅 Runtime-managed Preview；重启预算先持久化，最多一次，之后重新 Probe |
| Substantive Progress Ledger | 已实现 | Read/Search、重复 Build ID 与 Stage 变化不重置主 No-progress Fuse |
| Prompt/Token 可观测性 | 代码与本地 Contract 已通过，历史/Provider Gate 待完成 | `prompt.composition`、`RunPromptEfficiency@1`、Web Gross/Cached/Uncached |
| Split/Phase Budget | Legacy/Split/Phase Shadow/Enforced 本地路径已实现，生产 Evidence 待完成 | `RunBudgetProfile@1` 在 Run 创建时冻结；默认 Legacy + Phase Shadow；生产证据通过后才能 Enforce |
| Stable Prefix | 本地稳定性与 Evidence 身份绑定回归通过，Provider Smoke 待完成 | Workflow Progress 作为尾部 Ephemeral Message；Tool Set 稳定排序与 Hash；Cache Audit 绑定 Commit/Model Resource/Revision/Config Digest |
| Projection/Microcompact | 本地配对、Hash 与篡改回归通过，跨进程 Restart/Replay Gate 待完成 | 成功大 Tool Pair 成对摘要；Token/消息阈值；Checkpoint Projection Hash；Restart evidence 强制 Budget Profile 身份不漂移 |
| Workflow Driver | 代码与定向回归已通过，生产 Shadow 待执行 | 独立 Lifecycle Event；首错即停；Restart 恢复；Fumadocs 仍保持单入口 |
| Operation Usage/Budget | 本地 Contract 已通过，生产 Shadow 待执行 | `operationId` 跨 Run 汇总 Token/轮次/延迟；Operation Budget 默认 `shadow` |
| Efficiency Benchmark | Corpus/Planner/Preparation/Collector/Source Binding/Ledger/Evaluator/Release 本地 Contract 已通过，真实 Cohort 未采集 | 独立 10-Prompt Design System Corpus；默认 60 Pair/120 Attempt 执行计划；Runtime 原始指标经已验证 Paired Hash-chain 原子导入；Effective Design Profile Hash、Template、Provider、Prompt 全链路绑定；Release 重读三份原始输入并重新派生 |
| Partial Continuation | Snapshot/Eligibility/Coordinator/Successor 本地路径已实现，生产仍为 Shadow | 只接受 Runtime-owned Source Snapshot + Hash + Checkpoint + Workflow/Progress Ledger；标准 RunLifecycle 恢复；身份漂移/Build 前无快照时失败关闭 |
| Release Evidence | 本地 Fail-closed Contract 已实现；真实证据未完成 | Release mode 强制逐 Bundle Replay、Credential Scan、五类场景覆盖、完整 Budget Profile/逐 Turn/Operation 回放及独立 Production Policy；真实 Bundle 与终态 Smoke 尚未冻结 |

“已实现”表示代码路径和针对性回归存在，不等于已经满足 Definition of Done。只有第 9 节分层测试与第 10 节
发布门禁全部通过，才能将文档状态改为 `completed`。

### 1.2 本轮本地验证证据

以下结果来自 2026-07-23 当前工作区，不包含真实 Provider 请求、Kubernetes 资源变更、Runtime Pod 重启或
生产发布：

| 验证面 | 结果 |
|---|---|
| Runtime Library | 316 passed，2 ignored；ignored 为需要 npm Registry/完整发布环境的 Canary |
| Runtime HTTP API | 标准命令 111 passed，4 ignored；完整 Router/Session/SSE 用例统一使用有界 8 MiB Test Runner；Budget Profile 覆盖成功、404、旧 Run 409、篡改 500 及 401/403/200 鉴权矩阵 |
| Agent Loop Integration | 62 passed，1 ignored；ignored 为需要真实 DeepSeek Key 与网络的 E2E |
| Sandbox Tools | 92 passed，1 ignored；ignored 为真实 Fumadocs 安装与 Production Build Smoke |
| Template Registry | 13 passed |
| HTTP Contract Manifest | 5 passed |
| Shared Package | 43 passed；TypeScript typecheck passed |
| Web | Shared prebuild 与 TypeScript typecheck passed |
| Artifact Route Replay | 2 passed；Node Projection 与 Rust Corpus 一致，脱敏 Fixture 可离线重放 |
| Generation Reliability Contract | passed；包含脚本语法、离线证据、预算自检、Kustomize Render 与 Deployment Contract，不连接真实 Provider |
| Efficiency Benchmark Contract | 23 passed：8 个统计/Evaluator、5 个 Hash-chain/原子 Ledger、6 个 Paired Collector/Source Binding、3 个 Preparation、1 个 Cohort Planner；独立 Corpus 为 10 个 Design System Website Prompt；真实 Cohort 未采集 |
| Provider Cache / Release Evidence | Cache Audit、Terminal Bundle Set Replay、完整 Budget Profile 回放、Production Policy、Release Aggregate、Release Validator 定向回归 passed；包含 Dirty Source、Provider Revision 不匹配、Profile Hash 篡改、单轮/Operation 超限、Shadow Profile、缺失 Restart Bundle 和复制 Website 冒充 Docs 的失败关闭用例；真实终态证据尚未生成，故整体 Release Gate 仍未闭环 |
| Evidence Redaction | 真实 Runner 事件投影、Lifecycle Prompt Hash 与 Audit Fail-closed 已实现，真实 Bundle 冻结待完成 | Audit 校验事件路径边界、SHA、事件数和禁用字段；旧 raw stream 不具备发布资格 |
| Real-provider Release Preflight | 已实现，真实执行待完成 | Release mode 在任何协调/付费调用前要求 Clean Commit、Prepared Session 同 Commit 与 Readiness Probe；Audit mode 不冒充 Release |
| RuntimeEvidenceBundle | Build/已发布 Edit/Repair/Runtime Restart Assembler 与 Offline Replay 已实现，真实 Bundle 待冻结 | 冻结 Route/Candidate/Source、Context/Budget、Stream/Usage、HTTP Acceptance、Sandbox Release 与 Cache 身份；Repair 覆盖三条 Run Stream；Restart `@2` 绑定 Deployment/Pod 替换和两次清理确认；Draft/HMR Edit 仍属于外层生命周期中间证据 |
| 格式与补丁完整性 | `cargo fmt --check`、`git diff --check` passed |

Runtime/HTTP 全量逻辑回归的当前复现命令为：

```bash
cargo test --manifest-path services/runtime/Cargo.toml --lib --test http_api
```

统一 Test Runner 的 8 MiB 是重型测试的显式上限，不是全局环境变量，也不修改 Runtime Pod、Agent Budget 或
Provider 参数。新增完整 Session 测试若需要该 Runner，必须显式选择；若 8 MiB 仍溢出，Gate 直接失败，不允许
通过继续放大栈掩盖无界递归。

本表只证明本地实现与离线契约一致。它不能替代以下发布证据：固定版本真实 Provider Cache Smoke、真实 Runtime
跨进程 Restart Replay、至少 30 个同分布样本的分位数统计、Candidate Shadow Evidence、K3d/Production
终态 Smoke。上述证据未完成前，Phase Profile、`split_enforced` 默认值和 Workflow Driver/Continuation 均不得
进入生产强制路径。

2026-07-23 只读集群审计显示 `k3d-zerondesign-e2e` 当前 Runtime 仍运行
`anydesign/runtime:6a5603c0a464-dirty-bb6cdfe816f4`，Generation Candidate/Control 也仍是旧 dirty 镜像；主 Runtime
尚未配置 Continuation、Phase Budget 或 Split Budget 新环境变量。本地工作区有 192 个 dirty entries，且当前
进程没有 `DEEPSEEK_API_KEY` 或 `MODEL_GATEWAY_AUTH_TOKEN`。因此该集群只能作为历史诊断环境，不能证明本工作区实现，也不能生成 release-eligible
终态证据；本轮未修改集群、未重启 Pod、未发起付费 Provider 请求。

## 2. 当前真实证据

### 2.1 本次作品的模型用量

主题为“什么是 Design Engineer”的 Website，使用真实 Runtime API 和 `deepseek-v4-pro@4`。随后通过真实
Edit API 修改为亮色主题。

| Run | 结果 | 模型轮次 | Input Tokens | Cached Input | 非缓存 Input | Output Tokens | 单轮最大 Input |
|---|---|---:|---:|---:|---:|---:|---:|
| `run-9606014` Brief | completed | 4 | 25,390 | 18,432 | 6,958 | 1,001 | 6,924 |
| `run-9606068` Build | partial，预算耗尽 | 13 | 202,763 | 88,320 | 114,443 | 6,707 | 19,401 |
| `run-9606159` Build | partial，预算耗尽 | 14 | 218,278 | 115,328 | 102,950 | 8,154 | 20,151 |
| `run-9606259` Build | completed | 12 | 177,662 | 25,472 | 152,190 | 6,328 | 18,933 |
| `run-9606395` Edit | partial，no progress | 18 | 267,950 | 149,760 | 118,190 | 4,074 | 19,482 |
| `run-9606552` Edit | completed | 19 | 268,812 | 124,800 | 144,012 | 6,207 | 18,668 |
| 合计 | 3 completed / 3 partial | 80 | 1,160,855 | 522,112 | 638,743 | 32,471 | 20,151 |

口径说明：

```text
uncachedInputTokens = max(inputTokens - cachedInputTokens, 0)
totalTokens = inputTokens + outputTokens
```

`cachedInputTokens` 是 `inputTokens` 的组成部分，不能再次加入 Total。当前六个 Run 的整体缓存比例约为
45%。

### 2.2 用户任务级消耗

单独看最终成功 Run 会低估真实用户成本。应同时提供跨 Attempt 的 Operation 视图：

| 用户操作 | Gross Input | Cached Input | 非缓存 Input | Output | 说明 |
|---|---:|---:|---:|---:|---|
| 首次生成（Brief + 三次 Build） | 624,093 | 247,552 | 376,541 | 22,190 | 两次 Build 从头重跑 |
| 修改为亮色主题（两次 Edit） | 536,762 | 274,560 | 262,202 | 10,281 | Style Contract 与 PVC 恢复后重跑 |

最终成功 Build 的 177,662 Input 相比旧基线中位数 293,959 下降约 39.6%，说明 Generation Context
方向有效；但没有稳定达到原方案约 50% 的目标。首次生成按用户操作计算达到 624,093 Input，已经超过
旧基线中位数约 112%，不能被单次成功 Run 的改善掩盖。

对于“修改为亮色主题”这种范围明确的 Token/CSS Edit，19 个模型轮次和 268,812 Input 也明显偏高。

### 2.3 不存在单轮 Context Overflow

所有 Run 的单轮 Input 峰值都低于 20,200 Token。两个 Build 终止是因为 Run 累计 Input 超过 200,000，
不是模型单次上下文窗口耗尽：

```text
run-9606068: gross=202,763, cached=88,320, uncached=114,443
run-9606159: gross=218,278, cached=115,328, uncached=102,950
```

因此当前错误文案 `Run token budget exhausted` 虽然描述了实现事实，却容易被理解为模型 Context
Overflow。API、事件和 UI 必须明确区分 `run_gross_input_budget_exhausted`、
`run_uncached_input_budget_exhausted` 与 `turn_prompt_budget_exhausted`。

### 2.4 GenerationContext 大小符合预期

| Run 类型 | Serialized Bytes | 估算 Token | Payload 上限占用 |
|---|---:|---:|---:|
| Build | 3,862 | 966 | 约 5.9% / 64 KiB |
| Edit 第一次 | 3,446 | 862 | 约 5.3% / 64 KiB |
| Edit 最终成功 | 3,622 | 906 | 约 5.5% / 64 KiB |

GenerationContext 继续保持完整性校验、双 Hash、Run Binding 和冻结语义。本方案不通过删除 Acceptance、
Content Plan、Design Rules、Editable Surface 或 Provenance 来节省 Token。

### 2.5 本次 Design System Website / Docs 证据

2026-07-22 使用真实 `deepseek-v4-pro@4` 分别执行 Design System Website 和 Fumadocs Docs：

| Run | 结果 | 模型轮次 | Gross Input | Output | 关键现象 |
|---|---|---:|---:|---:|---|
| `run-8` Website | partial | 14 | 210,898 | 10,100 | 源码完整且独立 Production Build 通过，但累计 Input 超过 200k |
| `run-7` Docs | partial | 20 | 325,034 | 5,506 | Build 与 14 个检查通过，Preview 路由误解析后进入 9 轮诊断 |

`run-8` 再次证明不是单轮 Context Overflow，而是活动历史随轮次重传造成的累计预算耗尽。

`run-7` 的 `preview.publish` 在第 11 轮已经完成 110 个静态文件的 Production Build。验证报告中 Build、
Artifact Integrity、Desktop/Mobile、Accessibility、Responsive、Link、MDX、Navigation、Heading 和 Code
Block 均通过；失败项为 Metadata 和 Search Index。Browser Evidence 显示实际打开的是：

```text
requested route: /docs/
final URL:       http://127.0.0.1:<port>/docs/
page title:      Directory listing for /docs/
expected file:   out/docs.html
conflicting dir: out/docs/
```

这不是生成内容缺少 Metadata/Search，而是本地 Preview Server 使用 `python3 -m http.server`，它优先把
`/docs/` 解释为目录；远端 Candidate Server 的 clean-URL resolver 则会回退到 `docs.html`。同一 Artifact
在不同执行后端具有不同路由语义，是平台契约错误。

失败后第 12～20 轮依次读取完整 Validation Report、猜测不存在的 `layout.tsx`、搜索和读取 JSX/MDX。
完整报告包含 Candidate Manifest 和 110 个文件条目，使单轮 Input 从约 11.9k 上升到 24.1k，最终达到
32.9k。期间没有新 Candidate，也没有完成有效 Source Repair。

### 2.6 当前证据可重放性缺口

Frontmatter 中的 Run ID 只用于关联本轮分析，尚不能单独作为 Release Evidence：本地临时 Store 中的
`run-7/run-8` 可能与其他 Store 重名，`/tmp` Stream 和 `/var/folders` Workspace 也不具备持久性。特别是
`run-8` 没有冻结完整 Event Stream，因此不能把“八个 Run 可精确历史重算”作为当前已满足的前提。

发布证据与 Fixture Route Replay 是两种不同用途的 Bundle：

- `real_provider_terminal`：用于真实 Provider 终态发布门禁，Sandbox 释放后只冻结 Route/Candidate/Source
  身份 Hash、HTTP Acceptance 与 Cleanup 结果，不复制真实用户的 Route Manifest 正文；
- Fixture Route Bundle：用于无真实用户内容的 Route Resolver 离线回放，可包含已脱敏的
  `artifact-route-manifest.json`。

任何用于 Commit Exit 或 Release Gate 的真实 Provider 执行必须先生成脱敏
`RuntimeEvidenceBundle@1`：

```json
{
  "schemaVersion": "runtime-evidence-bundle@1",
  "bundleKind": "real_provider_terminal",
  "evidenceId": "design-system-docs-20260722-attempt-1",
  "gitSha": "...",
  "harnessRevision": "runtime-terminal-evidence@1",
  "runtimeMode": "real_provider",
  "projectId": "...",
  "runId": "run-7",
  "operationId": "...",
  "modelResourceId": "deepseek-v4-pro",
  "modelResourceRevision": 4,
  "providerConfigSha256": "...",
  "promptSha256": "...",
  "generationContextHash": "...",
  "runContextBindingHash": "...",
  "budgetProfileId": "phase-default",
  "budgetProfileHash": "...",
  "budgetProfilesSha256": "...",
  "templateVersion": "fumadocs-docs@runtime-p7",
  "candidateManifestHash": "...",
  "artifactRouteManifestHash": "...",
  "sourceFingerprint": "...",
  "streamSha256": "...",
  "result": {"status": "accepted"}
}
```

Bundle 输出目录由 Assembler 的 `--out` 显式指定；CI Artifact 建议按
`services/runtime/evidence/replay/<evidenceId>/` 归档。Assembler 拒绝覆盖已存在目录。终态 Bundle 固定包含：

- `manifest.json`：上述身份、命令、Hash 和结果；
- `events.ndjson`：脱敏后的 Append-only Run Events；
- `usage.json`：逐 Turn Provider Usage 与聚合；
- `budget-profiles.json`：场景内每个 Run 的完整冻结 Profile、Profile Hash 和 Phase；
- `case-summary.json`：冻结 Prompt/Acceptance/Context/Budget/Template 身份；
- `artifact-route-identity.json`：只含 Runtime 实际产生的 Route/Candidate/Source Hash 与 HTTP Probe；
- `validation-summary.json`：只含固定 Check/Owner/Status，不含页面正文；
- `provider-cache-summary.json`：冻结 Cache Audit 的 Commit/Model/Revision/Config/Tool Hash Version 身份及源审计 Hash；
- `checksums.sha256`：Bundle 内文件校验和。

不得保存 Provider Key、Authorization、完整 Prompt、用户源码或截图像素。需要复现页面内容时使用已脱敏的
固定 Fixture Artifact；真实用户 Artifact 只保存 Hash。`assemble-runtime-terminal-evidence.mjs` 只允许选择
accepted 且已进入稳定 Cache Audit 样本集的最终 Build，并要求 Clean Commit、完整 Source Snapshot、
Compiled Generation Context、Provider Request Presence、HTTP 200 Acceptance 与 Sandbox Release。
`replay-evidence.mjs` 在无网络、无真实 Provider 的条件下验证全部校验和、脱敏约束、跨文件身份，
并从 Event Stream 重算 Usage。Fixture Route Bundle 另外重放 Route Probe、Failure Owner 和 Progress Ledger。

### 2.7 Budget Profile Evidence 契约

`budgetProfileId/budgetProfileHash` 只能证明调用方声称使用了某个 Profile，不能证明该 Profile 的限制值，也
不能单独证明实际用量合规。终态 Bundle 必须增加 `budget-profiles.json`：

```json
{
  "schemaVersion": "runtime-evidence-budget-profiles@1",
  "profiles": [
    {
      "runId": "run-7",
      "operationId": "operation-7",
      "operationAttempt": 1,
      "phase": "build",
      "profile": {
        "schemaVersion": "run-budget-profile@1",
        "profileId": "phase-default-build",
        "phase": "build",
        "rolloutMode": "enforced",
        "tokenBudgetMode": "split_enforced",
        "operationBudgetMode": "enforced",
        "enforcedLimits": {
          "maxTurns": 16,
          "maxToolCalls": 100,
          "maxInputTokens": 300000,
          "maxGrossInputTokens": 300000,
          "maxUncachedInputTokens": 180000,
          "maxPromptTokensPerTurn": 64000,
          "maxOutputTokens": 50000
        },
        "phaseTargetLimits": {
          "maxTurns": 16,
          "maxToolCalls": 100,
          "maxInputTokens": 300000,
          "maxGrossInputTokens": 300000,
          "maxUncachedInputTokens": 180000,
          "maxPromptTokensPerTurn": 64000,
          "maxOutputTokens": 50000
        },
        "operationLimits": {
          "maxGrossInputTokens": 600000,
          "maxUncachedInputTokens": 400000,
          "maxOutputTokens": 100000,
          "maxTurns": 40,
          "maxToolCalls": 200
        },
        "profileHash": "..."
      }
    }
  ]
}
```

Harness 必须执行以下算法，禁止只消费 Runtime 输出的 `budgetConformance=true`：

1. 从已鉴权的 `GET /runs/{run_id}/budget-profile` 读取 Run 创建时冻结的 Profile；Legacy 历史 Run 缺失时返回
   明确冲突，不能用当前进程环境变量补造历史 Profile；
2. 对除 `profileHash` 外的全部 Profile 字段做递归 Key 排序与无空白 JSON 序列化，按 Runtime 相同规则计算
   SHA-256；Hash、Phase、Profile ID 和 Mode 必须与 Run/Context/Restart Evidence 完全一致；
3. `events.ndjson` 的每个 `model.usage` 必须携带 `runId + turn`。Replay 按 `(runId, turn)` 去重，逐项重算
   Gross、Cached、Uncached、Output、Turns 和单轮最大 Input；`cached > gross` 立即失败关闭；
4. 每个 Run 独立校验 `maxPromptTokensPerTurn`、Turns、Tool Calls、Gross、Uncached 和 Output。采用哪一组执行
   上限由冻结的 `tokenBudgetMode/rolloutMode` 决定，不能由 Replay 时的 Pod Env 决定；
5. Repair 等多 Run 场景不得只校验最终 Run。Setup、Review、Repair 的 Profile 与 Usage 全部进入 Bundle，并按
   `operationId` 重算 Operation 用量；同一 Operation 的 `operationAttempt` 必须从1开始连续且不重复。Build/
   Restart 若只冻结 Successor 而缺少 Predecessor，或超过 `operationLimits`，均失败关闭；
6. Release Gate 还必须读取仓库内批准的 Phase/Profile Policy，比较允许的 Mode、Profile Hash 和上限范围。
   Bundle 自带的超大上限不能自证合理；`shadow` 只可生成 Readiness Result，不具备 Release Eligibility；
7. `budget-profiles.json` 进入 `checksums.sha256`，其原始字节 Hash 同时写入 Manifest。缺文件、重复 Run、未知
   Run、字段为0、Hash 不一致、用量超限或 Policy 不匹配均失败关闭。

这样可以分别回答三个问题：Runtime 当时实际执行了什么上限、Provider 实际报告了多少用量、该上限是否是
发布策略批准的上限。三者不能由一个布尔字段替代。

批准策略固定为仓库文件 `infra/generation-reliability/release-budget-policy.json`，Schema 为
`runtime-release-budget-policy@1`。Release 命令必须显式传入该文件，Aggregator 将其原始字节 SHA-256 写入
Index，最终 Validator 再与实际输入文件比较；不得从 Bundle、当前 Pod Env 或未版本化的 CI Secret 推导。

```json
{
  "schemaVersion": "runtime-release-budget-policy@1",
  "releaseStage": "production",
  "requiredModes": {
    "rolloutMode": "enforced",
    "tokenBudgetMode": "split_enforced",
    "operationBudgetMode": "enforced"
  },
  "phaseProfiles": {
    "build": {
      "allowedProfileHashes": ["..."],
      "maximumLimits": {
        "maxTurns": 16,
        "maxToolCalls": 60,
        "maxInputTokens": 300000,
        "maxGrossInputTokens": 300000,
        "maxUncachedInputTokens": 180000,
        "maxPromptTokensPerTurn": 64000,
        "maxOutputTokens": 40000
      },
      "maximumOperationLimits": {
        "maxGrossInputTokens": 450000,
        "maxUncachedInputTokens": 270000,
        "maxOutputTokens": 80000,
        "maxTurns": 30,
        "maxToolCalls": 100
      }
    }
  }
}
```

Readiness/Canary 可以使用独立 Policy 允许 `shadow`，但输出只能是 `readinessPassed`，不能产生
`releaseEligible=true`。Production Policy 的 Profile Hash 或上限变更必须走代码评审；Policy 只能收紧或显式
版本升级，不能在一次失败 Smoke 后由 Harness 自动放宽。

Aggregator 将完整 Policy 与每个 Bundle 的完整 Budget Profiles 写入 Release Evidence。最终 Validator 不只检查
`budgetConformanceStatus/releaseBudgetPolicyStatus`：它会重新计算 Profile Canonical Hash、重新执行 Mode/Hash/
Phase/Operation 上限策略，并确认主 Run 属于该 Profile 集。CLI 发布校验还必须通过 `--budget-policy` 读取原始
Policy 文件并比较字节 SHA-256，避免手写 Index 或布尔值绕过门禁。

当前 Assembler 接受 `generation-real-provider-suite-evidence@2` 中最终 accepted Build，也接受存放在同一
accepted Suite 下的已发布 `generation-real-provider-edit-evidence@2`。已发布 Edit 必须同时证明
Provider/Context/Build/Route/Release 身份、HTTP Artifact 和 Sandbox 终态清理。Edit Runner 的清理已从
best-effort 改为有界幂等确认：要求两次成功 Release 响应，耗尽尝试会将原本 accepted 的证据改为
failed。Draft/HMR Edit 的 Sandbox 由外层生命周期持有，不能冒充已发布终态。Repair Adapter 会从同一
Suite 重新解析 Setup Edit 文件，合并 Setup Edit、Review、Repair 三条脱敏 Event Stream 并重算整个场景的
Usage；最终 Route/Source/Context 身份只取自 completed Repair Run，且必须有 HTTP 200、新 Version、Source
Mutation、Preview Publish、Marker 保留和父 Case Sandbox Release 证据。Runtime Restart Probe 已将原先单次 best-effort
Release 改为有界两次幂等确认，新证据为 `generation-context-runtime-restart-evidence@2`；`@1` 仍可用于历史
诊断，但因没有冻结 Cleanup 而不具备发布资格。`runtime_restart_terminal` Adapter 仅接受 Candidate `@2`，
将重启证据重新绑定到同一 Suite 的 Build Event Stream、Provider Cache、Run/Context/Budget、Source Snapshot、
Artifact Body Hash、Deployment Revision 和替换前后 Pod UID；任一身份漂移、Pod UID 未变或 Cleanup 不完整均失败关闭。

Release Aggregator 在 release mode 强制读取 `runtime-terminal-bundle-set@1`，逐个执行离线 Replay 后才生成
`runtime-terminal-bundle-index@1`；最终 Validator 会再次重算 Website/Docs/Edit/Repair/Runtime Restart 覆盖和
Commit/Model/Revision/Provider Config/Budget 身份。摘要布尔值或手工编写的 Index 不能代替 Bundle Replay。
`scenarioKind` 同时冻结在 Manifest 和 Case Summary 中；Website/Docs 还必须分别绑定 `/` 与 `/docs/`，
并且使用不同的 Project/Run 身份。因此复制 Website Bundle、改目录名或单独修改场景标签不能冒充
Docs 覆盖。
每个 Bundle 还必须冻结原始 `ProviderCacheSmokeAudit@1` 文件 SHA-256；Release Aggregator 会将它与
`RUNTIME_RC_PROVIDER_CACHE_EVIDENCE` 实际输入的原始字节 Hash 比较。因此即使 Commit/Model/Revision/Config
字段相同，不同 Cache 样本或重新编写的 Audit 也不能与终态 Bundle 混用。

## 3. 根因分析

### 3.1 Run 用量是多轮完整请求的累计，不是唯一上下文大小

Runtime 每轮重新构造：

```text
System Prompt
+ Runtime Workflow Progress
+ GenerationContext
+ 用户消息
+ 活动 Conversation / Tool History
+ Eager Tool Definitions
+ Deferred Tool Definitions
```

`ModelUsage.inputTokens` 来自 Provider 响应，并按唯一 `(runId, turn)` 取最新值后累加。该口径适合作为
Provider Known Usage，但不能回答“当前活动上下文有多大”或“有多少内容是重复发送”。

成功 Build 第一轮已有 7,091 Input。即使历史完全不增长，12 个模型轮次也会重复产生约 85,000 Input。
因此降低轮次与稳定前缀比继续裁剪约 900 Token 的 GenerationContext 更重要。

### 3.2 动态进度位于 System Prompt，破坏缓存稳定前缀

`agent_loop.rs` 当前先构造稳定的 System Prompt，再把每轮变化的 `Runtime Workflow Progress` 直接追加
进去。OpenAI-compatible 请求随后把整个字符串作为第一条 System Message。

当 `stage`、`completedSteps`、`nextAction` 或 Observation Budget 改变时，Prompt 前部随之改变。本次真实
Build 中：

- 状态稳定的轮次可命中约 16k～19k Cached Input；
- 状态变化的轮次经常只命中 256 Token；
- 最终成功 Build 的整体缓存率只有 14.3%；
- 最终成功 Edit 的缓存率为 46.4%。

这说明 Provider 缓存本身工作正常，但 Prompt 排列没有提供稳定前缀。

### 3.3 大 Tool Call 参数长期驻留

成功 Build 第 3 轮产生 5,411 Output Token，主要是完整页面源码的 `fs.write` Tool Call。下一轮 Input 从
10,325 增加到 15,497。最终 Checkpoint 中对应 Assistant 消息的 JSON 长度约 20,000 字符，并被后续轮次
持续重传。

当前协议正确保留了 Tool Call / Tool Result 配对，但“审计事实必须完整保存”不等于“每次模型请求必须
携带完整历史参数”。Runtime 缺少从 Durable Transcript 到 Active Model Window 的确定性投影层。

### 3.4 压缩只看消息数量，不看 Token 和消息体积

当前阈值为：

```text
COMPACT_MESSAGE_THRESHOLD = 32
COMPACT_KEEP_RECENT = 16
```

成功 Build 只有 12 个模型轮次，因此即使存在 20,000 字符 Tool Call 也没有及时压缩。最终 Edit 在第 15
轮触发压缩后，单轮 Input 从 18,668 降到 13,875，一次减少 4,793 Token，证明压缩方向有效但触发过晚。

此外，压缩后最多恢复 16,000 Token 源码。如果没有按当前 Workflow Target 筛选，恢复动作可能重新引入
刚被移除的大上下文。

### 3.5 确定性生命周期仍由模型逐轮驱动

`project.ensure_dependencies`、`project.build`、`preview.start`、`preview.dev_status`、
`draft.snapshot_create` 等动作的顺序已经由 Runtime Workflow State 决定，但当前仍依赖模型读取
`nextAction` 后再发出 Tool Call。

成功 Build 的后半段大量轮次只是在推进确定性状态机。每多一轮，即使模型只输出几十 Token，也会重新
提交约 15k～19k Input。

### 3.6 一个预算承担了三种不同职责

`RUNTIME_AGENT_MAX_INPUT_TOKENS=200000` 当前同时被当作：

1. Run 成本保护；
2. Agent 失控熔断；
3. Context 容量保护。

但它实际累计 Provider `prompt_tokens`，其中包含 Cached Input，也没有限制单轮 Prompt。结果是：

- 缓存命中较高的 Run 仍会被当作全额 Input 终止；
- 单轮 Context 是否过大没有独立门禁；
- 提高上限能让 Run 继续，却无法减少成本和延迟；
- 不同 Provider 是否支持 Cached Usage 会改变相同任务的终止行为。

### 3.7 Partial 后重跑没有复用已验证工作

本次两个 Build Partial 后均创建新 Run，重新加载同一 GenerationContext、重新读取基础源码并重新生成
页面。失败 Attempt 共消耗 421,041 Gross Input 和 14,861 Output。

Run Binding 不能跨 Run 复用，但已形成且通过 Workspace Policy 的 Source Snapshot、Mutation Receipt、
Build Diagnostic 和 Workflow Progress 可以作为新 Run 的冻结前置事实。当前缺少这种受控 Continuation。

### 3.8 UI 只展示总量，用户无法理解缓存与重试

当前 `RunModelUsage@1` 返回 Gross Input、Output、Cached Input 和 Turn Count，但 UI 主要展示
`totalTokens / inputTokens / outputTokens`。用户看不到：

- 非缓存 Input；
- 单轮 Prompt 峰值；
- Cache Hit Rate；
- 压缩前后大小；
- 当前 Run 是否属于同一用户操作的重试；
- 整个作品生成或修改操作的 Attempt 汇总。

### 3.9 Docs Artifact Entry Route 没有成为单一事实源

当前 `/docs/` 同时散落在：

- Brief 默认 Required Route；
- Fumadocs `EditableSurfaceMetadata.primary_routes`；
- Browser Validation Rules；
- Development Server Readiness Path。

但 `GenerationContract@1` 没有 `entryRoute` 或 `routePolicy`，Validator 根据 `ArtifactType::Docs` 硬编码
`/docs/`；Fumadocs `next.config.mjs` 却没有 `trailingSlash: true`，静态导出生成 `out/docs.html`。本地 Preview
使用 Python Server，远端 Preview 使用自定义 Node Resolver。四方没有共享同一个 Route Resolution Contract。

因此 Validator 报告的 Metadata/Search 只是二次症状。真正的首个失败应当是
`preview.entry_route_resolution_failed`，且应在 Browser Checks 前由 Runtime 检出。

### 3.10 Validation Failure 没有区分责任层

当前 `generation.validation_failed` 对所有 Blocker 返回同一个建议：读取报告、修改 Source、重新 Publish。
但失败至少分为四层：

| 层级 | 典型问题 | 责任方 | 是否需要模型 |
|---|---|---|---|
| Source | 缺少标题、Search 控件、断链 | 生成源码 | 是，执行有界 Source Repair |
| Build | 编译、依赖、静态导出失败 | 源码或依赖策略 | 通常需要模型或确定性依赖恢复 |
| Artifact | Manifest、Entry File、Hash 不一致 | Runtime Build/Stage | 否，平台失败关闭 |
| Serving | Route Resolver、Base Path、Preview 进程错误 | Runtime Preview | 否，确定性重试一次后失败关闭 |

Serving/Artifact 错误如果进入 Source Repair，会要求模型为平台 Bug 修改用户作品，不仅浪费 Token，还可能
破坏已经正确的源码。

### 3.11 Observation 被误当成 Substantive Progress

`RunProgressState.fingerprint()` 当前包含 `observations`。每读取一个新文件或执行一个新搜索，Fingerprint 都会
变化，即使 Candidate、Source、Build 和 Workflow Stage 没有前进。因此模型可以通过不断读取不同文件避开
`max_no_progress_turns`。

需要区分：

```text
Observation Progress
  = 新的有界诊断事实，只消耗 Observation Budget

Substantive Progress
  = 首次出现的新 Source Digest、新 Candidate Manifest、单调 Workflow Milestone 或 Durable Version
```

No-progress Fuse 只能基于 Substantive Progress。Observation 继续用于防重复读取和审计，但不能重置主熔断。

## 4. 修复目标与非目标

### 4.1 P0 正确性目标

1. Provider Known Usage 保持原始、可追溯且按唯一 Turn 幂等累计；
2. Cached Input 不重复加入 Total，但必须独立展示；
3. Run 预算、单轮 Prompt 预算和 Provider/Project 每日配额使用明确且不同的口径；
4. Provider 未返回 Usage 时继续使用保守估算并标记 `estimated=true`；
5. Provider 返回 `cachedInputTokens > inputTokens` 时不允许无声下溢，记录 Usage Contract Violation；
6. Prompt 压缩不得破坏 Tool Call / Tool Result 配对、待审批 Permission 或恢复幂等；
7. Restart 后必须从持久化事件恢复相同预算状态和相同 Active Window Projection；
8. 所有优化不得放宽 GenerationContext、DCP、Style Contract、Mutation Lease、Sandbox 和 Publication 边界。
9. Fumadocs `/docs/` 在 Local、Remote、Staged Artifact 和 Published Work 中必须解析到相同 HTML；
10. Browser Validation 前必须验证 Entry Route 可解析为 HTML，而不是目录、404 或数据文件；
11. Artifact/Serving Failure 不得进入 Source Repair，也不得要求新的 Source Mutation；
12. Source Repair 使用有界 Repair Context，不把完整 Candidate Manifest 和 Browser Evidence 注入模型窗口；
13. Observation 不得重置 Substantive No-progress Fuse；相同 Candidate 的相同失败不得无限重试。

### 4.2 P1 效率目标

以下是针对 `deepseek-v4-pro@4`、`next-app` 和当前 Runtime 工具集的 Benchmark 目标，不直接由单次 Smoke
判定发布。其他 Provider 在支持标准 Cached Usage 前只执行 Gross/Uncached 观测，不使用 Cache Hit Rate
作为门禁。

| 指标 | Greenfield Build | 简单 Style/Token Edit | 说明 |
|---|---:|---:|---|
| P50 模型轮次 | ≤ 8 | ≤ 6 | 失败诊断另计 |
| P95 模型轮次 | ≤ 12 | ≤ 9 | 不以扩大 Turn 上限达成 |
| P50 Gross Input | ≤ 140k | ≤ 100k | 单次成功 Attempt |
| P95 Gross Input | ≤ 180k | ≤ 140k | 单次成功 Attempt |
| P50 非缓存 Input | ≤ 90k | ≤ 65k | Provider 报告缓存时 |
| 单轮 Prompt 峰值 | ≤ 16k | ≤ 14k | 不含图片媒体字节 |
| Cache Hit Rate | ≥ 60% | ≥ 60% | 仅支持缓存的 Provider |
| 首次源码 Mutation | ≤ Turn 2 | ≤ Turn 3 | 与原 Generation Context 目标一致 |
| GenerationContext | ≤ 64 KiB | ≤ 64 KiB | required overflow 继续失败关闭 |
| 重复完整 Context Read | 0 | 0 | 相同 Epoch/Hash |
| 越界 Mutation | 0 | 0 | 硬门禁 |
| Required Fidelity | 100% | 100% | 硬门禁 |

Operation 级门禁：同一用户操作因 Runtime 内部 Partial/Retry 产生的 Gross Input 不得超过最终成功 Attempt
的 1.5 倍；超过时必须显示 `retry_amplification` 并阻止自动无限续跑。

Harness 将效率判定分成两层：

1. 确定性硬门禁：不得重传完整大 Tool Pair、不得让平台错误产生额外模型轮次、Prompt 估算不得超过配置、
   Restart 后 Projection/Usage/Progress 必须一致；
2. 统计 Benchmark：每个 Profile 至少 30 个 Accepted Attempt、覆盖至少 10 个固定 Prompt，并报告样本分布、
   Provider/Model Revision、P50/P95 和 Bootstrap Confidence Interval。样本不足时只显示 `insufficient_sample`，
   不得用 1～6 个不同场景声称 P95 通过。

真实 Provider Smoke 只验证功能、协议和明显退化，不断言精确 Token。上述 P50/P95 在满足样本要求前保持
Shadow；功能正确性和安全门禁从第一批实现开始就是硬要求。

### 4.3 非目标

- 不删除完整 DCP Artifact 或 Design Profile；
- 不削弱 required Acceptance、Content Plan Approval 或 Runtime Attestation；
- 不建立价格、货币或 Provider 账单系统；
- 不把 Cached Token 当作零成本；
- 不通过降低生成质量、删除页面内容或跳过 Build/Preview/Snapshot 达成指标；
- 不重新开启无边界的量化样本扩张；只执行本方案定义的定向回归和 Release 终态 Smoke；
- 不在日志、Metric、Checkpoint 或 Evidence 中保存 Provider API Key、Authorization Header 或完整 Prompt。

## 5. 目标架构

### 5.1 Token Usage 与 Budget 分层

新增内部结构：

```rust
struct RunTokenUsageV2 {
    gross_input_tokens: u64,
    cached_input_tokens: u64,
    uncached_input_tokens: u64,
    output_tokens: u64,
    max_turn_input_tokens: u64,
    turn_count: u32,
    estimated_turn_count: u32,
}
```

不从 Provider 价格推断成本。预算只使用可验证 Token 事实：

```rust
struct AgentTokenBudgets {
    max_gross_input_tokens_per_run: u64,
    max_uncached_input_tokens_per_run: u64,
    max_prompt_tokens_per_turn: u64,
    max_output_tokens_per_run: u64,
}
```

计算规则：

```text
gross += inputTokens
cached += min(cachedInputTokens, inputTokens)
uncached += inputTokens - min(cachedInputTokens, inputTokens)
maxTurnInput = max(maxTurnInput, inputTokens)
```

当 Provider 没有 Cached Usage 时，`cached=0`，因此所有 Input 保守计入 Uncached。不能根据模型名称猜测
缓存命中。

### 5.2 预算配置与兼容

新增配置：

```text
RUNTIME_AGENT_MAX_GROSS_INPUT_TOKENS
RUNTIME_AGENT_MAX_UNCACHED_INPUT_TOKENS
RUNTIME_AGENT_MAX_PROMPT_TOKENS_PER_TURN
RUNTIME_AGENT_TOKEN_BUDGET_MODE=legacy|split_shadow|split_enforced
RUNTIME_AGENT_PHASE_BUDGET_MODE=off|shadow|enforced
RUNTIME_AGENT_PHASE_{BRIEF|BUILD|EDIT|REPAIR}_MAX_GROSS_INPUT_TOKENS
RUNTIME_AGENT_PHASE_{BRIEF|BUILD|EDIT|REPAIR}_MAX_UNCACHED_INPUT_TOKENS
RUNTIME_AGENT_PHASE_{BRIEF|BUILD|EDIT|REPAIR}_MAX_PROMPT_TOKENS_PER_TURN
RUNTIME_AGENT_PHASE_{BRIEF|BUILD|EDIT|REPAIR}_MAX_TURNS

RUNTIME_AGENT_OPERATION_BUDGET_MODE=shadow|enforced
RUNTIME_AGENT_CONTINUATION_MODE=off|shadow|enforced
RUNTIME_AGENT_CONTINUATION_ALLOWLIST_JSON=[{"phase":"build","agentProfile":"build"}]
RUNTIME_AGENT_MAX_OPERATION_GROSS_INPUT_TOKENS
RUNTIME_AGENT_MAX_OPERATION_UNCACHED_INPUT_TOKENS
RUNTIME_AGENT_MAX_OPERATION_OUTPUT_TOKENS
RUNTIME_AGENT_MAX_OPERATION_TURNS
RUNTIME_AGENT_MAX_OPERATION_TOOL_CALLS
```

这些变量和现有 `RUNTIME_AGENT_MAX_TURNS` 都在 Runtime 进程启动时读取。当前版本修改 Deployment Env 后需要
Runtime Pod 滚动升级；它们不是 Provider 配置，修改 Provider/Model Service 本身不应触发 Runtime Pod
滚动。后续 Phase Budget Profile 若支持由控制面冻结到 Run，仍必须在 Run 启动时生成不可变
`executionProfile`，禁止运行中热改预算语义。

兼容规则：

1. 未设置新变量时，现有 `RUNTIME_AGENT_MAX_INPUT_TOKENS` 继续作为 Legacy Gross 上限；
2. `split_shadow` 同时计算新旧决策，但只执行旧决策；
3. `split_enforced` 使用 Gross、Uncached、Per-turn 三个门禁；
4. 旧事件和 Checkpoint 没有 Cached Usage 时按 `cached=0` 恢复；
5. 不重写历史 `model.usage`，新聚合器从现有事件确定性计算 V2；
6. `RUNTIME_AGENT_MAX_INPUT_TOKENS` 在两次稳定发布后才能废弃，且废弃前必须有启动告警。
7. Operation Budget 是同一 `operationId` 下所有 Attempt 的累计上限，不替代每个 Run 的单轮 Prompt 安全门禁；
8. Operation Budget 默认为 `shadow`；切换为 `enforced` 前必须先验证历史 Attempt 的归属和预算分布；
9. Root Run 生成新的 `operationId`，受控 Successor 继承该 ID；事件按 `(runId, turn)` 去重后再聚合，避免 Restart
   重放重复计费。
10. 新 Run 必须冻结 `RunBudgetProfile@1`；历史 Run 若没有 Profile，不伪造回写，只记录
    `run.budget_profile_missing` 并使用 Session 启动时的兼容上限。该兼容路径不能进入 Enforced cohort。
11. Runtime 提供只读 `GET /runs/{run_id}/budget-profile`。接口先按 Run 所属 Project 执行 Read
    Authorization，再返回已冻结 Profile；不存在的 Run 返回404，历史 Run 缺 Profile 返回409，Profile 自身
    Hash/Phase 校验失败返回服务端错误。接口不接受预算修改，也不从当前 Pod Env 动态重建 Profile。
12. 修改上述 Runtime Env 或批准的 Profile Policy 只影响新 Run。Env 方式需要 Runtime Pod 滚动升级；未来若
    由控制面在 Run 创建时注入已签名 Profile，可不重启 Pod，但仍禁止修改已经创建的 Run。

首期部署建议值只用于 Shadow，不直接作为所有 Provider 的永久默认：

| Phase | Gross / Run | Uncached / Run | Prompt / Turn | Turns |
|---|---:|---:|---:|---:|
| Brief | 80k | 40k | 24k | 6 |
| Build | 300k | 180k | 64k | 16 |
| Edit | 220k | 120k | 48k | 12 |
| Repair | 180k | 100k | 48k | 10 |

在 Prompt 稳定、Microcompact 和 Workflow Driver 上线前，Edit 的新预算只能 Shadow，否则现有 268k
路径会被提前终止。最终 Enforced 值以定向回归结果收紧，不能为迁就旧低效行为而扩大。

### 5.3 稳定 Prompt 前缀

目标请求布局：

```text
Stable System Prompt
Stable GenerationContext / Run Binding Message
Stable initial user intent
Projected durable conversation history
Latest Runtime Workflow Progress（动态、只保留一份、位于尾部）
```

具体要求：

1. `generation_context_system_prompt` 不包含会逐轮变化的字段；
2. `Runtime Workflow Progress` 改为独立 `runtime_workflow_progress` System Message；
3. 活动窗口中同一时刻只允许一条最新 Progress Message，更新时替换而不是追加；
4. Progress Message 必须位于历史窗口尾部，不能改变稳定前缀；
5. Repair Target、Permission Resume 等动态数据也进入尾部 Ephemeral Context；
6. Tool Definition 按稳定名称排序，并计算 `toolSetHash`；
7. Tool Definition 只在 Runtime Workflow Stage 真正改变可用能力时变化；
8. 不为提高缓存伪造或固定错误的 Workflow State。

增加不含 Prompt 内容的观测事件：

```json
{
  "type": "prompt.composition",
  "turn": 4,
  "staticPrefixHash": "...",
  "toolSetHash": "...",
  "workflowProgressHash": "...",
  "estimatedTokens": {
    "system": 420,
    "generationContext": 966,
    "toolDefinitions": 5100,
    "conversation": 7300,
    "workflowProgress": 180,
    "total": 13966
  }
}
```

只记录分类、Token 估算、Hash 和字节数，不记录原文。

### 5.4 Durable Transcript 与 Active Model Window 分离

引入 `TranscriptProjection@1`：

```text
Durable Transcript
  - 保存完整原始 User / Assistant / Tool Call / Tool Result
  - 用于审计、恢复、协议证明

Active Model Window
  - 从 Durable Transcript 确定性投影
  - 只保留当前任务需要的完整消息
  - 已完成的大 Tool Exchange 可替换为有界摘要
```

任何投影都不得修改 Durable Transcript。Checkpoint 保存：

- `projectionVersion`；
- `sourceTranscriptRange`；
- `contextWindowEpoch`；
- `projectedMessageWindow`；
- `projectionHash`；
- `protectedExchangeIds`；
- `lastConsumedConversationItemId`。

Runtime Restart 必须根据 Durable Transcript 重建出相同 `projectionHash`。不一致时失败关闭为
`transcript_projection_mismatch`，不能回退到未经验证的全历史窗口。

### 5.5 Tool Exchange Microcompact

新增按语义分类的 Microcompact：

| Tool 结果 | 压缩策略 | 必须保留 |
|---|---|---|
| 成功 `fs.write/fs.patch/fs.multi_patch` | 下一次模型请求前压缩大参数 | tool、path、input hash、result hash、bytes、workspace revision、mutation receipt |
| 成功 `project.ensure_dependencies` | 保留有界摘要 | lockfile hash、mode、exit status、duration |
| 成功 `project.build` | 保留摘要 | build id、source digest、artifact/report identity、duration |
| 成功 Preview/Snapshot | 保留摘要 | session epoch、workspace/durable revision、snapshot id、status |
| 失败 Tool | 暂不压缩 | error kind、diagnostic path、recoverable、required next action |
| Pending Permission | 禁止压缩 | 完整 Tool Call / Permission identity |
| 当前 Repair Target Source | 受 Token 上限保护 | 最新完整观察或明确 excerpt + hash |
| `run.complete` | 保留摘要 | terminal status、summary、output identity |

Tool Call / Tool Result 必须成对从 Active Window 移除，并用单条
`runtime_tool_exchange_summary` System Message 替代。禁止仅修改 Tool Call 参数但保留原 Tool Result，避免
向 Provider 构造虚假历史。

默认触发条件：

```text
单个已完成 Tool Exchange 估算 > 2,000 Token
或下一轮请求估算 > 14,000 Token
或活动 Conversation 估算 > 8,000 Token
或消息数量 > 32
```

压缩目标不是固定删除到 16 条，而是将下一轮 Prompt 降到 Phase Target 以下，同时保护：

- 最新用户意图；
- GenerationContext 与 Run Binding；
- 最新 Workflow Progress；
- 未完成 Tool Pair；
- 最新失败诊断；
- 当前 Mutation Target 的必要源码；
- 当前 Preview/Snapshot identity。

### 5.6 Token 驱动的 Full Compaction

`compact_if_needed` 改为同时接受：

```text
messageCount
estimatedConversationTokens
serializedConversationBytes
estimatedNextRequestTokens
largestMessageTokens
largestMessageBytes
workflowProtectedSet
```

Compaction 触发后：

1. 先执行 Tool Exchange Microcompact；
2. 如果仍超过 Target，再把旧 Conversation 写入 `state/context.md` 的版本化区块；
3. 仅恢复当前 Workflow Target 必需的 Source Observation；
4. Source Restore 使用 Phase-aware 预算：Build ≤8k，Edit/Review/Repair ≤4k，其他 Phase 为0；
5. 对大于单文件预算的源码只恢复经过 Hash 绑定的相关范围，或要求一次定向 `fs.read`；
6. 每次记录 `beforeTokens`、`afterTokens`、`removedTokens`、`restoredTokens` 与受保护原因；
7. 相同 Epoch 不得重复注入同一 Source Hash。

### 5.7 Runtime Workflow Driver

新增 `RuntimeWorkflowDriver`，只自动执行 Runtime 已能确定的生命周期动作。模型继续负责设计、内容、源码
修改和失败修复，Runtime 负责机械状态推进。

```text
模型提交 Source Mutation
  ↓
Runtime 验证 Mutation Receipt / Style Contract / Scope
  ↓
Runtime 自动执行当前 Profile 的确定性后续动作
  ├─ success → Ready / Durable Snapshot
  └─ failure → 写结构化 Diagnostic，回到模型修复
```

首期自动化：

| Profile | Runtime-owned success path |
|---|---|
| Greenfield Static Next | ensure dependencies → build → start fallback preview（需要时）→ snapshot |
| Cold Dev | stop prior Dev → restore dependencies → start Dev → wait current Epoch/Revision → snapshot |
| Warm HMR | wait current Epoch/Revision iframe ACK → snapshot |
| Style/Token Edit | validate token CAS → apply tokens → wait HMR → snapshot |
| Fumadocs | preview.publish 内部已有的 restore/build/validate/candidate 流程保持单入口 |

约束：

1. Runtime-owned 动作使用独立 Lifecycle Event，不伪装成模型 Tool Call；
2. 每个动作仍经过现有 Workspace、Sandbox、Style Contract 和 Acceptance 门禁；
3. 动作必须可取消、可超时、可从 Checkpoint 幂等恢复；
4. 首次失败立即停止自动链路，不能无模型参与地循环修复；
5. `run.complete` 可以由 Runtime 在所有硬门禁满足后执行原子完成，用户摘要使用确定性模板；
6. 需要自然语言总结时最多增加一个最终模型轮次，不为每个机械动作增加轮次。

### 5.8 Partial Continuation

现有“新 Run 必须有独立 Run Binding”的规则保持不变。新增受控 `RunContinuation@1`：

```text
Predecessor Partial Run
  ├─ frozen source snapshot
  ├─ mutation receipts
  ├─ build/validation diagnostics
  ├─ workflow progress
  └─ token usage
          ↓
Successor Run（新 runId、新 Binding）
  ├─ predecessorRunId
  ├─ frozen continuation snapshot id
  ├─ compact continuation summary
  └─ remaining operation budget
```

只允许以下 Partial 原因续跑：

- Token/Turn/Tool Budget Exhausted，但存在通过 Workspace Policy 的源码快照；
- Provider 可恢复中断；
- Runtime/Sandbox 基础设施中断；
- 构建失败且 Diagnostic 与 Source Revision 均已冻结。

以下情况禁止复用：

- GenerationContext、Content Plan、Design Profile、EditImpactPlan 或 Base Revision 已变化；
- 存在越界 Mutation；
- Style Contract 或 Acceptance 身份不匹配；
- Candidate 已被安全门禁拒绝且没有新的 Source Mutation；
- Source Snapshot Hash 无法验证；
- 用户明确要求重新生成。

Operation Budget 跨 Attempt 累计，最多自动续跑一次。第二次仍 Partial 时必须停止并向用户显示具体原因，
不得无限消费。

Continuation 不能把“同一个 PVC 仍然存在”当作 Source Snapshot。当前 `RunContinuationSnapshot@1` Contract
至少冻结：

```text
snapshotId / predecessorRunId / operationId / attempt
sourceSnapshotUri / sourceHash / workspaceRevision
generationContextContentHash / contentPlanHash / designProfileEffectiveHash
budgetProfileHash
editImpactPlanHash / baseVersionId / workflowProgressHash / workflowProgress
progressFingerprint / progressLedger / checkpointId
accumulatedUsage / remainingOperationBudget / createdAt
```

创建与调度必须由单一 Runtime Continuation Coordinator 负责，不能由 AgentLoop 自行递归启动新 Run：

1. AgentLoop 把 Run 原子地结束为 Partial，写入 Partial Reason 与最终 Checkpoint；若失败 Build 已产生经过验证的
   Runtime-owned Source Snapshot，可调用 Store 冻结 Continuation Snapshot，但不得自行递归启动 Run；
2. Coordinator 读取终态 Run 和冻结 Snapshot，重新生成 `ContinuationEligibilityDecision@1`；
3. 只有 Source Snapshot、所有冻结身份和剩余 Operation Budget 同时验证通过，才创建 Successor；
4. Successor 必须使用新的 `runId`、Session、GenerationContext Binding 和 Sandbox Lease；
5. `(operationId, predecessorRunId, snapshotId)` 是幂等键；Restart 重放不得创建第二个 Successor；
6. 创建成功后写 `run.continuation_created`，再由标准 RunLifecycle 启动 Session；
7. Snapshot 创建、Successor 创建和 Session Launch 任一步失败都保持可审计终态，禁止退化为“从头重跑”。

分阶段模式固定为：

| 模式 | 行为 |
|---|---|
| `off` | 不计算、不创建 Continuation |
| `shadow` | 计算 Eligibility、Operation Budget 和预期复用量，但不创建 Successor |
| `enforced` | 仅对 `RUNTIME_AGENT_CONTINUATION_ALLOWLIST_JSON` 精确匹配的 Phase/Profile 创建一次 Successor；配置缺失、畸形或任一身份不一致即失败关闭 |

当前部署清单固定为 `shadow`，代码中的 `enforced` 仅用于定向回归。虽然同进程并发 reconcile 幂等测试已通过，
在真实跨进程 Restart Replay、真实 Sandbox 恢复证据和 Enforced cohort allowlist 完成前，不得在生产启用
`enforced`；也不得把 Checkpoint ID、Draft Preview Session、Workspace Revision 或 PVC 路径单独当作可续跑证据。

### 5.9 Tool Definition 分阶段加载

现有 Eager / Deferred Tool Loading 保留，并增加 Stage-aware Tool Set：

1. `context_ready` 不暴露 Browser 和发布类诊断工具；
2. `source_authoring` 只暴露当前 Phase 允许的 Source/Style Mutation；
3. `validation_failed` 才加载对应 Diagnostic Read；
4. `draft_ready` 只暴露 `run.complete` 和必要状态查询；
5. 不在同一 Stage 内根据模型行为频繁增删工具，避免 Tool Set Hash 抖动；
6. 历史已使用但当前不可用的工具由 Transcript Projection 摘要处理，不向 Provider 留下不配对历史。

首轮约 7k Input 中，GenerationContext 约 900 Token，其余主要来自 System、用户消息和 Tool Definition。
新增 Prompt Composition Metric 后再决定哪些 Tool 需要 Deferred，不能凭 Schema 字符数直接删除功能。

### 5.10 Artifact Entry Route Contract

将入口路由从 `ArtifactType` 的隐式约定提升为 Template/Generation Contract 的显式语义，但不把文件
fallback 顺序暴露为运行时 Oracle：

```rust
struct ArtifactRouteContract {
    entry_route: String,           // Website: "/"; Docs: "/docs/"
    canonical_policy: RoutePolicy, // Root | TrailingSlash | CleanHtml
}
```

契约版本升级为 `generation-contract@2`。读取 `@1` 时只做确定性兼容映射：Website 映射 `/`，Docs 映射
`/docs/ + trailing_slash`；不根据 Artifact 文件内容猜测业务入口。新 Candidate 一律写 `@2`，历史
Validation Report 和 Artifact 保持原版本，不做破坏性回写。

Fumadocs 的目标契约固定为：

```json
{
  "entryRoute": "/docs/",
  "canonicalPolicy": "trailing_slash"
}
```

Build Adapter 根据冻结的 Template Contract 和本次实际输出生成唯一的 `ArtifactRouteManifest@1`。Preview、
Validator 和 Published Artifact Presenter 只能消费该 Manifest，不能各自从文件系统重新猜测 clean URL：

```json
{
  "schemaVersion": "artifact-route-manifest@1",
  "buildId": "build-...",
  "entryRoute": "/docs/",
  "canonicalPolicy": "trailing_slash",
  "routes": {
    "/docs/": {
      "file": "docs/index.html",
      "sha256": "...",
      "contentType": "text/html; charset=utf-8"
    }
  },
  "aliases": {
    "/docs": "/docs/"
  }
}
```

Route Manifest 不包含自身 Hash，也不反向引用 Candidate Manifest，避免形成不可计算的 Hash 环。Build Adapter
对 Route Manifest 文件字节计算 `artifactRouteManifestHash`，由 Candidate Manifest 和 Build Result 单向绑定；
Validator、Preview 与 Published Presenter 必须同时验证该绑定以及 Route Target 的文件 Hash。

旧 p3～p6 Artifact 只有 `docs.html` 时，兼容 Build Adapter 可以把 `/docs/` 和 `/docs` 映射到同一个
`docs.html` Hash。若 `docs.html` 与 `docs/index.html` 同时声明同一个语义路由，无论内容是否相同，都失败为
`artifact.route_ambiguous`；不得通过不同 fallback 顺序让两个 URL 返回不同文件。

落地要求：

1. Fumadocs `next.config.mjs` 和 `build_overlay.rs` 同时增加 `trailingSlash: true`，新构建优先生成
   `out/docs/index.html`；
2. Template Version 从 `fumadocs-docs@runtime-p6` 升为 `runtime-p7`，旧 p3～p6 Artifact 保持可读；
3. `GenerationContract` 显式携带 `entryRoute/canonicalPolicy`，Browser Validation 不再按 Artifact Type 硬编码；
4. Candidate Build 只有在 Route Manifest 中每个路由唯一映射到 Candidate Manifest 内文件时才能成功；
5. Local Preview 不再使用 `python3 -m http.server`，改为读取 Route Manifest 的静态服务器；
6. Remote Candidate Preview 与 Published Artifact Presenter 也必须读取同一份 Route Manifest；实现语言可以
   不同，但解析结果必须通过共享 Conformance Corpus；
7. Server 禁止目录列表、隐式 `.html/index.html` fallback、Manifest 外文件、symlink 越界和路径穿越；
8. Preview 启动后、Browser Validation 前执行 Entry Route Probe，断言 HTTP 200、Content-Type 为 HTML、
   resolved artifact path/Hash 与 Route Manifest 一致，并校验 Candidate Manifest Hash Header；
9. `/docs` 只作为 `/docs/` 的显式 Alias，二者必须解析到同一个文件 Hash。

对既有 p6 项目，不要求用户修改源码：兼容 Build Adapter 为现有 `docs.html` 生成 Route Manifest。只有新
初始化或显式模板升级才写入 `trailingSlash: true`。

### 5.11 Validation Failure Taxonomy 与责任路由

新增结构化分类：

```rust
enum ValidationFailureOwner {
    Source,
    Build,
    Artifact,
    Serving,
    Runtime,
}

struct ValidationFailure {
    check_id: String,
    owner: ValidationFailureOwner,
    error_kind: String,
    repairable_by_model: bool,
    diagnostic_ref: Option<String>,
}
```

处理矩阵：

| Owner | Runtime 动作 | Run 行为 |
|---|---|---|
| Source | 生成有界 Repair Context，允许一次模型修复 | `repair_required` |
| Build | 先执行一次确定性依赖/Build 分类，再决定是否交给模型 | `build_repair_required` 或失败关闭 |
| Artifact | 中止 Candidate，保留证据，不要求 Source Mutation | `failed/partial`，平台错误 |
| Serving | 用规范 Resolver 重启 Preview 一次 | 成功则继续；再次失败则平台错误 |
| Runtime | 保留 Checkpoint 和 Correlation ID | 失败关闭并告警 |

`preview.publish` 返回的 `suggestedAction` 必须由 Owner 决定。禁止对 Artifact/Serving/Runtime 返回“修改
源码后重新发布”。Source Repair 的 no-source-change guard 保持；平台失败不得使用该 Guard 强迫源码变化。

Owner 不能只根据 `checkId` 或自然语言错误推断，必须按固定 Evidence 优先级分类：

```text
1. Build Receipt 不成功
   → Build
2. Candidate/Artifact/Route Manifest 身份、Hash 或唯一性不成立
   → Artifact
3. Entry Route Probe 无法按 Route Manifest 返回目标 HTML
   → Serving
4. Browser/Verifier/Sandbox 本身 unavailable、timeout 或 contract violation
   → Runtime
5. 正确页面已经加载后的 Required Content/Accessibility/Link/Search/Metadata 失败
   → Source
```

同一个 `search-index` 或 `metadata` Check 只有在 Entry Route Probe 已通过后才能归属 Source。任何
`Unavailable` 都不能默认解释为 Source Failed。分类器输入只允许 Runtime Receipt 和结构化 Check Result，
不得读取模型输出中的 Owner、错误解释或建议动作。

### 5.12 Bounded Repair Context 与双层进展熔断

完整 `state/validation-report.json` 继续作为 Durable Evidence 保存，但不进入 Agent Tool Namespace。Repair
阶段只向模型暴露新增的：

```json
{
  "schemaVersion": "generation-repair-context@1",
  "candidateVersionId": "version-110",
  "candidateManifestHash": "...",
  "sourceFingerprint": "...",
  "owner": "source",
  "failedChecks": [
    {
      "checkId": "search-index",
      "message": "...",
      "observedRoute": "/docs/",
      "observedTitle": "...",
      "targetFiles": ["app/docs/layout.jsx", "lib/layout.shared.jsx"]
    }
  ],
  "maxRepairCycles": 1
}
```

约束：

- 不包含完整 Candidate Manifest、完整 Browser Evidence、Screenshot 或全部成功检查；
- 同时限制 `serializedBytes <= 16 KiB` 与 `estimatedTokens <= 4,000`，任一超出都失败关闭为
  `validation.repair_context_too_large`；
- Target Files 由 Template Editable Surface 与失败检查映射产生，不允许模型猜测 `.tsx/.jsx`；
- Agent 的 `fs.read` Policy 明确拒绝完整 Validation Report、Candidate Manifest 和 Browser Evidence；需要更多
  信息时，由 Runtime 根据 Target File Hash 生成一次有界、签名的 excerpt；
- System Prompt、Workflow Progress 和 Tool Error 统一引用 `state/repair-context.json`，不得继续提示
  模型读取 `state/validation-report.json`；
- 相同 `candidateManifestHash + failedCheckIds` 最多进入一次诊断；
- Source Repair 最多一次 Publish Cycle；修复后的新 Candidate 仍失败时直接 `generation.repair_exhausted`；
- Serving/Artifact Failure 的 Repair Cycle 为 0，不调用模型。

Progress 拆为 Observation Ledger 与单调 Substantive Ledger，Hash 只是 Ledger 的完整性证明，不直接把易变 ID
当作进展：

```text
observationHash
  = read/search/path/report identity

SubstantiveProgressLedger
  - first unseen sourceFingerprint produced by an allowed mutation
  - first unseen candidateManifestHash for that sourceFingerprint
  - first completion of a monotonic workflow milestone
  - first durableSnapshotId / outputVersionId

substantiveProgressHash
  = hash(canonical monotonic ledger)
```

新的 `buildId`、`candidateVersionId`、相同 Source 的重复 Build、相同 Manifest 的重新 Publish，以及 Workflow
Stage 往返都不能新增 Ledger Entry。Milestone 必须来自显式有向状态图，只允许前进或进入终态，不能靠字符串
变化判断。

`max_no_progress_turns` 只观察 `substantiveProgressHash`。Observation Budget 继续独立限制 Read/Search；新的
读取可以丰富诊断，但不能重置主熔断。进入 Repair 后默认：最多 2 个无实质进展模型轮次、最多 1 次
Source Mutation、最多 1 次重新 Publish；新 Candidate 即使出现不同失败证据也不自动开启第二个 Repair
Cycle，避免换一种失败继续消耗预算。

## 6. 数据契约与 API

### 6.1 保留 `RunModelUsage@1`

现有契约继续返回：

```json
{
  "schemaVersion": "run-model-usage@1",
  "runId": "run-123",
  "inputTokens": 268812,
  "outputTokens": 6207,
  "cachedInputTokens": 124800,
  "totalTokens": 275019,
  "estimated": false,
  "turnCount": 19
}
```

不改变 `totalTokens = inputTokens + outputTokens`，避免破坏 Provider Model Service 方案和现有 Web。

### 6.2 新增 `RunPromptEfficiency@1`

```json
{
  "schemaVersion": "run-prompt-efficiency@1",
  "runId": "run-123",
  "grossInputTokens": 268812,
  "cachedInputTokens": 124800,
  "uncachedInputTokens": 144012,
  "outputTokens": 6207,
  "turnCount": 19,
  "maxTurnInputTokens": 18668,
  "averageTurnInputTokens": 14148,
  "cacheHitRateBasisPoints": 4643,
  "generationContextEstimatedTokens": 906,
  "generationContextRepeatedEstimatedTokens": 17214,
  "promptCompactionCount": 1,
  "promptTokensRemovedByCompaction": 4793,
  "largeToolArgumentTokensRetainedPeak": 5010,
  "retryAmplificationBasisPoints": null,
  "estimated": false
}
```

新增路由：

```text
GET /runs/{runId}/prompt-efficiency
```

该路由沿用 Run 的 Project Read 授权，不暴露 Prompt、源码或 Tool 参数。

### 6.3 新增用户操作聚合

为 Build/Edit Start 请求创建稳定 `operationId`。Brief、Build Successor 和 Edit Successor 可关联到同一个
用户操作，但不同用户指令不能被错误合并。

```text
GET /projects/{projectId}/generation-operations/{operationId}/usage
```

`operationId`、`operationAttempt`、前后继 Run 和 Continuation Snapshot 身份同时出现在 Run History 与
`GenerationContextStatus`，调用方不需要从日志或相邻时间推断 Operation。

返回：

- Run/Attempt 列表及每次 Attempt 的状态、Token、轮次和时间戳；
- Gross/Cached/Uncached/Output、总 Token 与总轮次；
- 当前 Operation 状态、自动 Continuation 次数与端到端 Latency；
- Retry Amplification。

活动 Runtime 时长、基础设施恢复等待与用户等待分解需要独立的低敏感度时间事件；在这些事件落地前，API 只返回
端到端 Latency，不得从 Conversation 文本或日志时间猜测分段耗时。

历史 Run 没有 `operationId` 时保持单 Run 展示，不根据相邻时间或相同 Prompt 猜测归组。

### 6.4 新事件

新增 Append-only 事件：

```text
prompt.composition
prompt.microcompacted
prompt.compacted
prompt.projection_restored
prompt.projection_mismatch
token.budget_decision
token.usage_contract_violation
workflow.lifecycle_started
workflow.lifecycle_completed
workflow.lifecycle_failed
run.continuation_created
artifact.route_manifest_created
artifact.route_ambiguous
preview.entry_route_probed
preview.entry_route_resolution_failed
validation.failure_classified
validation.repair_context_created
validation.repair_exhausted
run.substantive_progress
```

`token.budget_decision` 必须同时记录 `mode`、`exhausted` 与 `enforced`；耗尽决策至少包含：

```json
{
  "mode": "legacy|split_shadow|split_enforced",
  "budgetKind": "gross_input|uncached_input|turn_prompt|output|turn|tool_call",
  "used": 202763,
  "limit": 200000,
  "exhausted": true,
  "enforced": true,
  "grossInputTokens": 202763,
  "cachedInputTokens": 88320,
  "uncachedInputTokens": 114443,
  "turn": 13
}
```

`validation.failure_classified` 必须包含 `owner`、`repairableByModel`、`checkIds`、Candidate/Source Identity 和
有界 `diagnosticRef`，但不得包含完整报告或源码。`preview.entry_route_probed` 记录 requested route、resolved
artifact path、HTTP status、content type 和 manifest hash；不记录页面正文。

## 7. 代码改造范围

| 文件/模块 | 改造内容 |
|---|---|
| `services/runtime/src/agent_loop.rs` | Usage V2、拆分预算、稳定 Prompt、Ephemeral Progress、Token 驱动压缩、Projection 调用 |
| `services/runtime/src/model_gateway.rs` | 明确 Stable/Ephemeral Message 顺序；保持 OpenAI Tool Pair 转换 |
| `services/runtime/src/types.rs` | 新 Event、Checkpoint Projection 元数据、Operation/Continuation identity |
| `services/runtime/src/run_metrics.rs` | Prompt Efficiency 与 Operation 聚合；Provider Usage Contract 校验 |
| `services/runtime/src/conversation.rs` | Durable Transcript 与 Projected Window 的持久化、恢复和 Hash 校验 |
| `services/runtime/src/tools/runtime.rs` | Stage-aware Eager/Deferred Tool Snapshot、稳定 Tool Set Hash |
| `services/runtime/src/run_lifecycle/start.rs` | Phase Budget Profile、operationId、Continuation Preflight |
| `services/runtime/src/run_lifecycle/continue_run.rs` | Successor Run 与冻结 Source Snapshot 复用 |
| `services/runtime/src/runtime/` | `RuntimeWorkflowDriver`、生命周期幂等恢复 |
| `services/runtime/src/generation_contract.rs` | `entryRoute/canonicalPolicy`、Route Manifest 与 Failure Owner 契约 |
| `services/runtime/src/templates/fumadocs_docs/files/next.config.mjs` | 新模板启用 `trailingSlash` |
| `services/runtime/src/templates/fumadocs_docs/build_overlay.rs` | Overlay 与模板使用相同 Next Route 配置 |
| `services/runtime/src/templates/fumadocs_docs/mod.rs` | p7 版本、Route Contract、旧版本兼容 |
| `services/runtime/src/tools/sandbox/project/build.rs` | 从 Template Contract 与实际 Artifact 生成唯一 Route Manifest |
| `services/runtime/src/tools/sandbox/preview/lifecycle.rs` | Local Preview 移除 Python Server，只按 Route Manifest 服务 |
| `services/runtime/src/tools/sandbox/preview/validation.rs` | Entry Route Probe、Owner 分类、有界 Repair Context |
| `services/runtime/src/tools/sandbox/preview/publish.rs` | 按 Failure Owner 路由，不再统一强制 Source Repair |
| `services/runtime/src/http_api/artifact_presenter.rs` | Published Work 使用同一 Route Manifest 语义 |
| `infra/agent-sandbox/base/static-preview-server.js` | Remote Preview 按 Route Manifest 解析，不做隐式 fallback |
| `services/runtime/evidence/replay/` | 脱敏 Evidence Bundle、共享 Route Corpus 和 Failure Fixtures |
| `infra/generation-reliability/replay-evidence.mjs` | 离线重放 Usage、Route、Owner 和 Progress Ledger |
| `services/runtime/src/http_api/routes/runs/metrics.rs` | `/prompt-efficiency` |
| `services/runtime/src/http_api/routes/projects.rs` | Operation Usage 查询 |
| `packages/shared/src/api-types.ts` | 新严格 Schema，不修改 `RunModelUsage@1` |
| `packages/shared/src/runtime-client.ts` | 新查询方法 |
| `apps/web/components/project-shell.tsx` | Gross/Cached/Uncached、Attempt 汇总和明确失败原因 |
| `infra/agent-sandbox/runtime/deployment.yaml` | 新预算模式与 Shadow 配置 |
| `infra/generation-reliability/` | 定向前后对比、Restart、No-cache、Large-write 门禁 |

Provider Gateway 已能从标准 `usage.prompt_tokens_details.cached_tokens` 解析缓存 Token。除增加
`cached <= input` 的低敏感度异常审计外，不需要改变 Provider API 或凭证模型。

## 8. 实施阶段与提交计划

### Commit 0A：Route Oracle、Conformance Corpus 与 Replay Bundle（P0）

- 定义 `generation-contract@2` 与 `artifact-route-manifest@1`；
- Build Adapter 从 Template Contract 生成唯一 Route Manifest；
- `docs.html + docs/index.html` 同路由时确定性拒绝为 `artifact.route_ambiguous`；
- 建立 Node/Rust/Published Presenter 共用的 JSON Conformance Corpus；
- 将脱敏 p6 Docs Artifact、run-7 Stream Summary 和 Failure Fixtures 冻结为 Evidence Bundle；
- 实现离线 `replay-evidence`，暂不改变生产执行路径。

退出条件：Route Manifest Schema/Hash 稳定；Corpus 可离线运行；run-7 的 Route/Owner/Progress 可从 Bundle
重放；Artifact 双映射必定失败且不依赖文件遍历顺序。

### Commit 0B：Resolver 同构与 Fumadocs p7（P0）

- Fumadocs Template 和 Build Overlay 增加 `trailingSlash: true`，发布 `runtime-p7`；
- Local Preview 移除 Python Server，改为 Route Manifest Server；
- Remote Preview 和 Published Presenter 只按 Route Manifest 服务；
- p3～p6 由兼容 Build Adapter 生成 Manifest，不修改用户源码；
- 禁止目录列表、隐式 fallback、Manifest 外文件、symlink 越界和路径穿越。

退出条件：p7 `docs/index.html` 与 p6 `docs.html` 分别生成唯一 Manifest；`/docs` 和 `/docs/` 在 Local、Remote、
Published Work 中解析到相同文件 Hash；完整安全 Corpus 通过。

### Commit 0C：Entry Probe 与 Failure Owner Shadow（P0）

- Browser Validation 前执行 Entry Route Probe；
- 按 Build → Artifact → Serving → Runtime → Source 的证据优先级分类；
- 新事件记录旧决策、新 Owner、差异原因和 Correlation ID；
- Shadow 阶段只记录，不改变现有 Repair 行为。

退出条件：固定 Source/Build/Artifact/Serving/Runtime Fixtures 分类 100% 确定且 Restart 后一致；Metadata/Search
只有在 Entry Probe 通过后才可能归为 Source；任何 `Unavailable` 不会归为 Source。

### Commit 0D：Owner Enforcement、Bounded Repair 与单调进展（P0）

- Artifact/Serving/Runtime Failure 不进入 Agent Source Repair；
- Serving 只允许一次确定性 Preview Restart，再失败则平台错误；
- Agent 只可读取 ≤16 KiB 且 ≤4k Token 的 Repair Context；
- Tool Policy 拒绝完整 Validation Report、Manifest 和 Browser Evidence；
- 用单调 Substantive Progress Ledger 替代包含易变 ID 的 Fingerprint；
- 对相同 Candidate/Blocker 增加有界 Repair Cycle。

退出条件：Serving Failure 分类完成后的新增模型调用数为 0，Source Fingerprint 不变化；连续 Read/Search、
重复 Build ID、重复 Candidate Version 和 Stage 往返均不构成实质进展；合法新 Source/Candidate 可推进；
Fumadocs Production Build、Candidate、Acceptance 和 Published Work 回归通过。

最小验证命令：

```bash
cargo test --manifest-path services/runtime/Cargo.toml \
  tools::sandbox::preview::validation::tests
cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools \
  static_preview_server_serves_only_frozen_candidate_snapshots
cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools \
  preview_start_spawns_static_server_from_fumadocs_out
cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools \
  fumadocs_docs_real_next_build_smoke
node infra/generation-reliability/replay-evidence.mjs --conformance \
  services/runtime/evidence/replay/contracts/artifact-route-conformance@1.json
node infra/generation-reliability/replay-evidence.mjs \
  services/runtime/evidence/replay/fixture-docs-route-failure
```

真实 Provider 用例继续为 ignored/manual gate，只在 Fixture 与 K3d Contract 全部通过后执行一次，不作为日常
单元测试依赖。

### Commit 1：Prompt 与 Token 可观测性

- 新增 `RunTokenUsageV2` 内部聚合；
- 新增 Prompt Composition 估算和 Hash 事件；
- 新增 `RunPromptEfficiency@1`；
- Web 展示 Gross、Cached、Uncached、Cache Hit Rate、Turn Peak；
- 不改变任何执行决策。

退出条件：具备完整 Durable Events 的历史六个 Run 与 run-7 Evidence Bundle 可离线重算；run-8 只作为
不可重放的调查基线，不用于自动 Gate。使用相同 Prompt 新执行一次 Website 并生成完整 Bundle 后，替代
run-8 进入回归集合；所有可重放样本的新旧 `RunModelUsage@1` 完全一致。

### Commit 2：拆分预算

- 增加 Gross/Uncached/Per-turn 配置；
- 实现 `legacy` 与 `split_shadow`；
- 增加明确的 Budget Kind 和错误文案；
- Provider Usage 缺失、估算、缓存大于输入均有测试。

退出条件：两个历史失败 Build 在 Shadow 中显示“Legacy Gross 会失败，Split Budget 不会因缓存本身失败”。

### Commit 3：稳定 Prompt 前缀

- 将 Workflow Progress 与 Repair Target 移到尾部 Ephemeral Message；
- 同类动态消息只保留最新一条；
- Tool Definition 稳定排序并记录 Tool Set Hash；
- 增加跨 Turn Stable Prefix 回归。

退出条件：Workflow Stage 改变时 `staticPrefixHash` 不变；DeepSeek 定向 Smoke 的 Cache Hit Rate 不低于
当前基线，且功能结果一致。

### Commit 4：Transcript Projection 与 Microcompact

- Durable Transcript 与 Active Window 分离；
- 成功大 Tool Exchange 确定性压缩；
- Token/字节/消息数联合触发 Full Compaction；
- Checkpoint 保存 Projection Version/Hash；
- Restart 重建一致性门禁。

退出条件：20k 字符 `fs.write` 在下一模型轮次不再完整重传；Tool Pair、Permission、Repair Diagnostic 和
Restart 测试全部通过。

### Commit 5：Runtime Workflow Driver

- 自动推进 Next Greenfield、Cold Dev、Warm HMR 和 Style Token 生命周期；
- 失败时结构化返回模型；
- 成功时原子 Draft Ready；
- 保留 Fumadocs `preview.publish` 单入口。

退出条件：简单 Style/Token Edit 不超过 6 个模型轮次；构建失败仍能回到模型完成一次有界修复。

### Commit 6：Partial Continuation 与 Operation Budget

- 6A（已落地本地路径）：新增 `operationId`、`operationAttempt`、`predecessorRunId` 与事件契约；
- 6A（已落地本地路径）：`GenerationOperationUsage@1` 跨 Attempt 汇总 Token、轮次、延迟和 Retry Amplification；
- 6A（已落地本地路径）：Operation Gross/Uncached/Output/Turn/Tool Budget 支持 `shadow`/`enforced`，默认 `shadow`；
- 6B（已落地本地 Snapshot 路径）：`RunContinuationSnapshot@2` 验证 Runtime-owned Source Manifest/Hash，冻结
  Checkpoint、Workflow Progress、Substantive Progress Ledger、Generation/Content/Design/Edit/Base 身份、累计
  用量和剩余预算；
- 6B（已落地本地 Snapshot 路径）：失败 Build 返回 Source Snapshot 身份；Build 前无 Snapshot、Hash 不匹配或
  身份不完整时失败关闭；
- 6B（已落地本地 Eligibility 路径）：重新验证 Source、Workspace Revision、Context/Plan/Profile/Edit/Base、
  Checkpoint、Operation Budget 与一次自动续跑上限；
- 6B（已落地本地 Coordinator 路径）：`off`/`shadow`/`enforced` Controller 重新计算 Eligibility；`enforced` 通过
  标准 RunLifecycle 创建、绑定并启动 Successor，恢复不可变 Source Snapshot，不复用父 Run Checkpoint/PVC；
- 6B（已落地定向回归）：最多一次自动续跑；并发 reconcile 只创建并启动一个 Successor，第二次 Partial 因
  `automatic_continuation_count` 失败关闭；
- 6B（已落地本地门禁）：Enforced 只允许精确 Phase/Profile allowlist；Store/Lifecycle/Controller 重建后的
  Restart reconcile 不会重复创建或启动 Successor；Restart evidence 强制 Budget Profile ID/Hash/Mode 不漂移；
- 6B（发布前未完成）：真实跨进程 Restart Replay 和真实 Kubernetes Sandbox Snapshot 恢复证据。

退出条件：预算或 Provider 中断后的 Successor 不重新生成未变化页面；Context 或 Base 变化时拒绝复用；并且
在 Source Snapshot 不可验证时不会创建 Successor。上述发布前证据未完成前，Commit 6 不得退出生产 Shadow。

### Commit 7：Enforcement、UI、基础设施与文档冻结

- 从 `split_shadow` 切换定向 Candidate 到 `split_enforced`；
- 已固化 `RunBudgetProfile@1`：实际执行上限与 Phase Target 同时冻结到 Run，Profile Hash 防止恢复漂移；
- 提供只读鉴权 Budget Profile API；历史缺 Profile 明确失败，不从当前 Pod Env 补造；
- Terminal Bundle 冻结场景内全部 Run 的完整 Profile，Replay 重算 Profile Hash 与逐 Turn/Run/Operation Usage；
- 增加版本化 Release Budget Policy；Production 只接受批准的 Enforced Mode/Profile/上限，Shadow 只输出 Readiness；
- 补齐 Phase Budget 生产 Shadow 分布与 Enforced cohort gate；
- UI 展示明确口径和 Retry Amplification；
- 更新 K3d/CI/Runbook/Release Evidence；
- 完成 Release 终态 Smoke 后冻结基础设施。

退出条件：第 10 节全部门禁通过；不继续扩大量化样本。

## 9. 测试方案

### 9.1 Rust 单元测试

必须覆盖：

1. `cachedInputTokens` 是 Input 子集，不重复进入 Total；
2. `cached > input` 产生 Contract Violation 且安全 Clamp；
3. 同一 Turn 重放只保留最新 Usage；
4. Restart 从事件恢复相同 Gross/Cached/Uncached；
5. Legacy、Split Shadow、Split Enforced 的决策矩阵；
6. Per-turn Prompt 在 Provider 调用前可由保守估算阻断；
7. Provider 实际 Usage 超出估算时在响应后正确终止；
8. Workflow Progress 改变不改变 Stable Prefix Hash；
9. Ephemeral Progress 只保留最新一条并位于尾部；
10. Tool Definition 顺序确定且 Hash 稳定；
11. 成功大 `fs.write` Tool Pair 被成对投影为摘要；
12. 失败 Tool、Pending Permission、未配对 Tool Call 不压缩；
13. Token 阈值可触发压缩，即使消息少于32条；
14. 压缩后 Source Restore 不超过 Phase Budget；
15. Projection 在 Restart 后 Hash 一致；
16. Projection 不一致失败关闭；
17. Workflow Driver 成功、失败、取消、超时、Restart 幂等；
18. Continuation 对冻结身份匹配和不匹配分别允许/拒绝；
19. Operation Budget 跨 Attempt 累计且最多自动续跑一次；
20. 所有 Metric/Event 均不包含 Prompt、源码或 Secret；
21. `/docs` 与 `/docs/` 在 `docs.html`、`docs/index.html` 两种 Artifact 上解析一致；
22. Route Resolver 禁止目录列表、路径穿越和 Candidate Root 外读取；
23. Entry Route Probe 能区分 HTML、404、目录、非 HTML 与 Manifest Hash 不匹配；
24. Serving/Artifact Failure 不设置 `repair_required`，也不要求 Source Mutation；
25. Source Failure 生成有界 Repair Context，且 Target Files 使用模板真实扩展名；
26. Observation Hash、重复 Build/Version 和 Stage 往返不重置 Substantive No-progress；首次出现的新
    Source/Candidate Digest 或单调 Milestone 可以重置；
27. 相同 Candidate + Blockers 达到 Repair Cycle 上限后确定性终止。

### 9.2 Contract 测试

- `RunModelUsage@1` 不变；
- `RunPromptEfficiency@1` Rust/Shared/Web 严格一致；
- `GET /runs/{run_id}/budget-profile` 的鉴权、404/409、Phase/Hash 校验和只读语义；
- `RunBudgetProfile@1` 的 Rust/Node Canonical JSON Hash Golden Vector 一致；
- Operation Usage 的无历史分组行为；
- 新 Budget Error 的 HTTP/SSE 序列化；
- Checkpoint 新字段向后兼容；
- 旧 Event Log 可恢复；
- Provider Gateway `cachedInputTokens` 缺失时为0；
- `GenerationContract` 旧版本默认 Entry Route 的兼容读取；
- `ArtifactRouteManifest@1` 在 Node/Rust/Published Presenter 中使用同一 Golden Corpus；
- `RuntimeEvidenceBundle@1`、Checksums 和离线 Replay Schema；
- `runtime-evidence-budget-profiles@1` 覆盖单 Run、多 Run Repair、重复 Run、缺 Profile、篡改限制值、未知 Mode
  和超限 Usage；
- Validation Failure Owner、Repair Context 和 Route Probe Event 的 HTTP/SSE 严格序列化。

### 9.3 本地与 Fixture 集成测试

| 场景 | 重点断言 |
|---|---|
| 20k 字符新页面写入 | 下一轮不重传完整 Tool 参数 |
| 多次小 Patch | 不过度压缩最新 Repair Target |
| Build 成功 | Workflow Driver 完成 Snapshot |
| Build 失败后修复 | Diagnostic 保留，修复后压缩旧失败 |
| Warm HMR | 不执行 Production Build |
| Style Token Edit | Style Contract/CAS 保持有效 |
| Permission Pause/Resume | Tool Pair 与预算恢复一致 |
| Runtime Restart | Projection、Usage、Workflow 均一致 |
| Provider 无 Cached Usage | Uncached 等于 Gross，保守执行 |
| Provider Usage 异常 | Contract Violation 可观测且不下溢 |
| Fumadocs p7 静态导出 | `out/docs/index.html`，`/docs/` 验证通过 |
| Fumadocs p6 兼容 Artifact | 只有 `out/docs.html` 时 `/docs`、`/docs/` 均返回相同 HTML |
| Local/Remote Preview 对比 | 相同 Route Contract、status、content-type、manifest hash |
| Preview Resolver 故障 | 不调用模型、不修改 Source，以 Serving Owner 失败关闭 |
| Docs 真实 Source 缺少 Search | 只下发有界 Repair Context，一次修复后重新 Publish |
| 连续诊断读取 | Observation 增长，但 Substantive No-progress 在配置轮次终止 |

Route Conformance Corpus 必须由 Node Preview Server、Rust Validator、Artifact Presenter 三方读取同一份 JSON
Vectors，并至少覆盖：`/docs`、`/docs/`、query、fragment 前 URL、重复 slash、大小写、percent encoding、
`..`/encoded traversal、symlink、HEAD、Content-Type、Cache Header、Manifest 缺失/过期/Hash 不匹配、双路由
文件、Manifest 外文件、Base Path、并发 Candidate、端口冲突、Server Crash 和终态进程清理。三方只比较规范
结果对象，不比较实现日志文本。

### 9.4 Fixture、Benchmark 与真实 Provider 分层

#### 9.4.1 确定性 Fixture Gate

以下场景使用 Fixture Model/Gateway 和固定 Artifact，不产生真实 Provider 成本，并作为每次 PR 的硬门禁：

1. Build Failure、Serving Failure、Artifact Ambiguity、Verifier Unavailable 的 Owner 分类；
2. Provider 不报告 Cached Usage、返回异常 Usage 和同 Turn Replay；
3. Runtime Restart、Permission Resume、Projection Restore；
4. 大 Tool Pair、完整 Validation Report 和重复 Read/Search；
5. Route Corpus、路径穿越、symlink、Manifest 漂移、进程退出和端口冲突。

故障必须通过注入式 Harness Backend 产生，例如 `FixturePreviewServerMode::DirectoryListing`、
`FixtureVerifierMode::Unavailable`、`FixtureArtifactMode::AmbiguousRoute`；禁止通过随机杀进程、抢端口或修改生产
源码制造非确定性失败。

#### 9.4.2 Benchmark Cohort

P50/P95 使用版本化 Prompt Set、固定 Design Profile/Template 和同一 Model Revision。每个 Profile 至少 30 个
Accepted Attempt、至少 10 个 Prompt；失败 Attempt 单独报告，不能从分位数样本中静默删除。输出包括样本数、
分布、置信区间和与 Baseline 的 Effect Size。达不到样本数时结果为 `insufficient_sample`。

定量 Benchmark 使用独立的 `runtime-efficiency-benchmark-cases.json`，不修改五用例真实 Provider Functional
Canary。该 Corpus 固定 10 个 Design System Website Prompt，因此同一组 Prompt 可同时执行 Greenfield Build 和
Style/Token Edit；两类 Workload 默认各重复 3 次，得到每个 Variant/Workload 30 个 Attempt。Dry Run 只生成
执行计划，不触发 Provider。Runner 本身会重新验证 Corpus Version、10 个唯一 ID/Prompt/Expected Text、Website
Kind、`/` Route 和 Design System 主题，不能仅凭 Session 中 SHA 匹配绕过语义约束：

```bash
GENERATION_COHORT_CASES_FILE=infra/generation-reliability/runtime-efficiency-benchmark-cases.json \
  bash infra/generation-reliability/prepare-generation-context-cohort-session.sh

GENERATION_EFFICIENCY_DRY_RUN=1 \
  bash infra/generation-reliability/run-runtime-efficiency-benchmark-cohort.sh \
  <prepared-session-dir> <batch-prefix>
```

移除 `GENERATION_EFFICIENCY_DRY_RUN=1` 才会顺序执行真实 Provider Pair；这是显式付费操作，不属于普通
Contract Gate。脚本会在任何 Provider 调用前验证 Paired Ledger Hash-chain、Sample Schema 与 Pair 身份；遇到
篡改或半写 Pair 会停止要求审计，只有验证后的完整 Pair 可跳过，不会静默重复付费。

该门禁由以下版本化产物执行：

- `runtime-efficiency-benchmark-session@1`：冻结 Prompt Set、Profile、Template 与 Provider/Model 身份；
- `runtime-efficiency-benchmark-ledger-record@1`：append-only Hash Chain，Attempt 序号必须连续且 ID 唯一；
- `runtime-efficiency-benchmark-cohort@1`：由 Ledger Assembler 计算原始 Ledger SHA，不接受手工自报；
- `runtime-efficiency-benchmark-evaluation@1`：输出失败分类、P50/P95、Bootstrap 区间、Effect Size 与门禁。
- `runtime-efficiency-benchmark-source-binding@1`：证明 Benchmark Attempt 与原始 Paired Ledger + Mapping 的确定性派生结果完全一致。

真实样本不得手写 Attempt JSON。Runner 必须同时采集 `run-efficiency-metrics@1` 和
`run-prompt-efficiency@1`，Paired Cohort Sample 只保留低敏感度数值与 Hash。完成 Control/Candidate Pair 后，
由桥接 Collector 验证原始 Paired Ledger Hash-chain、Provider/Template/Profile/Prompt 身份，再原子追加两个
Benchmark Attempt：

```bash
node infra/generation-reliability/prepare-runtime-efficiency-benchmark.mjs \
  <generation-context-paired-ledger.ndjson> <new-benchmark-directory>

node infra/generation-reliability/collect-runtime-efficiency-benchmark.mjs \
  sync \
  <generation-context-paired-ledger.ndjson> \
  <new-benchmark-directory>/benchmark.ndjson \
  <new-benchmark-directory>/import-mapping.json
```

生产 Runner 使用 `sync`：存在新 Attempt 时原子导入；没有新增时重新验证完整 Source Binding 并成功退出，确保
完成态重复执行是只读且幂等的。底层 `collectRuntimeEfficiencyBenchmarkAttempts` 仍保持“无新增即失败”，用于
Fixture 和审计代码发现意外重复调用。

Mapping 固定 `greenfield → greenfield_build`、`warm_copy_css → style_token_edit` 的 Profile ID，并将 Fixture ID
映射到冻结 Prompt Set 中的 Prompt ID。其他 Bucket 不进入本门禁。Collector 会跳过已导入的 Pair Side；若同批
任一 Accepted 样本缺失 turns/token/cache/首次变更/上下文字节/重复读取/越界变更/Fidelity 指标，整批不写入。
Benchmark 采集必须使用 `GENERATION_COHORT_MAX_CASE_ATTEMPTS=1`；Sample 冻结 Case Attempt 数，Collector 对不等于
1 的 Control/Candidate 失败关闭，防止内部失败重试从失败分布中消失。
`generationContextBytes` 是 Runtime `generationContextEstimatedTokens × 4` 的保守字节上界，因向上取整最多比
序列化原文多 3 bytes；Generation Context 关闭的 Baseline 值为 0 合法。

每个选中 Run 还必须携带由 Runtime 冻结 Design Context Manifest 投影的
`run-design-profile-identity@1.effectiveProfileHash`。Pair 两侧必须完全一致，Pair Ledger 将该 Hash 作为身份字段，
Benchmark Import 必须再次等于目标 Profile 的 `designProfileHash`；不得依据管理员当前选择或测试 Mapping 补造。

Paired Session 必须在 Hash-chain 起始记录中冻结 `{source: {commit, dirty}}` 与 `fixtureManifestSha256`。
Benchmark Session 的 Source 必须逐字段相同，Prompt Set SHA 必须等于该 Fixture Manifest SHA；Release Source
Binding 再将 Paired Source 与 clean Release Commit 比较。旁路 `session-meta.json` 只提供部署协调信息，不能
单独证明统计样本来源。

```bash
node infra/generation-reliability/runtime-efficiency-benchmark-ledger.mjs verify <benchmark-ledger.jsonl>
node infra/generation-reliability/runtime-efficiency-benchmark-ledger.mjs assemble \
  <benchmark-ledger.jsonl> <benchmark-cohort.json>
node infra/generation-reliability/runtime-efficiency-benchmark.mjs <benchmark-cohort.json>
```

Release RC 必须同时设置：

```bash
RUNTIME_RC_EFFICIENCY_BENCHMARK_LEDGER=<benchmark-ledger.jsonl>
RUNTIME_RC_EFFICIENCY_SOURCE_LEDGER=<generation-context-paired-ledger.ndjson>
RUNTIME_RC_EFFICIENCY_IMPORT_MAPPING=<benchmark-import-mapping.json>
```

Aggregate 与最终 Validator 都直接读取这三份原始输入，验证 Paired Hash-chain，重新派生全部 Attempt，并比较
Benchmark raw SHA、Cohort、Evaluation 和 Source Binding。只提交手工 Cohort/Evaluation、只提供 Benchmark
Ledger，或手工 append 一个结构合法且阈值通过的 Attempt 都不能满足门禁。
通用 `ledger.mjs append` 只用于 Fixture 与 Contract 测试；生产 Release Benchmark 的 Attempt 必须由上述
Paired Collector 生成并保留对应原始 Paired Ledger，禁止人工复制 Runtime UI 或日志中的 Token 数值。

#### 9.4.3 真实 Provider 终态 Smoke

不为了获得漂亮的统计重复付费执行。Fixture、Contract、Replay 和 Benchmark 条件满足后，对以下五个真实
Runtime API 场景各执行一次：

1. 当前“什么是 Design Engineer”Greenfield Website；
2. 对同一作品执行“修改为亮色主题”；
3. 一个预期 Build Failure 后单次 Repair；
4. 一个 Runtime Restart 后继续的 Build；
5. Orbit Design System Fumadocs Docs，验证 `/docs/`、Search、Metadata 和 Candidate Promotion。

每个真实场景保存为 `RuntimeEvidenceBundle@1`：

- Run/Operation identity；
- Model Resource revision；
- Gross/Cached/Uncached/Output；
- 每轮 Input 曲线；
- 场景内每个 Run 的完整 `RunBudgetProfile@1` 与原始文件 SHA-256；
- 按冻结 Profile 重算的 Per-turn/Run/Operation Budget Conformance；
- Prompt Composition 分类与 Hash；
- Cache Hit Rate；
- Compaction before/after；
- Tool Call、首次 Mutation、Draft Ready 和总时长；
- Required Fidelity、Source Snapshot、DraftSnapshot；
- Sandbox/PVC terminal cleanup；
- 不含 Prompt/源码/Secret 的 Evidence 摘要。

真实 Smoke 只断言协议、功能和确定性上限，不对单次 Token、Cache Hit Rate 或 P50/P95 做精确断言。

## 10. 发布门禁

### 10.1 功能硬门禁

- Greenfield Website 可通过真实 Runtime API 完成；
- Fumadocs Docs 的 `/docs`、`/docs/` 在 Local Preview、Remote Preview 和 Published Work 返回相同文档 HTML；
- Entry Route Probe 在 Browser Validation 前通过，且禁止目录列表；
- Serving/Artifact 故障不会触发模型 Source Repair；
- 亮色主题修改准确反映在当前 Draft；
- Required Text、Design Profile Fidelity 和 Acceptance 不退化；
- DraftSnapshot 可恢复；
- Published Work 继续由显式 PublishWorkflow 创建；
- Runtime Restart、Provider Retry、Permission Resume 不重复执行已完成 Mutation；
- Sandbox/PVC 在终态释放；
- 无越界 Mutation、无跨项目数据读取、无 Secret 泄漏。

### 10.2 Token 硬门禁

- 不再出现仅因 Cached Input 被全额累计而提前终止的 Run；
- 每个终态 Bundle 都包含完整 `budget-profiles.json`；Manifest 的 `budgetProfilesSha256`、Checksums、Run/Phase、
  Context/Restart 的 Profile ID/Hash/Mode 必须一致；
- Replay 必须按 `(runId, turn)` 从 `events.ndjson` 重算 Usage，并与 `usage.json` 逐 Turn、逐 Run 完全一致；
- 每个模型调用的实际 `inputTokens` 不超过该 Run 冻结 Profile 的 `maxPromptTokensPerTurn`；不得用平均值、
  P95 或 Runtime 自报布尔值替代逐 Turn 比较；
- 每个 Run 的 Turns、Tool Calls、Gross、Uncached 和 Output 均符合冻结执行上限；多 Run 场景还必须按
  `operationId` 汇总并符合 `operationLimits`；
- Release 使用的 Profile 必须匹配仓库批准的 Phase/Profile Policy。`shadow`、缺 Profile、未知 Mode、Hash
  不一致、上限为0、Bundle 自带但未批准的宽松上限均不具备 Release Eligibility；
- Release Evidence 必须包含 `ProviderCacheSmokeAudit@1` 且为 `passed`；缺失证据、稳定前缀未重复、估算 Usage
  或 `provider_not_reporting_cached_usage` 均失败关闭；
- Cache Audit 必须来自 accepted 且 `realProviderVerified=true` 的 Suite，并与 Release Evidence 使用完全相同的
  clean source commit、`modelResourceId`、`providerResourceRevision` 和 `providerConfigSha256`；任一字段缺失或
  不一致都失败关闭，不能复用旧 Model Revision/Provider Config 的缓存结果；
- Release Aggregate 与最终 Validator 必须从逐 Run 记录重算 `auditedRunCount`、`stableRunCount`、Gross Input 和
  Cached Input；只修改 `status/releaseEligible` 不能提升不完整证据；
- 20k 字符 Tool Call 不跨多个后续轮次完整重传；
- Serving/Artifact/Runtime Failure 分类后新增模型轮次为0；
- Operation Retry Amplification ≤ 1.5x；
- Metric 可解释 Gross、Cached、Uncached 和重试来源。

Benchmark 目标单独报告：简单亮色 Edit P50 ≤100k/P95 ≤140k，Greenfield P50 ≤140k/P95 ≤180k，支持
缓存的 DeepSeek Cache Hit Rate 目标 ≥60%。只有满足第9.4.2节样本数、同分布 Prompt 和固定 Model Revision
后才能升级为 Release Gate；单次终态 Smoke 不参与分位数判定。

### 10.3 发布与基础设施终态 Smoke

修复代码通过后只执行一次终态 Smoke：

```text
真实 Build
→ 真实亮色 Edit
→ 真实 Fumadocs Docs Build 与 /docs/ Candidate Validation
→ DraftSnapshot
→ PublishWorkflow
→ Published Work HTTP 200
→ Runtime/Packager Ready
→ Sandbox/PVC terminal cleanup
→ Token/Prompt Evidence 冻结
```

Smoke 完成后不为了获得更漂亮的统计继续付费扩大样本。未达到效率门禁时回到具体实现修复，使用相同固定
场景重新验证，不更换 Fixture 掩盖退化。

## 11. Rollout 与回滚

### 11.1 Rollout 顺序

```text
route oracle + replay corpus
→ manifest resolver + fumadocs p7
→ entry probe + validation owner shadow
→ validation owner enforced + bounded repair + monotonic progress
→ observability only
→ split budget shadow
→ stable prompt enabled
→ microcompact shadow
→ microcompact enabled
→ workflow driver shadow
→ workflow driver enabled
→ continuation enabled
→ split budget enforced
→ release terminal smoke
→ infrastructure frozen
```

### 11.2 独立回滚开关

```text
RUNTIME_AGENT_TOKEN_BUDGET_MODE=legacy
RUNTIME_AGENT_PHASE_BUDGET_MODE=off
RUNTIME_PROMPT_LAYOUT_MODE=legacy
RUNTIME_TRANSCRIPT_PROJECTION_MODE=off
RUNTIME_AGENT_WORKFLOW_DRIVER_MODE=off
RUNTIME_AGENT_CONTINUATION_MODE=off
RUNTIME_AGENT_CONTINUATION_ALLOWLIST_JSON=[]
RUNTIME_VALIDATION_FAILURE_OWNER_MODE=legacy|shadow|enforced
RUNTIME_VALIDATION_REPAIR_CONTEXT_MODE=legacy|bounded
```

Route Resolver 和禁止目录列表属于正确性/安全修复，不提供回退到 Python Directory Listing 的开关。若 p7
模板产生兼容性问题，只回滚新模板选择到 p6；共享 Resolver 继续支持 p6 的 `docs.html`。

回滚只改变新 Run。已经启动的 Run 使用启动时冻结的 `executionProfile`、Budget Profile、Prompt Layout
Version 和 Projection Version，不能在运行中切换语义。

### 11.3 回滚安全

- 新事件 Append-only，旧版本可忽略；
- `RunModelUsage@1` 保持不变；
- Checkpoint 新字段可选，旧 Checkpoint 使用 Legacy Projection；
- Durable Transcript 永远保留原始事实，关闭 Projection 后可重新构造 Legacy Window；
- Continuation 不修改 Predecessor Run；
- 不执行数据库破坏性迁移；
- 不回滚用户已生成的 DraftSnapshot、WorkVersion 或 Publication。

## 12. 风险与防护

| 风险 | 后果 | 防护 |
|---|---|---|
| 压缩删除模型仍需的源码 | Repair 质量下降 | 保护当前 Target；失败时允许一次定向 Read |
| Tool Pair 被破坏 | Provider 协议错误 | 成对投影；Pending Permission 禁止压缩 |
| 缓存率成为错误目标 | 固定过期状态或降低正确性 | Cache 仅效率门禁，Workflow State 继续权威更新 |
| 旧 Provider Cache 证据串入新 Release | 错误模型或旧配置误通过效率门禁 | Cache Audit 与 Release 对 Commit、Model Resource、Revision、Config Digest 四元组做双重交叉校验 |
| Bundle 只保存 Budget Profile Hash | 无法证明具体限制值或实际用量合规 | 冻结完整 Profile；Rust/Node 重算 Hash；逐 Turn/Run/Operation 回放；与批准 Policy 交叉校验 |
| Bundle 自带超大预算并自报通过 | 低效 Run 通过自签名门禁 | Release Policy 独立于 Bundle，限制允许的 Mode、Profile Hash 和上限范围；Shadow 仅可 Readiness |
| Uncached Budget 低估真实成本 | Cached Token 仍有成本 | 同时保留 Gross 硬上限，不建立零成本假设 |
| Workflow Driver 自动循环 | 隐性高成本或重复副作用 | 只执行确定性成功路径；首次失败立即返回模型 |
| Continuation 复用错误源码 | 跨目标污染 | 新 Run Binding + Snapshot Hash + Context/Base 匹配 |
| Restart 投影漂移 | 重复 Tool 或遗漏事实 | Projection Version/Hash 不一致失败关闭 |
| Phase Budget 过紧 | 合法复杂生成被阻断 | Shadow 后启用；显式用户可见原因；不自动无限续跑 |
| Tool Set 过度裁剪 | 模型无法修复 | 失败阶段加载对应诊断工具；保留显式升级入口 |
| 观测事件泄露 Prompt | 安全与隐私问题 | 只记录分类、计数、Hash、字节和 Token 估算 |
| p7 Trailing Slash 改变既有链接 | 旧链接跳转或重复路由 | Resolver 同时支持两种形式；Canonical 由 Route Contract 决定 |
| Failure Owner 分类错误 | 平台错误交给模型或源码错误提前失败 | Shadow 对比旧决策；按 Check ID + Probe Evidence 分类 |
| Repair Context 过度裁剪 | 模型缺少必要诊断 | Template 映射 Target Files；允许一次有界定向 Read，不回退完整 Manifest |
| Resolver 路径穿越 | 跨 Candidate 读取 | `path.resolve` Root 边界、Manifest Membership、无目录列表、专项安全测试 |
| Read 不再算进展导致过早熔断 | 合法复杂诊断被终止 | Observation Budget 独立保留；首次出现的新 Source/Candidate 或单调 Milestone 才重置主熔断 |
| Node/Rust Resolver 语义漂移 | Preview 与 Published Work 不一致 | 共享 Golden Corpus；每个实现返回同一规范结果对象 |
| Build/Version ID 制造假进展 | 诊断循环无法熔断 | 单调 Progress Ledger；重复 Source/Manifest 和 Stage 往返不记账 |
| 临时 Run ID 被当作证据 | 无法重放或关联错误 Store | Evidence Bundle + Store Identity Hash + Checksums；临时路径不进入 Gate |

## 13. Definition of Done

以下 `[x]` 只表示当前工作区已有直接代码与本地 Contract 证据；它不替代真实 Provider、Clean Commit、新镜像、
Pod Restart、Published Work 和 Sandbox/PVC 终态证据。整体状态只有在全部条目完成后才能改为 `completed`。

- [ ] 所有 Gate 样本具备校验通过的 `RuntimeEvidenceBundle@1`，临时 Run ID 不单独作为证据；
- [x] `RunModelUsage@1` 兼容且所有可重放 Bundle 重算一致；
- [ ] run-8 由同 Prompt 的完整可重放 Website Bundle 替代后才进入自动回归集合；
- [x] `ArtifactRouteManifest@1` 是 Preview、Validator、Published Presenter 的唯一 Route Oracle；
- [x] Fumadocs p7、p6 兼容 Artifact 的 `/docs` 与 `/docs/` 本地路由矩阵通过；
- [x] Node/Rust/Published Presenter 读取同一 Route Conformance Corpus；
- [x] Ambiguous Route、Manifest 外文件、路径穿越和目录列表均失败关闭；
- [x] Browser Validation 前 Entry Route Probe 可验证 HTML 与 Manifest Identity；
- [x] Serving/Artifact Failure 不调用模型、不要求 Source Mutation；
- [x] Source Failure 使用同时满足 ≤16 KiB、≤4,000 estimated tokens 的 Repair Context，且 Repair Cycle 有界；
- [x] Observation 与 Substantive Progress 分离，诊断读取不能无限续命；
- [x] 重复 Build ID、Candidate Version 和 Workflow Stage 振荡不能制造实质进展；
- [x] `RunPromptEfficiency@1` 和 Operation Usage 的 Runtime/HTTP/Web 本地路径完成；
- [x] Gross、Cached、Uncached、Per-turn Prompt 四类事实可观测；
- [x] 每个新 Run 可通过只读鉴权 API 获取创建时冻结的完整 `RunBudgetProfile@1`；历史缺失不伪造；
- [x] 终态 Bundle 冻结场景内全部 Run 的 Budget Profile，Rust/Node Canonical Hash Golden Vector 一致；
- [x] Replay 从事件逐 Turn 重算 Usage，并独立验证 Per-turn、Run、Operation 上限与批准的 Release Policy；
- [x] Repair 的 Setup/Review/Repair 三个 Run 均进入预算回放，不只校验最终 Repair Run；
- [x] Split Budget Shadow 与 Enforced 均有回归；
- [x] Dynamic Workflow Progress 位于尾部 Ephemeral Message，不再改变 Static Prefix；
- [ ] Provider Cache Smoke 与 Release Provider 的 Commit、Model Resource、Revision、Config Digest 身份完全一致；
- [x] Tool Set 稳定排序并以完整 Definition（Schema/Policy/MCP Identity）计算 Hash；
- [x] 大 Tool Exchange 在执行成功后可确定性成对 Microcompact；
- [x] Full Compaction 同时按 Token、字节、下一轮完整请求和消息数触发，并记录版本化触发原因；
- [x] Restart Fixture 中 Projection Hash 与预算恢复一致；
- [x] Runtime Workflow Driver 的确定性生命周期动作不再消耗模型轮次；
- [x] Partial Continuation 本地路径从不可变 Source Snapshot 恢复而非重做已验证源码；
- [x] 自动 Continuation 最多一次并受 Operation Budget 约束；
- [ ] Greenfield 和亮色 Edit 达到第 10 节 Token 门禁；
- [x] Benchmark 样本不足时显示 `insufficient_sample`，不宣称 P50/P95 通过；
- [x] Benchmark Attempt 由已验证 Paired Cohort Hash-chain 原子导入；Accepted 缺指标时零写入失败关闭；
- [x] Release Aggregate/Final Validator 重读原始 Paired Ledger + Mapping，并拒绝结构合法的手写 Benchmark Attempt；
- [ ] Required Fidelity、Acceptance、DraftSnapshot、PublishWorkflow 全部通过；
- [ ] Sandbox/PVC terminal cleanup 通过；
- [ ] Release 终态 Smoke Evidence 冻结；
- [x] 没有通过提高 Legacy 200k 上限掩盖重复上下文；
- [x] 重型 Runtime HTTP 测试统一接入有界 8 MiB Test Runner；标准 `cargo test` 无需全局栈变量即可通过；
- [ ] 没有日志、API、Evidence 或 UI 泄露 Provider 凭证、Prompt 或完整源码。

## 14. 最终建议

实施顺序必须是：

```text
先统一 Docs Artifact Entry Route 和 Preview Resolver
→ 再按责任层分类 Validation Failure
→ 限制 Repair Context 与无实质进展循环
→ 让 Token 消耗可解释
→ 再拆分错误预算口径
→ 稳定 Prompt 前缀
→ 压缩大 Tool History
→ 减少确定性模型轮次
→ 最后启用受控续跑和硬预算
```

如果只能先做一个提交，优先完成 Commit 0A：先建立唯一 Route Oracle、可重放证据和跨实现 Corpus；没有
可靠 Oracle 时直接改 Resolver 只会把 Harness 假阳性换成另一种假阳性。随后依次完成 0B～0D，阻止平台
错误继续消耗模型轮次，再实施 Stable Prompt Prefix 与 Tool Exchange Microcompact。如果只提高
`RUNTIME_AGENT_MAX_INPUT_TOKENS` 或 `RUNTIME_AGENT_MAX_TURNS`，Run 可能不再 Partial，但 Token、时长和
用户成本都会继续增长，不构成修复。
