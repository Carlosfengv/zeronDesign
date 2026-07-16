---
title: Website Design Context Package 实施方案
status: implementation-in-progress
last_updated: 2026-07-16
evidence_level: provider-verified
functional_decision: go
release_decision: no-go-for-enforcement-rollout
scope: zeronDesign Runtime Website（Astro）
created: 2026-07-14
depends_on:
  - ./2026-07-12-design-profile-lifecycle-closure-spec.md
  - ./2026-07-10-design-profile-fidelity-remediation-plan.md
  - ./2026-07-08-runtime-api-freeze.md
---

# Website Design Context Package（DCP）实施方案

## 0. 评审与执行摘要

> **评审结论（2026-07-16 更新）：** `a0d036f3d8ad` 仍是最新的严格 Provider release RC：`releaseEligible=true` / `result=pass`，证据等级保持 `provider-verified`。在其后新增 Batch E durable-store metrics、签名 paging、ledger/rollback 以及 registry EOF fallback 修复的组合字节，已形成新的 detached clean 实施候选 `1c0cd85b64db`，并在 fresh k3d 集群 `zd-dcp-canary-rc-0716-1005` 完成 fixture **audit RC**；部署 `/version`、Runtime image 与 evidence 均绑定 `1c0cd85b64db`，metrics 管理路由已实测 `401/200` 鉴权与固定 operational-export schema。该次证据使用 fixture provider，按设计为 `releaseEligible=false` / `result=fail`，只证明最新实现可洁净构建、部署和运行，不能覆盖或降级上一份 Provider release RC，也不能冒充 canary。发布判定仍为 **No-Go（仅指 enforcement rollout）**；当前不可替代的剩余门禁是生产部署与真实值班目的地验收、单项目 7 天且不少于 30 次 publish、指标结论页及精确 `enabled=false` 回退演练。两个 clean SHA 都是由组合工作区生成的 detached 临时候选，尚未提交到 `main`，不能表述为正式分支发布物。
>
> **主功能验收（2026-07-16）：Functional Go。** 本轮按“保证主要功能通路正常可用”的范围复跑 DCP compiler/verifier `8/8`、required-read/fidelity gate `2/2`、Profile Sync 内核 `10/10`、Runtime HTTP Profile Sync/recovery `2/2`、Web typecheck、BFF contract smoke、Website DCP Build/Edit/Repair 三条独立生命周期，以及 headless browser → Next BFF → 真实 Runtime Router 的联合 L4，全部通过。7 天/30 publish 不属于本次主功能验收，不再做本地时间窗稳定性模拟；它只影响后续生产 enforcement rollout 的 `canary-verified` 等级，不影响当前 Build/Edit/Repair、Profile Sync 和 BFF/Runtime 联调交付。
>
> **交付原则：** 代码、单测、fixture、部署 RC、真实 Provider 和 canary 是六个不同证据层级。低层证据不得替代高层门禁；失败审计也是正式证据，必须进入阻塞台账而不是被成功用例覆盖。

### 0.1 当前证据、未证实范围与发布判定

为避免把“代码已写入工作区”误读为“可以开启生产 enforcement”，当前事实按证据来源拆分如下：

| 证据面 | 当前结论 | 证据 / 限制 |
|---|---|---|
| 本地 Runtime / HTTP | **通过，`verified-fixture`** | `run-runtime-harness-local-gates.sh` 已于最终组合工作区完整通过：remote-fs/architecture/tool-contract、sandbox tools、permission、agent loop、HTTP、template build、shared package、Fumadocs real build、脚本/validator 与 computed-style smoke 均为 green；sandbox tools 为 `93 passed / 0 failed / 1 ignored`，HTTP 全量为 `110 passed / 0 failed / 3 ignored`。新增 `batched_fs_reads_preserve_the_target_patch_lease` 回归证明带 required-read 追踪副作用的 `fs.read` 不再作为 concurrency-safe 工具并行执行；新增 Canary metrics HTTP 回归证明聚合只接受冻结 cohort/policy revision 和白名单低基数指标。ignored 项由同次严格 RC 中的真实 Provider/部署验证补充，但本地 fixture 本身仍不证明真实模型遵从性。 |
| Web BFF | **通过，`verified-fixture`（控制面 + 联合浏览器交互）** | typecheck、production build、BFF smoke，以及真实 Next + 真实 Runtime Router + headless Chrome 的同次 L4 已通过；联合 fixture 覆盖 DCP READY/materialized/style verified、失败 fidelity rule/recipe/element/viewport/current-target/repair context、Profile 漂移入口、三方 diff、缺失冲突决议拦截、显式 apply-target、confirm 后 child Run DCP ownership/alignment、真实 token 写回、唯一 child Run 与最终截图 SHA-256。新 child Run 在 Agent 验证前允许 `styleContract.verified=null`，shared/BFF 已按三态契约接受并显示 pending。仍缺部署 canary 中的同链路记录、image/config identity 与观察窗。 |
| k3d Runtime/Sandbox observe | **通过，`verified-fixture`** | `services/runtime/target/e2e-evidence/zerondesign-dcp-rc/` 记录 Website Build→Edit 的 DCP materialization、全部 required reads、同一 content hash、`preview.publish`、artifact 与截图 SHA-256。证据为 commit `e0c3debafa96`、dirty files `76`、fixture provider；不能作为 clean release、真实 Provider 或 enforced RC 证据。 |
| 部署态 enforced RC | **通过，`provider-verified`** | clean detached 候选 `a0d036f3d8ad`、fresh cluster `zd-dcp-provider-rc-0715-2312` 的 release 模式 RC 已通过。同候选 provider DCP lifecycle 为 Build `run-719` → Edit `run-861` → Review `run-1025` → Repair `run-1040`，`finding-1031` 为 blocking/repairable/`fixed`；Build/Edit/Repair 均为 gate ready、materialization ready、fidelity passed、missing required reads 为空。Build/Edit/Repair mutation 由真实 Provider 执行，Review 为显式标注的 fixture-seeded finding。8 个恢复场景全 pass、orphan count 为 `0`。 |
| 供应链与网络隔离 | **同次 release RC 通过** | `runtime-rc-preflight@1` 以 `prefetchImages=true` 校验 11 个锁定镜像：全部 `lockedDigestVerified=true`、`mutableTagMatchesLock=true`、`pulled=true`；lock hash 为 `c9fd0b390bdc915f04a1f1742df6eedf4c243c3069b1da223c35e97956504914`。Debian/Node 遇到 Docker Hub 429 时仅切换到批准镜像源做同 digest inspect，最终 pull 仍为 canonical；运行态同时证明 `directRegistryDenied=true`、`npmProxyInstallPassed=true`。 |
| 真实 Provider / canary | **同次 RC 达到 `provider-verified`；canary 待执行** | 经批准的 `deepseek-chat` 在 clean RC 中完成普通 Website Build `run-1380` / Edit `run-1485`、Docs Build `run-1593` / Edit `run-1690`，以及 enforced DCP Website Build `run-719` / Edit `run-861` / Repair `run-1040`。Website `/`、Docs `/docs/` 与 provider DCP `/` 的内容和 computed-style 均通过，terminal tool failure 为 `0`，Sandbox 释放后使用 fresh project principal 读取 artifact 均返回 `200`。provider DCP 的 Review `run-1025` 为 fixture-seeded，`finding-1031` 已由真实 Repair 置为 `fixed`。canary 观察窗、指标结论页和精确 policy 回退演练仍未执行。凭证未写入仓库、命令行或证据包。 |

**当前功能判定：Functional Go；当前发布判定：No-Go（仅限 rollout/enforcement）**。Build/Edit/Repair、Profile Sync、Web BFF/Runtime 主通路已达到可联调、可演示、可继续集成的交付状态；本次不以 7 天时间窗作为主功能验收项。共享环境仍须保持 `RUNTIME_DESIGN_CONTEXT_ENFORCEMENT_V1=false`；若没有可健康探测的 browser worker，effective enforced Run 必须 fail closed，禁止回退到临时宿主机浏览器。只有计划扩大生产 enforcement allowlist 时，才重新启动第 15 节 canary/rollback 运营门禁。

最新权威机器证据位于 `/tmp/zerondesign-dcp-provider-rc-a0d036f3d8ad/services/runtime/target/e2e-evidence/zd-dcp-provider-rc-0715-2312/release-evidence.json`；其 `schemaVersion=release-evidence@1`、`releaseEligible=true`、`result=pass`，并已独立通过 `validate-release-evidence.mjs`。该路径是本机临时 clean worktree 的审计路径，不是仓库内长期归档地址；提交前应复制到受控、不可变且不含凭证的 release evidence 存储，并记录最终分支 commit。历史失败证据继续以 `services/runtime/target/e2e-evidence/zerondesign-dcp-rc/`、`zerondesign-dcp-network-20260715/` 和 `zerondesign-dcp-repair-20260715/` 为入口，用于证明缺口发现与修复过程；其中旧 `releaseEligible=false` 不能覆盖本次 clean RC，也不得删除。

| 最新 clean release RC 锚点 | 值 |
|---|---|
| Repository / cluster | detached clean commit `a0d036f3d8ad`（完整对象 `a0d036f3d8ad9a16343260bd6861ff736b04111b`）；cluster `zd-dcp-provider-rc-0715-2312`；node UID `4206d193-7cd0-48bd-9629-4cdffde7ec19` |
| Runtime image | `anydesign/runtime:a0d036f3d8ad`；manifest `sha256:7e378a4e05f10cafe774aa1ca0f692700be9c5a0bb5553569e592e048af28ef7`；config `sha256:6afb244319956aba0263c73993a74cb4c35cff5a56b68e91691adad7ea6785a7`；reported commit `a0d036f3d8ad` |
| preflight / egress | `runtime-rc-preflight@1`，11/11 locked digest verified、tag matched、pulled；`prefetchImages=true`；`directRegistryDenied=true`；`npmProxyInstallPassed=true` |
| enforced DCP Provider | project `rc-website-dcp-enforced-dcp-provider-1784128564`；Build/Edit/Review/Repair `run-719/run-861/run-1025/run-1040`；`finding-1031=fixed`；Build/Edit/Repair 为真实 Provider，Review 为 fixture-seeded；三阶段 gate/materialization/fidelity/read 均通过 |
| 真实 Provider Website | project `rc-website-1784128564-real`；Build/Edit `run-1380/run-1485`；artifact `/` 内容与 computed-style pass；释放后 project-principal HTTP `200` |
| 真实 Provider Docs | project `rc-docs-1784128564-real`；Build/Edit `run-1593/run-1690`；artifact `/docs/` 内容与 computed-style pass；释放后 project-principal HTTP `200` |
| 恢复、安全与传输 | 8/8 recovery scenarios pass、orphan count `0`；project ownership/JWT/mTLS/rotation window verified；secret scan matches `[]` |
| 最终 release 结论 | `releaseEligible=true` / `result=pass` / `provider-verified`；enforcement rollout 仍为 No-Go，原因仅剩批次 E canary/rollback |

最新 Batch E 实施字节另有一份**非发布** audit 锚点；它用于证明新增代码已进入 clean image 和 fresh cluster，不改变上表的 Provider release 权威性：

| 最新 clean audit RC 锚点 | 值 |
|---|---|
| Repository / cluster | detached clean commit `1c0cd85b64db`（完整对象 `1c0cd85b64db6008dd5a746b544d90786a73e192`）；cluster `zd-dcp-canary-rc-0716-1005`；node UID `a7fccb4d-abef-4ea5-8587-39531cad5e6b` |
| Runtime image | `anydesign/runtime:1c0cd85b64db`；manifest `sha256:09e2e1f0f8e7e3b877c70fa03b1f96d60390f8d5be727c47c5da19dfe46df5ac`；config `sha256:29ddf625bed89c75da71d587608d89916b9a75f70138c68b4725e2b6044589d0`；`/version.repositoryCommit=1c0cd85b64db`、`repositoryDirty=false` |
| preflight / egress | audit preflight `prefetchImages=false`；11/11 locked digest verified、tag matched；lock hash `c9fd0b390bdc915f04a1f1742df6eedf4c243c3069b1da223c35e97956504914`；运行态 `directRegistryDenied=true`、`npmProxyInstallPassed=true` |
| enforced DCP fixture | project `rc-website-dcp-enforced-1784167663`；Build/Edit/Review/Repair `run-411/run-505/run-581/run-596`；`finding-587=fixed`；Build/Edit/Repair gate/materialization/fidelity/read 均通过；8/8 recovery scenario pass、orphan count `0` |
| metrics route | 同 project/Profile revision 的完整查询：缺内部授权为 `401`；内部管理员授权为 `200`，schema 为 `design-context-canary-operational-export@1`，固定六类 alert 均为 `triggered=false`；本次 `sampleCount=0`，只能证明部署和契约 |
| audit 结论 | fixture provider、无 approval/credential；按 fail-closed release 规则为 `releaseEligible=false` / `result=fail`。这是正确审计结果，不得改写为 release failure 回归或 canary pass |

该 audit RC 机器证据位于 `/tmp/zerondesign-dcp-canary-1c0cd85b64db/services/runtime/target/e2e-evidence/zd-dcp-canary-rc-0716-1005/`；独立预检证据位于同 worktree 的 `services/runtime/target/e2e-evidence/zd-dcp-canary-preflight-1c0cd85/preflight.json`。两者都是本机临时路径，正式提交/部署前仍须迁移到不可变 evidence store。

### 0.2 实施基线与签收口径

本文同时是架构方案和实施清单。为避免把“接口已写出”误判为“功能可发布”，后续每一项只使用下列状态；PR 描述、测试报告和灰度申请也必须沿用同一口径。

| 状态 | 含义 | 可作为发布依据？ |
|---|---|---|
| `implemented` | 代码与契约已落地，至少有单元/契约测试 | 否；仍可能缺真实生命周期或故障路径证据 |
| `verified-fixture` | 在隔离 Runtime/Website fixture 上完成可重复的端到端验证 | 仅可进入部署 RC |
| `deployed-rc-verified` | clean image 在目标 Runtime/Sandbox/browser-worker 形态通过 enforced 生命周期与 restart gate | 仅可进入真实 Provider 验证 |
| `provider-verified` | 经批准真实 Provider 完成 DCP required reads、修改、修复与 publish | 仅可申请单项目 canary |
| `canary-verified` | 在受控真实环境按本方案阈值完成观察窗并完成一次回退演练 | 可以申请扩大 allowlist |
| `release-ready` | 所有 DoD、兼容回归和证据包齐全 | 可以讨论默认策略变更 |

| 工作流能力 | 当前状态 | 已有证据 | 关闭条件 / 不应误读为 |
|---|---|---|---|
| DCP 编译、冻结、materialization、read gate | `provider-verified` | Rust lib/HTTP、真实 workspace/Node build、small/large source、restart 与冻结 identity 回归均通过；同一 clean release candidate 的真实 Provider enforced Build/Edit/Repair 同时证明 required reads、materialization、style/fidelity、finding fixed、preflight、egress、auth 与 recovery | Provider 技术门禁已关闭；下一层仅接受真实 canary 观察窗与回退证据 |
| Prompt assembler、source 语义索引、只读 DCP 诊断 | `provider-verified` | section 组装、source 权限、Runtime HTTP、BFF smoke 与 clean RC required-read 证据；manifest/diagnostics 与 `design_context.status` 共用冻结身份校验，HTTP 已覆盖 401/403/200、历史快照读取和敏感字段不回传；非法 project scope 在 attach 前拒绝，遗留/持久化 identity 漂移由共享 validator fail closed；同候选真实 Provider 已使用同一工具面完成 Website Build/Edit/Repair required reads | Provider 身份链路已关闭；不代表 canary、运营告警或模型自主 Review 已通过 |
| Profile token sync | `verified-fixture` | 三方 merge、CAS/幂等状态机、RunLifecycle apply/recovery 与 HTTP 回归；BFF→真实 Router 已覆盖无冲突写入回读、冲突决议/缺失决议拒绝与重复 confirm；写入后、完成前的 restart recovery 已回归为同一 child Run 的 `Applied` operation | 仍需真实环境运行证据 |
| a11y / responsive verifier 与 fidelity 裁决 | `provider-verified` | Runtime 镜像内固定 Chromium + Node collector、真实 headless launch、固定 viewport、unavailable/lost fail-closed、required failure repair 与新 candidate promotion均已覆盖；同候选 Provider DCP Build/Edit/Repair 均为 fidelity passed | Provider 门禁已关闭；canary 观察窗仍是更高层门禁 |
| enforcement 控制面 | `implemented` | 双 flag、精确 allowlist、internal-admin CAS、重启恢复、显式 disable 覆盖 | 尚未做共享环境 canary；不得提前把默认 enforcement 打开 |
| 指标、告警、灰度与回滚演练 | `implemented`（Runtime durable-store 聚合 + 十类事件出口；最新 clean audit RC 已部署验证） | compiler、read gate、capability gap、fidelity outcome、recipe/rule failure、source section、required a11y/responsive、Profile Sync、verifier unavailable/runtime-lost 均已发出结构化 `MetricRecorded`；internal-admin metrics export 按 frozen policy cohort 从持久化 Run/event/operation 重算 publish 样本、六项阈值和停止/回退告警，采集器生成不可覆盖的来源 JSON、ledger fragment 与一页报告；签名 HTTPS dispatcher 对触发告警 fail closed 投递，并将每份 snapshot 的 delivered/not-required 决策写入 hash-chain。候选 `1c0cd85b64db` 已在 fresh k3d 中验证 `/version` identity、无鉴权 `401`、有鉴权 `200` 与 `design-context-canary-operational-export@1`；该审计查询样本数为 `0`，不构成指标结论 | 生产部署、真实值班目的地绑定/投递验收、7 天观察窗、至少 30 次真实 enforced publish 与精确 policy 回退演练仍未完成；在此之前保持 **No-Go**，不能以 route `200`、零样本 export 或 audit RC 替代运行证据 |

**实施纪律：** 每次状态升级都必须在第 15.1 节证据包中追加对应 run/operation id、hash、测试命令和结论。没有证据包的“已完成”一律按 `implemented` 处理；任何失败都先降级状态，再决定是否修复或扩大范围。

### 0.3 本轮签收看板（评审入口）

下表是本文件的唯一进度入口。正文描述机制和约束；状态升级、排期和灰度申请只以本表的“下一份不可替代证据”为准。

| 交付切面 | 当前等级 | 已成立的边界 | 下一份不可替代证据 | 通过后的结论 |
|---|---|---|---|---|
| DCP 生命周期 | `provider-verified` | clean image 的真实 Provider enforced Build→Edit→Repair、fixture-seeded Review、finding fixed、DCP lineage、publish、项目 Pod egress、preflight 与 8 类 recovery 已进入同一机器证据并通过 fail-closed validator | 单项目 7 天且不少于 30 次 publish 的 canary 结论页和精确回退证据 | 可升级为 `canary-verified` |
| enforced 验证 | `provider-verified` | Chromium/Node collector、a11y/375/768/1440、真实 Provider Build/Edit/Repair style/fidelity、依赖、restart、worker-lost fail-closed、供应链/egress、授权 artifact read 与 secret scan 均在同一 clean RC 中通过 | 同一精确 cohort 的真实 canary 阈值与 `enabled=false` 回退演练 | 可升级为 `canary-verified` |
| Profile Sync Runtime | `verified-fixture` | Runtime HTTP 与 BFF→真实 Router 已覆盖无冲突 token 写入回读、逐项冲突决议/缺失决议拒绝、格式错误与有效参数失配、重复 confirm 的 child Run 唯一性，以及写入后完成前中断的 Store restart recovery | 真实环境的受控 Profile Sync run：保留 operation id、plan hash、before/after hash、Runtime/BFF image digest，并验证审计事件与失败处置 | Runtime 控制面可与 BFF 在同一 canary 证据包中联合签收 |
| Web BFF | `verified-fixture`（控制面 + 联合浏览器交互） | ownership、服务端派生 source/target hash、请求收口均有 typecheck/mock smoke；真实浏览器已在同一次 L4 中连接真实 Runtime Router，完成 DCP/fidelity 状态读取、失败规则与 repair context 展示、冲突 plan/guard/决议/confirm、child DCP 回读、token 写入、child Run 唯一性与截图 digest | 部署 canary 中保留同链路的 operation/plan/token/image identity、截图 digest 和观察窗 | Web 控制面可与 Runtime 一起签收 |
| 真实 Provider Website/Docs | `provider-verified` | clean RC 已由经批准的 `deepseek-chat` 完成 Website/Docs Build/Edit，并在同一候选完成 enforced DCP Build/Edit/Repair、fixture-seeded Review、finding fixed、真实 source mutation、新 candidate、正确路由 artifact 与 computed-style；image/config/DCP identity 已关联 | 单项目 canary 观察窗与回退；不得把 fixture-seeded Review 写成模型自主 Review | 可申请 canary，完成后升级 `canary-verified` |
| rollout | `planned` | exact allowlist、CAS policy、显式 disable 回退和审计已经具备；最新 clean audit RC 已证明 Batch E Runtime route/identity 进入实际镜像，但未绑定真实 webhook，也没有 canary 样本 | 正式分支构建和生产部署、真实值班目的地 readiness probe、单一真实项目的 7 天/30 publish 指标观察窗与 `enabled=false` 回退演练 | 可升级为 `canary-verified` |

**不可替代性规则：** mock Runtime 只能证明 BFF 的参数收口和越权处理；它永远不能关闭“token 已写入、child Run 已创建、重复确认不重复执行”的 Runtime 联调缺口。反过来，Runtime HTTP fixture 也不能关闭 BFF 的 ownership、principal 签名和浏览器输入收口缺口。

真实模型门禁入口 `real_provider_public_runtime_website_and_docs_lifecycle_matrix` 的 Website leg 已于 2026-07-15 执行通过。它显式开启 DCP master/enforcement、为本次 Profile revision 配置精确 allowlist，并在 Build、Edit、Repair 三个 mutation phase 校验 required reads。Edit 完成后，harness 以 `harness-seeded-review` 创建与当前 candidate 绑定的 blocking Review finding，随后由真实 Provider 执行 Repair；只有新 version 被 promotion、finding 变为 `fixed`、三阶段 DCP content hash 一致且修复后的 artifact 包含精确目标文本才通过。Review finding 由审计器注入是为了把“Repair 遵从性”与模型主动发现问题的随机性分离，不能表述为真实 Provider 已自主完成设计评审。复跑仍必须同时提供经批准的 Provider 凭证、approval reference、可健康启动的 Chromium/Node collector、网络与 npm registry：

`RUNTIME_PROVIDER_APPROVAL_ID` 不是 DeepSeek 下发的标识，也不是 API key。它是调用方为“允许本次测试使用真实付费 Provider”登记的可追溯授权引用，例如变更单号、审批工单、任务 ID 或安全评审记录。该值可以进入 evidence；不得把 API key、Bearer token、JWT 或其他凭证填入此字段。若组织没有审批系统，应先由负责人形成一条带日期、范围、模型和批准人的内部记录，再将其不可变引用作为该值；临时自造且无法回查的字符串不能作为生产 canary 的授权证据。

在个人/开发门禁中，可把当前 Codex 任务 ID 与用户明确授权日期组成可回查引用，例如 `codex-thread-<thread-id>-user-approved-<yyyymmdd>`；它只证明“本次开发测试获准使用付费 Provider”，不得复用于生产 canary。生产 canary 必须改用组织内真实审批单、变更单或发布单引用，并在 session 记录批准人、范围、模型与有效期。调用脚本时，API key 仅通过隐藏交互或当前进程环境注入；approval reference 作为普通审计元数据传入，两者不得互换。

```bash
RUNTIME_PROVIDER_APPROVAL_ID=<approved-reference> \
REAL_PROVIDER_PROJECT_FILTER=website \
cargo test --manifest-path services/runtime/Cargo.toml --test http_api \
  real_provider_public_runtime_website_and_docs_lifecycle_matrix \
  -- --ignored --nocapture
```

推荐通过 `services/runtime/scripts/run-real-provider-http-lifecycle-e2e.sh` 执行，以同时产出 provider log、computed-style 结果与 `evidence-summary.json`。汇总器会校验 Website Build/Edit/Repair 的 stream、artifact/promotion、DCP required reads、相同 content hash、Review/Repair/finding lineage、`fixed` 状态，以及每条 evidence 上的 provider/model/approval reference；缺 Repair、未 fixed、approval 不匹配或 Repair 仍为 observe 均 fail closed。历史通过记录位于 `.runtime-evidence/provider-website-20260715-204708/`；API key 通过隐藏交互输入注入，未写入仓库、命令行或证据包。该历史结果继续证明本地工具面；最新同候选 clean RC 已把真实 Provider DCP Repair 与部署 image/config identity 合并，批次 D 因而关闭。

### 0.4 当前阻塞台账

阻塞项按依赖顺序关闭。任何一项不得通过降低门禁、清理持久化证据或改写证据口径绕过。

已关闭但必须保留历史证据的 P0：stale project-init transaction journal 不再阻断 Runtime 启动。启动恢复现在按 backend 逐条隔离失败 Run，写入 `project.init.startup_recovery` 审计、`recovery_required`、terminal event 与可操作 metadata，并保留原 journal；新增单测证明坏 journal 与健康 Run 可同时恢复，随后同一 k3d 持久卷上的 Runtime rollout 已成功跨过历史坏 journal。该结论只关闭“启动 CrashLoop”，不替代供应链或 clean RC。

| 优先级 | 阻塞项 | 关闭条件 | 需要保留的证据 |
|---|---|---|---|
| 已关闭 | 部署态 enforced RC | clean image 已在不跳过 preflight/egress 的 release 模式完成 Runtime + k3d Sandbox + browser worker 的 enforced Build/Edit/Review/Repair、restart 与机器校验 | commit/image manifest/config、DCP/verifier hash、Build/Edit/Review/Repair/finding ids、artifact/evidence URI、validator 输出均在 `release-evidence@1` |
| 已关闭 | 同候选真实 Provider DCP 遵从性 | clean candidate `a0d036f3d8ad` 已完成真实 Provider enforced DCP Build/Edit/Repair、fixture-seeded Review 与 finding fixed；release validator 已绑定 image/config/DCP identity | approval reference、provider/model、run ids、image/config、DCP/read/publish/computed-style 与 review provenance 均在 `release-evidence@1` |
| P1 | canary 与回退未执行 | 单一精确三元组完成 7 天且不少于 30 次 publish 的观察窗，并成功写入 `enabled=false` 回退 | 指标结论页、policy revision、回退前后新 Run mode、告警与审计保留 |

#### 0.4.1 批次 B review 记录（供应链与 egress）

本记录是本次 lock 更新的 review reference，不等于 release approval。变更只接受 OCI identity 一致的来源；镜像加速源仅用于在 Docker Hub 429/EOF 时填充本地缓存，返回 digest 必须与 review 后的 Docker Hub/官方索引完全相同，lock 中仍保留 canonical ref。

| 镜像职责 | 旧 digest | review 后 digest | 评审结论 |
|---|---|---|---|
| Runtime Debian | `sha256:60eac759…` | `sha256:7b140f374…` | 官方 `bookworm-slim` 多架构 OCI index；amd64/arm64 与 attestation manifest 均存在，允许刷新 |
| Runtime/Sandbox/fixture Node | `sha256:a25c9934…` | `sha256:5647be70…` | 官方 `22-bookworm` 多架构 OCI index。首次 review 刷新为 `a84054eb…`；随后上游在相同 official-images revision `c517c39b…` 下补齐 ppc64le，amd64/arm64 manifest 保持 `175215…` / `63c733…`，多架构 index 因此变为 `5647be70…`。已同步更新 lock、Runtime Node stage、Sandbox 默认 base、fixture 声明与安全测试 |
| local-path PVC helper | 未纳入 lock | `sha256:8a45424d…` | K3s `local-path-provisioner` 的隐式 `mirrored-library-busybox:1.36.1` 依赖；新增 lock，并在任何 Sandbox PVC 创建前把 `helperPod.yaml` 改为 `ref@digest` |

preflight 现在分成两个明确档位：audit 使用 `PREFLIGHT_PREFETCH_IMAGES=0`，逐项验证 locked `ref@digest` 可解析且当前 tag 未再次漂移；release 使用 `PREFLIGHT_PREFETCH_IMAGES=1`，在前述检查基础上预取全部锁定镜像。脚本会聚合所有 registry/identity/drift 错误后统一失败，不再由首个漂移掩盖后续项。2026-07-15 在 Node index 更新为 `5647be70…` 后，完整预取曾遭遇 Docker Hub 匿名 429/EOF；现已增加批准镜像源的 exact-digest fallback，fallback 只在 canonical rate-limit/network failure 时用于 inspect，必须返回同一 locked digest，否则 fail closed，实际 pull 仍优先 canonical。2026-07-16 的 audit 复跑又证明 registry 客户端可能只返回 plain `EOF`；分类器现显式覆盖单词边界 `EOF`、connection reset、TLS handshake timeout、`ETIMEDOUT` 与 rate-limit，同时拒绝对 manifest unknown、digest mismatch、denied 等身份错误使用 fallback，并由独立 Node policy test 冻结语义。最终 Provider clean RC 的 `runtime-rc-preflight@1` 记录 11/11 entries 全部 verified/matched/pulled，Debian/Node 的 `inspectSource=approved-fallback`、`fallbackReasons=[rate_limited]` 与 `pullSource=canonical` 均可审计；最新 `1c0cd85b64db` audit preflight 也为 11/11 verified/matched，因此实现与文档的 EOF 语义已重新一致。

Runtime 镜像构建另暴露了基础设施可移植性问题：原 Dockerfile 硬编码清华 Debian mirror，该端点在本次环境 TLS 握手失败，而 `deb.debian.org` 与 `security.debian.org` 均可达。现已将两者改为可覆盖的 build args，默认使用 Debian 官方 HTTPS 源；clean release build 已记录最终 Runtime manifest/config digest，后续正式构建仍须保存实际 build args。

fixture gateway 不再要求集群额外拉取同一 Node 基础镜像：RC 部署后将 gateway image 显式设置为本次 Runtime candidate image，复用候选镜像内已锁定的 Node runtime，并把 gateway 与被测 Runtime 绑定到同一 image identity。该设计减少 Docker Hub 二次拉取和版本分叉，但 fixture provider 身份不因此变成真实 Provider，reused image 也仍不具备发布资格。

网络根因不是 default-deny YAML 缺失，而是 `SandboxTemplate` 默认 `networkPolicyManagement: Managed` 会由 controller 生成额外 public-internet allow policy；Kubernetes NetworkPolicy 取允许规则并集，最终绕过仓库 egress 约束。Website/Docs 模板现均显式使用 `Unmanaged`，只保留仓库的 default-deny、Runtime ingress、DNS egress、npm-proxy egress 四条策略。网络断言移动到项目 Pod 尚被 Runtime 绑定的 dependency evidence 阶段，避免 release 后 warm-pool 回收造成 UID 竞态；`npm-proxy-gate@2` 再校验 Website/Docs 各自的 Pod UID/IP、lockfile hash、tarball request、DNS/proxy 和 `directNpmjsDenied=true`。

本轮可复核命令与证据：

```bash
PREFLIGHT_PREFETCH_IMAGES=0 bash infra/agent-sandbox/preflight-runtime-rc.sh
cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_security
ANYDESIGN_E2E_CLUSTER=zerondesign-dcp-network-20260715 \
  SANDBOX_BASE_IMAGE='docker.io/library/node:22-bookworm@sha256:a25c9934ff6382cd4f08b6bc26c82bf4ea69b1e6f8dabfb2ead457374127c365' \
  bash infra/agent-sandbox/run-k8s-e2e.sh # 仅为网络机制审计复用旧 Sandbox base；不得计入新 lock release
```

历史机器证据：`zerondesign-dcp-network-20260715/npm-proxy.json` 为 `npm-proxy-gate@2` / pass；同目录旧 `release-evidence.json` 的 `releaseEligible=false` / `result=fail` 如实保留 dirty、fixture、reused image 与 skip preflight 限制。最新 clean RC 的 pass 证据以第 0.1 节路径为准，两者不可互相覆盖。

#### 0.4.2 Web / Runtime 联调 review 记录

本轮浏览器回归和 L4 fixture 先后暴露并关闭四个边界错误：BFF confirm 返回 child Run 后未登记 product-side ownership，导致 UI 切换后 events/design-context 自己返回 404；Profile Sync 又把冻结 style contract 的 `/workspace/project` 与 DCP manifest 的 `project` 做字面比较，导致真实编译产物无法 plan；plan 路由读取实际 contract 后仍使用读取前的 Run 快照，使漂移只能延迟到 confirm 才被拒绝；最后，真实 child Run 在 Agent 验证前合法返回 `styleContract.verified=null`，但 shared schema 只接受 boolean，使 BFF 把可读 diagnostics 错误映射为 400。修复后的不变量是：BFF 必须在返回 confirm 前幂等登记 child Run；Runtime 比较 appRoot 时只规范化 `/workspace/` 前缀，仍对 template、contract schema 和 token-to-CSS-variable mapping 做精确校验；contract/token 读取后必须重新获取 Run，让 `verified=false` 在 plan 阶段立即 fail closed；diagnostics 的 style-contract 状态必须保持 `null / false / true` 三态，UI 分别按 pending / failed / verified 语义消费。L4 seed 必须使用编译器产生的冻结 style contract 和 artifact manifest hash，并通过 Runtime `fs.read` 记录验证结果，不得手写弱化版本或预置验证结论。

2026-07-15 的最终本地回归结果：

| 验证 | 结果 | 允许证明的结论 |
|---|---|---|
| `cargo test ... bff_profile_sync_bff_to_real_runtime -- --ignored --nocapture` | pass | 真实 Next + 真实 Runtime Router + headless Chrome 的同次 clean/conflict/UI 决议、token 写回、confirm replay/mismatch、唯一 child Run 与截图 digest 成立 |
| `npm --prefix apps/web run typecheck` | pass | Web 类型契约成立 |
| `npm --prefix apps/web run test:design-context-bff` | pass | ownership、服务端派生同步参数和 child Run ownership 的 mock-Runtime BFF 边界成立 |
| `npm --prefix apps/web run build` | pass | Next production build 成立 |
| `node services/runtime/scripts/test-real-provider-evidence-summary.mjs` | pass | Provider evidence summary 对 Repair/finding/approval 的 fail-closed 语义成立；不代表真实 Provider 已运行 |
| `run-real-provider-http-lifecycle-e2e.sh`（`REAL_PROVIDER_PROJECT_FILTER=website`） | pass | 已批准真实 Provider 完成 Website Build/Edit/Review/Repair、required reads、三阶段同一 DCP hash、新 source snapshot/new version promotion、finding fixed 与 computed-style；只证明受控本地 Provider 门禁，不替代 clean deployed RC/canary |
| `node services/runtime/scripts/test-release-evidence-validator.mjs` | pass | release validator 的 fixture 判定语义成立；不代表 release evidence 已通过 |
| `cargo fmt --manifest-path services/runtime/Cargo.toml -- --check`、appRoot 定向 lib 与 HTTP drift 回归 | pass | Rust 格式、真实冻结 contract appRoot 兼容，以及漂移在 plan 阶段 fail closed 成立 |

#### 0.4.3 真实 Provider Website lifecycle review 记录

2026-07-15 使用已批准的 `deepseek/deepseek-chat` 在受控本地 Runtime 执行 Website-only lifecycle。最终证据目录为 `.runtime-evidence/provider-website-20260715-204708/`；`evidence-summary.json` 为 `ok=true`，required project 只有 `real-http-website`，computed-style 为 `ok=true`，malformed stream/evidence 与 unscoped event 均为 `0`。Build `run-8` 发布 `version-117`，Edit `run-143` 从该版本生成 `version-233`，审计器 Review `run-298` 创建 `finding-301`，Repair `run-303` 从 `version-233` 生成 `version-432` 并将 finding 置为 `fixed`。Edit/Repair 的 source snapshot 均发生变化，Build/Edit/Repair 的 DCP content hash 均为 `3e299323892ddfaa8dbb01d5f31379eabdd4768f3ad639aa544d16e15b21e5fb`；最终 artifact 的 `--runtime-primary` 计算值为 `#f97316`。

真实调用没有直接一次通过，而是依次暴露以下 Runtime 产品缺口。处理原则是修 Runtime 契约并增加回归，不通过放宽 harness 或提示词掩盖：

| 暴露项 | 根因 | 修复与防回归 |
|---|---|---|
| 长上下文 bootstrap 超过 `fs.write` 输入预算 | Runtime 自有 `state/context.md` 在真实对话压缩后超过直接工具的字符/序列化限制 | Runtime-owned workspace write 在预算内使用 `fs.write`，超限时以 7,000 字符分片执行 `fs.write_chunk + fs.commit_chunks` 原子覆盖；AgentLoop 回归强制生成大于 48,000 字符的上下文并断言无 `tool.input_too_large` |
| Repair 无法接收同一 bootstrap 能力 | frozen Repair profile 允许 `fs.write`，但未允许语义等价的 chunk write/commit | Repair policy 显式允许 `fs.write_chunk`、`fs.commit_chunks`，并在 policy 回归中冻结 |
| Repair 只有 finding ID，缺少可执行目标 | inherited parent history 淹没目标，模型无法只凭 ID 判断需要修改的内容 | Runtime 校验 finding/project/parent/base candidate/repairable 状态，只投影目标 finding 的 bounded summary；同时作为 system context 与最新 Runtime-generated user message 注入，明确 finding 文本不可信且不能修改 policy/tool/read boundary；测试证明非目标 finding 不进入上下文 |
| Repair 可用 `preview.report_candidate` 晋升旧 snapshot | manual report 路径允许新 version 继续引用 Edit 的旧 source snapshot | Repair 明确拒绝 `preview.report_candidate`，只允许经 `preview.publish` 构建、校验和晋升；policy deny/audit 回归覆盖 |
| `run.complete` 接受无实际修复的 Repair | completion 只看终态，没有强制新 version、当前 Run source snapshot 与 `preview.updated` 的一致性 | Build/Edit/Repair completion 均要求 output version；Repair 额外要求与 base 不同、由当前 Run 创建、source snapshot 非空且变化，并存在匹配的当前 Run `PreviewUpdated`；有 DCP 时继续要求 fidelity pass。回归覆盖缺 output、stale snapshot 失败与 fresh promoted snapshot 成功 |
| Website-only 汇总仍按 Website+Docs 要求证据 | runner 未把 project filter 传给 evidence summarizer | runner 根据 filter 传递精确 `--project`；本次 summary 的 requiredProjects 仅包含 `real-http-website` |

该记录证明真实 Provider 能遵从当前 Website DCP/Repair 契约，也证明 fail-closed gate 能拦截“看似 completed、实际未产生新源码”的 Repair。它不证明模型自主完成 Review，也不替代 clean image、目标部署网络/身份链路或 canary 观察窗。approval reference 可以进入证据，Provider API key 不得进入日志、仓库、环境快照或证据包。

#### 0.4.4 Canary evidence validator review 记录

`design-context-canary-evidence@1` 已从“字段存在性检查”收紧为与本方案退出门禁一致的交叉验证：观察窗必须由 ISO 起止时间证明至少 `10080` 分钟，且 enforced publish 不少于 `30`；observe/enforced 必须是同一 project/Profile revision cohort 的不同 Run/candidate，并分别绑定 persistent disabled `observePolicyRevision` 与后续 persistent enabled `policyRevision`。两者不允许伪造为同一 policy revision，因为 `disabled → enabled` 的 CAS 更新必然生成新 revision。每条 Run 必须列出 required/read 文件、artifact/materialization hash、worker、Runtime 冻结的 enforcement policy binding 与 evidence URI；Repair 必须有不同 blocked/review/repair Run、`fixed` finding 和新 promoted candidate；clean/conflict/recovery Profile Sync 必须使用不同 operation/child Run、产生真实 before/after 变化，并与 BFF 记录的 conflict operation 精确对应。指标还必须证明 verifier unavailable/runtime-lost、意外 read gate、超过 24 小时 recovery 债务均为零，required finding repair rate 为 100%，且 `enabled=false` 回退与四条兼容 Run 证据齐全。

本地 fixture 已增加短观察窗、重复 Website Run、重复 sync child Run、无 token 变化、BFF operation 不匹配和复用 candidate 等负例并通过。该结果只证明校验器会 fail closed，不代表真实 canary 已执行。

#### 0.4.5 完整本地 Runtime harness review 记录

2026-07-15 在当前组合工作区执行：

```bash
bash services/runtime/scripts/run-runtime-harness-local-gates.sh
```

命令最终执行到 `git diff whitespace` 并以 `0` 退出；补齐诊断工具行为、DCP 读取面安全与低基数指标回归后再次执行仍为相同结果。该 gate 是本地实现/契约层证据，不替代 clean image、k3d release RC、真实 Provider 或 canary。首次运行先后暴露并关闭三项问题，后续 review 继续关闭七项行为、安全、生命周期、可观测性与产品解释性证据缺口：

| 暴露项 | 根因 | 处理与防回归 |
|---|---|---|
| remote workspace filesystem boundary 失败 | browser evidence 的 `#[cfg(test)]` fixture 直接使用 host `std::fs`，但没有声明本地测试所有权边界 | 按仓库既有约定以具名 `browser-evidence-test-fixtures` boundary 包裹测试模块；生产路径未增加 host filesystem 例外；boundary checker 与两项真实 Chromium 测试通过 |
| sandbox tool contract baseline 失败 | `diagnostics.accessibility`、`preview.audit_responsive`、`design_context.status` 已注册，但旧 v1 baseline 未冻结其顺序与空对象 input schema | contract baseline 升为 v2，并纳入三项只读工具；tool order/schema、eager loading、block interrupt 与 permission 集成回归通过 |
| large-source lifecycle 失败 | Edit/Repair fixture 已读取 profile/usage/recipes/index/required source section，但漏读 `state/style-contract.json`，被 mutation/publish gate 正确拒绝 | 不放宽生产 gate；补充两阶段实际 style-contract read，并断言 `design_context_style_contract_verified=true` 后才能 patch/publish；Build→Edit→Repair 定向用例与 HTTP 全量通过 |
| 新诊断工具只有注册契约，缺行为级证据 | 未直接证明缺 report 时的错误类型、a11y/viewport finding 隔离、report 所有权和 `design_context.status` 的状态/并发语义 | 新增 executor 回归：缺 report 返回 `design_context.fidelity_report_missing`；两类诊断只返回对应 assertion 且不返回非目标私有字段；普通 `fs.write`/`fs.patch`/`fs.delete` 不能创建、修改或删除 Runtime-owned fidelity report；DCP status 从 `read_required` 收敛为 `ready` 并显示 style contract verified；三项只读工具均标记 concurrency-safe |
| manifest/diagnostics 只校验存储字段，缺少统一身份重算与直接授权证据 | HTTP 与 `design_context.status` 原先各自读取冻结状态，未统一重算 payload/artifact/profile hash，也未直接证明历史快照、跨项目/owner 拒绝和响应最小披露 | 新增共享 `validate_run_design_context_identity`：重算 manifest content/artifact-manifest hash，逐文件核对 bytes/SHA-256，核对冻结 effective Profile id/version/hash/project scope，并比对 Run 的 Profile/DCP/Brief/policy/appRoot/mode/warnings 身份；HTTP 与 tool 共用该校验并 fail closed。新增 HTTP 回归覆盖无凭证 401、跨项目/错误 owner 403、合法 owner 200；冻结 Profile scope 与 Run project 不一致在 attach 前拒绝，遗留/持久化 identity 漂移经同一 validator 映射为 HTTP conflict 或 typed tool error。Profile 不存在于当前 store 时历史 Run 仍按冻结快照可读；manifest/diagnostics 与 `design_context.status` 均不回传 Profile 私有值、browser executable、完整 verifier environment 或 runtime token mapping |
| 完整性校验只覆盖 diagnostics，执行生命周期仍可旁路 | AgentLoop materialization、mutation read gate、Edit/Repair child、Profile Sync 与 fidelity 各自只比较少量字段；任一冻结 artifact 或 Run identity 漂移可能在诊断失败时仍进入执行 | 新增统一 `frozen_run_design_context_manifest` 入口并替换全部生产解析点；attach 在写入前原子验证，child creation/inheritance 在复制前验证，materialization/read/style-contract evidence 只能写入完整且 hash 精确匹配的 DCP；mutation/publish、Profile Sync plan/apply 与 fidelity 均 fail closed。新增回归证明 artifact tamper 在 mutation 前返回 `design_context.integrity_failed`、非法 attach 不留下半写 Run、错误 materialization hash 与 materialize 前 read evidence 均被拒绝 |
| Repair child 继承 DCP 后被当前 Profile 重绑 | StartRun 在 `create_repair_run_for_findings` 已复制冻结 Profile/DCP 后，仍执行普通 active Profile attach；Repair 无 surface/template execution target 时会清空 inherited effective hash，直到 workspace bootstrap 才失败 | 对已有冻结 DCP 的 child 跳过当前 binding 重绑和 fidelity 重配置，保持父 Run 的 Profile revision/effective hash/source mode；Build→Edit→Review→Repair 的 small/large source HTTP lifecycle 均重新通过 |
| Profile Sync 控制面读取被误记为 Agent required-read | sync apply 在 AgentLoop 前经 control-plane `fs.read` 读取实际 style contract，通用 post-tool hook 会把该读取写进 child Run read evidence，并尝试提前标记 style verified | DCP read evidence 现在要求冻结身份有效且 materialization hash 已存在；Store 与 post-tool hook 双重拒绝 materialize 前写入，控制面读取不再替模型满足 read gate。全量 gate 因而暴露 large-source Repair fixture 依赖物化阶段的隐式 Profile 读取；不放宽生产门禁，改为 Repair Agent 显式读取 `inputs/design-profile.json` 后再 mutation/publish。BFF recovery、Runtime sync、Repair 定向回归与 HTTP 全量均通过 |
| 指标表定义十类、Runtime 原先只发两类成功路径 | compiler、read gate、source、fidelity、rule/a11y/responsive、capability 与 runtime-lost 只有工具结果或 report，没有统一低基数事件；Profile Sync 也只记录 plan/apply 成功，无法区分 confirm 和稳定拒绝原因 | 在真实决策点补齐十类 `MetricRecorded`，公共 helper 只加入固定 `mode/surface/phase`；compiler 只记录 passed/failed 与稳定 reason，read gate 只记录 reason/计数，source 只记录访问模式/section 数/bytes，fidelity 只记录稳定 rule/recipe/kind/priority/viewport，sync 只记录 stage/status/机器码。新增回归覆盖 compile success、plan drift、confirm hash mismatch、initial failure→repair pass、a11y/responsive、runtime-lost、indexed source read 与 capability gap；禁止写入路径列表、artifact/operation id、token/source/profile 原文或错误正文 |
| Run 详情未解释 fidelity outcome | Runtime 已产出完整 fidelity report，但通用 HTTP diagnostics 与 ProjectShell 只显示 DCP/read/style-contract，DoD #10 和失败卡片要求未落地 | diagnostics 从绑定 Run 的 Runtime-owned conversation evidence 取最新 report，只投影 bounded rule/recipe/kind/selector/viewport、受控 current/target 摘要与规范化 appRoot repair target；不回传 preview URL、raw source/HTML、完整 report、browser executable、token mapping 或绝对 workspace 路径。ProjectShell 提供可锚定 rule detail、required failure 数、element/viewport/property/current/target 与 repair context；真实 Router + Chrome L4 和授权最小披露回归通过 |

完整 gate 的可证明范围：Rust 格式与边界检查、sandbox contract/tools/permission、AgentLoop、HTTP fixture、Astro/Fumadocs build、shared package、provider gate fail-closed/dry-run、evidence/canary validator、computed-style 与 dark-theme smoke。最终一次全量结果为 sandbox tools `93/0/1`、HTTP `110/0/3`、template build `5/5`；fidelity report 精确路径行为、sandbox security、tool-permission、project authorization 与 Web BFF smoke 分别为 `1/1`、`13/13`、`10/10`、`4/4`、pass。它证明诊断只能读取 Runtime-owned report、HTTP 摘要及 bounded fidelity outcome 受 Run project 权限和冻结 Profile scope 约束，恶意 metadata 中的 raw source/value、private URL、browser executable、token mapping 与绝对 workspace path 不会进入响应；但本地 harness 与严格 RC 仍是分层证据，不能互相替代。

#### 0.4.6 严格 clean release RC review 记录

严格 RC 使用 detached clean 候选、fresh cluster、release preflight、批准 Provider 与全项目矩阵执行。执行过程中暴露的问题均按 Runtime/tool contract 修复，未通过弱化门禁关闭：

| 缺口 | Runtime / 工具补充 | 防回归与证据 |
|---|---|---|
| 120 秒 project principal 在长 Provider 等待后过期 | runner 在每个长等待后的 project API、artifact access 前签发 fresh JWT；不延长 token TTL，不复用旧 principal | release fixture/real/DCP artifact 在 Sandbox 释放后均以 project principal 返回 `200`；evidence 只记录 auth scope/status，不记录 token |
| Docker Hub 匿名 429 阻断 release preflight | image lock 增加显式 approved fallback；只有 canonical rate-limit/network failure 才启用，fallback digest 必须等于 locked digest | `runtime-rc-preflight@1` 记录 source/reason/digest；11/11 entry verified/matched/pulled |
| 批量 `fs.read` 并行时 required-read lease 被后写覆盖 | `fs.read` 因 read-tracking 副作用标记为非 concurrency-safe；真正无状态的诊断工具仍可并行 | `batched_fs_reads_preserve_the_target_patch_lease` 与 sandbox tools `93/0/1` |
| artifact release 后匿名 probe 固定得到 401，无法证明授权可读 | 改为 fresh project-scoped principal，并把 `artifactAccessAfterRelease` 设为 release validator 必填 | Website/Docs fixture、真实 Provider 与 enforced DCP 均记录 authenticated `200` |
| Docs 实际发布在 `/docs/`，runner 错查 `/` | artifact assertion 显式记录并校验 kind-specific route | release validator 要求 Website `/`、Docs `/docs/`；真实 Docs 内容与 computed-style pass |
| Edit 可在实际 mutation 前 publish，candidate freeze 后无法再补写 | Build/Edit 使用不同验收文本；runner 必须在首次 publish 前观察到 Edit source mutation；candidate freeze 保持不变 | 真实 Website/Docs Build/Edit 均生成新 source snapshot/version，terminal tool failure 为 `0` |

本次机器结论为 `RC_RELEASE_ELIGIBLE=true`。历史 RC 关闭批次 C，但当时不能自动把批次 D/E 标为完成；后续第 0.4.7 节的同候选 Provider RC 已关闭批次 D。clean detached commit 仍必须进入正式 review/提交或由等价源码重建，临时证据目录也必须迁移到受控 evidence store。

#### 0.4.7 同候选真实 Provider DCP review 记录

2026-07-15 以 detached clean candidate `a0d036f3d8ad9a16343260bd6861ff736b04111b`、fresh cluster `zd-dcp-provider-rc-0715-2312` 和 Runtime image manifest `sha256:7e378a4e05f10cafe774aa1ca0f692700be9c5a0bb5553569e592e048af28ef7` 执行 release 模式 RC。最终 `release-evidence@1` 为 `releaseEligible=true` / `result=pass`，并再次通过 `validate-release-evidence.mjs`；generic API-key 扫描与 evidence `secretScan.matches` 均为 `0`。

同次候选的真实 Provider 证据包括：

- 普通 Website：Build `run-1380`、Edit `run-1485`，路由 `/`，artifact 内容/computed-style 通过，释放后 project principal HTTP `200`。
- Docs：Build `run-1593`、Edit `run-1690`，路由 `/docs/`，artifact 内容/computed-style 通过，释放后 project principal HTTP `200`。
- enforced DCP Website：Build `run-719`、Edit `run-861`、Review `run-1025`、Repair `run-1040`；`finding-1031` 为 blocking/repairable/`fixed`。Build/Edit/Repair 均为真实 `deepseek-chat` mutation，Review 来源明确记录为 `fixture-seeded` / `deterministic-tool-sequence`。三段 diagnostics 均满足 gate ready、materialization ready、fidelity passed、missing required reads 为空，最终 `/` artifact 断言通过，释放后授权读取 `200`，terminal tool failure 为 `0`。

此记录关闭“同一 release candidate 未合并真实 Provider DCP Repair”的批次 D 缺口，并允许申请单项目 canary。它不关闭三项边界：Review 不是模型自主发现；detached candidate 尚未成为正式分支提交；7 天/30 publish、指标结论页和 `enabled=false` 回退仍未发生。因此文档升级为 `provider-verified`，但 enforcement rollout 继续保持 **No-Go**。

#### 0.4.8 Batch E 启动前工程 review 记录

Batch E 审计发现五项会使“真实 Canary”退化为人工填表或只告警不通知的缺口，现已在代码层关闭，但尚未产生真实观察窗：

1. 原校验器只验证最终汇总 JSON，没有逐日来源链。新增 `design-context-canary-ledger.mjs`，以受锁 NDJSON hash chain 记录 session、Website Run、required finding、Profile Sync、BFF、publish samples、metrics snapshot、alert delivery、rollback 与 compatibility；每条记录绑定来源 URI/文件 SHA-256。append 阶段拒绝凭证模式和无效语义，finalize 从嵌入记录重新计算样本数、窗口和 failure-rate delta，且拒绝覆盖既有输出。篡改任一 payload/hash/顺序或只改汇总数字均失败。
2. 原 Run 只冻结 effective observe/enforced，没有冻结产生该结论的 rollout policy revision。`AgentRun` 现新增 `designContextEnforcementBinding`，生产 Build 与 Profile Sync child 在 DCP attach 时原子记录 `source/enabled/policyRevision/policyUpdatedBy`，Edit/Repair 继承该绑定，manifest/diagnostics 的 package summary 对 BFF/Canary 暴露最小化字段。Persistent binding 必须有正 revision/updatedBy，config binding 不得伪造 persistent revision，enforced DCP 不得绑定 disabled policy。
3. 原回退证据仍可由人工声明。新增两阶段 `run-design-context-canary-rollback.mjs`：`disable` 使用 internal-admin CAS 写入同一精确 policy 的 `enabled=false` 并要求 revision `+1`；随后 `verify` 只接受 rollback 后新建且完成的 Run，核对 Runtime diagnostics 中 frozen persistent disabled binding、effective observe、clean read gate，并从 ledger 验证 Profile Sync recovery 证据仍在，最后才生成可 append 的 rollback event。Admin/principal token 只经环境传入且不写入证据。
4. 原 `metrics.snapshot` 仍可人工填写，并且账本会拒绝保存告警触发的失败快照。新增 internal-admin `GET /internal/projects/{projectId}/design-context-canary-metrics`，只从 Runtime durable Run/event/Profile Sync operation 按精确两个 policy revision 与不可变时间窗聚合 publish 样本、failure-rate delta、verifier/read-gate/recovery/repair 指标和停止/回退告警；`collect-design-context-canary-metrics.mjs` 将响应固化为不可覆盖的 operational export、逐日 metrics fragment 与最终 publish fragment，`render-design-context-canary-report.mjs` 生成不越权宣告 `canary-verified` 的一页报告。账本现在允许保留进行中或失败的多个 metrics snapshot，finalize 只使用最后一份并继续对未达阈值结果 fail closed。
5. 原告警仅存在于运营 JSON，没有外部 paging 和投递审计。新增 `dispatch-design-context-canary-alerts.mjs`：首个 Canary workload 和指标采集前用签名 probe 验证真实值班目的地；日常只接受 Runtime durable-store export 的固定告警码，触发时使用环境变量中的 HTTPS webhook 与 HMAC-SHA256 密钥投递，携带稳定 event id 供接收端幂等。非 2xx、非 HTTPS、缺密钥或输入不一致均不生成成功 fragment。URL、签名、密钥和响应正文不进入证据，只保留目的地逻辑 ID、HTTP 状态与触发码。source ledger 必须有且只有一个成功的 `alert.destination-probe`，且每个 `metrics.snapshot` 必须有唯一匹配的 `alert.delivery`；触发告警要求 `delivered`，无告警要求显式 `not-required`，缺失或伪造均使最终 validator 失败。

Review 同时修正了旧 schema 的不可能条件：同一 persistent policy 无法同时产生 observe 与 enforced。Canary cohort 现在固定同一 project/Profile revision，但使用 `observePolicyRevision`（disabled）与后续 `policyRevision`（enabled）两个不同且递增的 CAS revision；required repair、Profile Sync、指标与回退继续绑定 enforced revision。Ledger 与 validator 均按此真实状态机交叉验证。

上述能力只把 Canary 变成“可真实采集、可发现篡改、可从 Runtime 绑定 policy”的工程流程；它不能预先制造 7 天时间、30 次 publish、生产告警结果或回退后的真实 Run。因此 Batch E 仍保持未完成。

#### 0.4.9 Fumadocs 构建确定性 review 记录

全量并行 gate 暴露出 Fumadocs 的生成时序竞态：`next build --webpack` 偶发在 `.source/server.ts` 生成完成前解析 `../.source/server`，表现为单测孤立运行偶尔通过、全量运行稳定失败，失败现场随后又能看到生成文件。该问题不是测试噪声，也不能通过重试或放宽断言处理。模板 build 已改为先显式执行 `fumadocs-mdx`，再执行 `next build --webpack`；模板测试冻结该命令，全量 `astro_build_agent` 恢复为 `5/5`，完整本地 gate 再次以退出码 `0` 通过。该修复只关闭本地/模板构建确定性，不提升 Provider 或 canary 证据等级。

#### 0.4.10 最新 Batch E clean audit RC review 记录

2026-07-16 使用 alternate index 与 `git commit-tree` 生成 detached clean 实施候选 `1c0cd85b64db6008dd5a746b544d90786a73e192`；118 个纳入文件逐一与组合工作区做 SHA-256 对比，排除两份独立长规格，真实 Git index 保持未暂存。候选先通过 registry policy 定向测试和独立 audit preflight，再在 fresh cluster `zd-dcp-canary-rc-0716-1005` 执行 fixture audit RC。最终 Runtime image manifest/config、`/version` 与 evidence 的 repository commit 均绑定 `1c0cd85b64db`，仓库 dirty 为 false；Website、Docs、enforced DCP fixture、项目 Pod egress、mTLS/auth、8 类 recovery 和 secret scan 均完成。因 provider 明确为 fixture 且无 approval/credential，`release-evidence@1` 正确返回 `releaseEligible=false` / `result=fail`，没有通过降低 validator 冒充 release。

本轮保留并关闭了三类基础设施问题：

1. 首次 clean audit RC 在 worktree 中临时复用 `packages/shared/node_modules` symlink，仓库 fingerprint 将 symlink 当目录递归，预检失败。修正执行纪律为：本地 gate 可以临时挂载依赖，但任何 fingerprint/preflight/image build 前必须移除；失败目录 `zd-dcp-canary-rc-0716-0950` 保留，不删除来制造成功历史。
2. 后续 registry inspect 分别遇到 CloudFront/plain `EOF` 和 Debian auth endpoint/plain `EOF`。旧 fallback 分类器只识别 `unexpected EOF`，与方案声称的“429/EOF exact-digest fallback”不一致。新增 `preflight-registry-policy.cjs` 与定向测试，将单词边界 `EOF`、connection reset、TLS handshake timeout、`ETIMEDOUT` 和 429/rate-limit 归为可恢复 transport/rate-limit；`manifest unknown`、digest mismatch、denied 等 identity/authorization 错误仍禁止 fallback。即使分类命中，也只有 lock 显式声明 approved mirror 且 mirror 返回完全相同 locked digest 时才允许继续，未放宽镜像身份。
3. Docker Desktop 本次配置为 8 GiB。Runtime release layer 在内存/IO 压力下约耗时 42 分钟，但最终成功；因此这不是代码失败，也不能靠缩短 timeout 隐藏。RC runner 必须记录 CPU/内存/磁盘和 build duration，单次 Runtime image build timeout 至少预留 60 分钟。8 GiB 只是“本机观察到可完成”的下界，不是 CI 容量承诺；稳定 CI 建议从 12 GiB 及以上起配并用连续多次构建重新标定，未实测前不得把该建议写成已验证门槛。

部署后又对新增 metrics route 做了实际集群核验：完整 query 无内部授权返回 `401`；从 cluster secret 仅在进程内读取管理员 token 后返回 `200`，响应 schema 为 `design-context-canary-operational-export@1`，cohort/profile/policy/window 与固定 alert code 形状正确，响应和命令输出均未包含 token。该查询使用未来 canary revision `2` 和历史时间窗，当前 store 没有匹配 observe/enforced publish，因此样本数为 `0`。此结果关闭“新增 route 只在单测存在、未进入镜像”的部署缺口，但不关闭任何真实样本、阈值、paging、7 天窗口或 rollback 缺口。

证据分层保持不变：`a0d036f3d8ad` 仍是最新 `provider-verified` release RC；`1c0cd85b64db` 是包含最新 Batch E 代码的 `verified-fixture` clean audit RC。正式生产部署必须从经过 review 的分支 commit 重建，重跑 `PREFLIGHT_PREFETCH_IMAGES=1` release preflight 和真实 Provider/approval 门禁，并绑定真实 webhook readiness probe，不能直接把 detached audit candidate 提升为生产物。

#### 0.4.11 主功能通路复验与 L4 路径修复记录

2026-07-16 按“主要功能正常可用、暂不验证 7 天审计稳定性”的范围重新执行核心门禁。结果为：DCP compiler/verifier `8/8`、required-read/fidelity gate `2/2`、Profile Sync 内核 `10/10`、Runtime HTTP Profile Sync 与 crash recovery `2/2`、Web typecheck 和 BFF contract smoke 全部通过；Website fixture 分别证明 Build 能在真实 workspace 完成 required reads、source mutation、build/publish，Edit 能继承冻结 DCP 并恢复/重新物化 promoted snapshot，Repair 能阻断 required a11y failure 并只在修复后 promotion。联合 L4 最终证明 headless browser、ProjectShell、Next BFF、公开 Runtime Router、三方 token plan/conflict decision/confirm、真实 token 写回与唯一 child Run 串联成功。

联合 L4 首次执行失败于 BFF 启动前：复用的 Cargo test artifact 将旧 clean-candidate 的 `CARGO_MANIFEST_DIR=/private/tmp/zerondesign-dcp-canary-88ace9c99aca/services/runtime` 编译进测试二进制，导致脚本从已失效的临时 `apps/web` 路径启动；独立 BFF smoke 同时通过，证明不是业务路由失败。测试入口已改为优先读取运行时 `ZERONDESIGN_REPO_ROOT`，否则从当前目录向上发现 `apps/web/package.json`，编译期目录仅作为最后兜底。强制重编译后同一联合 L4 通过。该修复保证复制/复用 target artifact 时不会静默指向旧 worktree，属于测试可移植性修复，不改变生产 Runtime 行为。

本轮不生成、加速或伪造 7 天/30 publish 数据，也不把本地 ledger 耐久性测试作为功能交付条件。第 15 节完整 canary 仍作为未来扩大生产 enforcement 的独立运营流程保留；当前交付结论是 **Functional Go / rollout No-Go**。

### 0.5 决策记录（不可在实现中悄然改变）

| 决策 | 固定结论 | 变更门槛 |
|---|---|---|
| DCP 真相源 | `DesignProfile + Brief + template capability`；DCP 只是冻结投影 | 需要新的 profile/lifecycle 设计评审，不可由 compiler 或 BFF 旁路 |
| 强制执行资格 | `master=true`、`enforcement=true`、Profile declared enforced，且精确 policy 允许；持久化 `enabled=false` 优先于环境 allowlist | 新增资格维度须更新 flag 矩阵、审计和回滚剧本 |
| 发布裁决 | 仅 `preview.publish` 可阻断 required rule；诊断工具只读 | 新 gate 必须有稳定 rule id、证据与 observe 兼容路径 |
| Profile sync | 控制面显式确认；agent 不可创建/确认/重开 operation | 任何自动同步都必须重新评审授权、冲突和可恢复事务 |
| 浏览器运行时失效 | enforced fail closed，observe 只记录 warning；禁止回退到未探测浏览器 | 仅可通过 VerifierRegistry/政策版本升级调整 |
| 灰度回退 | 先对精确 policy `enabled=false`，再关闭 enforcement，最后才关闭 master | 回退不能删除 run/operation/evidence，必须保留审计链 |

### 0.6 方案决策摘要

为 Website Build / Edit / Repair 增加一个由有效 `DesignProfile` 编译出来的、可冻结且可验证的 **Design Context Package（DCP）**。DCP 不是新的 Profile 类型，也不改变现有 `DesignProfile`、`Style Contract`、`Design Capsule`、Fidelity Gate 或 sandbox 权限模型；它是它们之间面向 Website agent 的运行时投影。

每个 DCP 必须区分两类信息：**内容包**是可确定性重建的设计上下文；**Run Binding**只记录该内容包被哪个 Run 在何时 materialize、读取和验证。`runId`、时间戳和读取状态不得参与 DCP 内容 hash。

```text
DesignProfile active revision + surface/template overrides
  + Brief（website / astro-website）
  + template capability
  + imported source index（可选）
             │ compile once at StartRun
             ▼
     Design Context Package（run snapshot）
             │ materialize into sandbox
             ▼
 usage.md / component-recipes.json / context-index.json
 style-contract.json / design-profile.json / design.md
             │ required-read gate + tool policy
             ▼
 Build / Edit / Repair → preview.publish → fidelity report
```

目标是让模型在第一次生成 Website 时得到“可执行的 token 与组件约束”，而不是只得到一段设计说明；同时保留 Runtime 对实际源码、模板能力和发布质量的强制校验。

## 1. 当前基础与问题定义

当前 Runtime 已完成以下关键能力：

- StartRun 可按 explicit → project → workspace → organization 解析 `DesignProfile`；显式 id 不可见时不 fallback。
- Build 会将 surface/template override materialize 成 effective profile，并把 id、version、base hash、effective hash 冻结到 `AgentRun`。
- sandbox 中已有 `inputs/design-profile.json`、`inputs/design.md`、`inputs/brief.md`；imported Profile 在 source fallback 时还会出现 `inputs/design-source.md` 和 index。
- Build/Edit Prompt 已要求读取 design context；调用改变源码/发布的工具前，Runtime 会阻断未完成 required-read 的 run。
- `style.update_tokens` 与 `state/style-contract.json` 是模板 token 的执行边界；`preview.publish` 会返回 fidelity report 并要求修复 required failure。

缺口不在“有没有 Profile”，而在 **Profile 到 Website 生成上下文的投影仍过于粗粒度**：

1. `design-profile.json` 面向完整契约与审计，模型需自行从嵌套 JSON 中推导页面组件如何组成。
2. `design.md`/Capsule 是压缩描述，但没有显式告诉模型每个 Website 组件哪些 recipe、状态和 token 可用。
3. source index 已经解决“原始设计来源怎么安全读取”，但没有表达 source section 对 token、组件行为、视觉参考、内容语气的用途和优先级。
4. Run hash 覆盖 effective Profile，但没有单独说明本次实际投递到模型/要求读取的所有派生产物集合。
5. Website 的通用质量规则（响应式、键盘焦点、表单状态、避免泛化 UI）仍主要分散在 Prompt 或各 Profile 中，复用与模板适配不足。
6. 当前 `browser.screenshot` 固定在单一桌面 viewport，`preview` 的 `accessible` 只表示 URL 可连通；两者都不能证明移动端布局或 WCAG/ARIA 可访问性。

## 2. 范围与非目标

### 2.1 本期范围

- 仅支持 `surface=website` 且 `template=astro-website`。
- 覆盖 Build、Edit、Repair；Brief 和 Docs 不改动行为。
- 增加 DCP 编译、run snapshot、sandbox materialization、required-read gate、fidelity evidence 与测试。
- 只复用现有 Runtime API；`POST /runs` 的外部 request shape 不因 DCP 而改变。

### 2.2 非目标

- 不引入第二套 DesignProfile 存储、绑定或生命周期。
- 不让 agent 直接复制任意 `tokens.css` 或绕过 `style.update_tokens`。
- 不把完整 source、HTML fixture 或预览页面无差别塞进 system prompt。
- 不把模型自述的“自检通过”当作发布门禁；发布仍以 Runtime fidelity/report 为准。
- 不把 Website Clone 纳入本期。未来 clone 模式必须明确是否禁用项目 DesignProfile，不能静默套用品牌规则。

## 3. 目标架构

### 3.1 固定层级与优先级

任何 Website run 使用下列优先级，低优先级不得覆盖高优先级：

```text
Runtime policy / sandbox boundary
  > 用户确认的 Brief 与本轮 Edit acceptance criteria
  > effective DesignProfile（base + surface + template overrides）
  > Website craft packs
  > template style contract 与 component capability
  > imported design source evidence
  > agent 的推断
```

其中 `effective DesignProfile` 继续沿用现有 id/version/hash 语义；DCP 只记录其运行时投影，不能成为新的 source of truth。

### 3.2 DCP 组成

| 产物 | 路径 | 来源 | 用途 |
|---|---|---|---|
| 完整 Profile | `inputs/design-profile.json` | effective Profile | 审计、完整结构化契约 |
| Design Capsule | `inputs/design.md` | 现有 capsule + readable design_md | 模型的高层视觉、内容、无障碍摘要 |
| 使用协议 | `inputs/design-profile-usage.md` | DCP compiler | 读序、token/编辑策略、禁止事项 |
| 组件配方 | `inputs/component-recipes.json` | `profile.components` + template registry | 生成时可复用的组件、状态、a11y、token binding |
| 上下文索引 | `inputs/design-context-index.json` | source index + DCP compiler | 按需读取 source/preview/evidence 的用途、优先级与预算 |
| DCP manifest | `state/design-context-manifest.json` | DCP compiler | 实际上下文集合、hash、版本、required reads |
| 样式执行契约 | `state/style-contract.json` | template + existing mapping | 唯一可通过 `style.update_tokens` 修改的 token 边界 |
| 初始化前样式契约 | `inputs/template-style-contract.json` | template 的纯 `StyleContractSpec::render` | `project.init` 前可安全读取的 token 能力声明 |

所有路径都使用既有 workspace 相对路径；不得允许模型以绝对路径绕过 sandbox。

## 4. 数据契约

### 4.1 新类型

在专用 `services/runtime/src/design_context.rs` module 定义并序列化以下结构；`AgentRun` 的持久化字段仍位于既有类型层。字段使用 camelCase 对外、snake_case 对内的现有惯例。不得再把 DCP schema 分散到 route、tool 或 Web BFF 中各自解释。

```rust
/// Deterministic design-context payload. It has no derived self hash,
/// run id, timestamps, read status, or other volatile fields.
pub struct DesignContextPackagePayload {
    pub schema_version: String,              // "design-context@1"
    pub design_profile_id: String,
    pub design_profile_version: u32,
    pub base_profile_hash: String,
    pub effective_profile_hash: String,
    pub brief_hash: String,
    pub brief_schema_version: String,
    pub surface: String,                     // website
    pub template: String,                    // astro-website
    pub template_manifest_sha256: String,
    pub expected_app_root: String,            // project
    pub compiler_version: String,
    pub declared_enforcement_mode: ProfileEnforcementMode,
    pub effective_compatibility_mode: ProfileCompatibilityMode,
    pub verification_policy: VerificationPolicySnapshot,
    pub artifact_manifest_hash: String,
    pub resolved_runtime_tokens: BTreeMap<String, String>,
    pub resolved_token_snapshot_hash: String,
    pub required_reads: Vec<DesignContextReadRequirement>,
    pub craft_packs: Vec<ResolvedCraftPack>,
    pub layout_guidance: Vec<LayoutGuideline>,
}

/// Expected materialized files. This manifest never contains itself or the
/// outer state/design-context-manifest.json file.
pub struct DesignContextArtifactManifest {
    pub schema_version: String,              // "design-context-artifacts@1"
    pub artifacts: Vec<DesignContextArtifact>,
}

/// Persisted wrapper. Hashes are derived before this object is assembled.
pub struct DesignContextManifest {
    pub schema_version: String,              // "design-context-manifest@1"
    pub payload: DesignContextPackagePayload,
    pub content_hash: String,
    pub artifact_manifest: DesignContextArtifactManifest,
}

/// Volatile run snapshot. Persist it on AgentRun/evidence, never as input
/// to DesignContextPackagePayload/contentHash.
pub struct DesignContextRunBinding {
    pub run_id: String,
    pub brief_version_id: String,
    pub design_context_content_hash: String,
    pub materialization_hash: String,
    pub compiler_version: String,
    pub verification_environment: VerificationEnvironmentBinding,
    pub materialized_at: DateTime<Utc>,
}

pub struct ComponentRecipe {
    pub id: String,                          // e.g. "button.primary"
    pub role: String,                        // navigation, action, content, input
    pub priority: RecipePriority,             // required | preferred
    pub allowed_templates: Vec<String>,
    pub anatomy: Vec<ComponentPart>,
    pub states: Vec<String>,                 // default, hover, focus-visible, disabled
    pub token_bindings: BTreeMap<String, String>,
    pub accessibility: Vec<String>,
    pub source_refs: Vec<DesignContextReference>,
    pub verification: Vec<RecipeVerification>,
}

pub struct DesignContextArtifact {
    pub path: String,
    pub kind: String,                        // profile, usage, component_recipes, source_index...
    pub bytes: u64,
    pub sha256: String,
    pub required_before_mutation: bool,
}

pub struct DesignContextReadRequirement {
    pub path: String,
    pub reason: String,                      // token_contract, component_recipe, source_evidence...
    pub phases: Vec<AgentPhase>,
}

/// Versioned rule set that is part of the deterministic DCP policy.
pub struct VerificationPolicySnapshot {
    pub policy_id: String,
    pub a11y_ruleset_version: String,
    pub viewport_matrix_id: String,          // e.g. website@375-768-1440/v1
    pub required_verifier_kinds: Vec<String>,
}

/// Actual Runtime execution evidence. It is recorded on the Run Binding,
/// not used to reinterpret an already compiled DCP policy.
pub struct VerificationEnvironmentBinding {
    pub registry_version: String,
    pub capability_snapshot_hash: String,
    pub browser_worker_version: Option<String>,
    pub a11y_engine_version: Option<String>,
    pub viewport_engine_version: Option<String>,
    pub availability: VerificationAvailability,
    pub resolved_at: DateTime<Utc>,
}
```

需要在同一 module 明确定义 `ComponentPart`、`DesignContextReference`、`ResolvedCraftPack`、`LayoutGuideline`、`RecipePriority`、`RecipeVerification`、`ProfileEnforcementMode`、`ProfileCompatibilityMode`、`VerificationAvailability` 的完整 schema、未知字段策略及稳定排序规则；不能把它们留给各调用方自行解释。`ResolvedCraftPack` 必须包含 pack id/version，`RecipeVerification` 必须引用稳定 verifier/rule id。

Hash 采用无自引用的三层语义，计算顺序不可改变：

- 先生成所有预期 artifact bytes，按相对路径稳定排序并计算 `DesignContextArtifactManifest`；它只能覆盖 path、kind、bytes、sha256、required-read metadata，明确排除 `state/design-context-manifest.json` 自身。
- `artifactManifestHash = canonical_json_hash(artifactManifest)`。
- 将 `artifactManifestHash` 写入不含 `contentHash` 的 `DesignContextPackagePayload`，再计算 `contentHash = canonical_json_hash(payload)`；禁止对最终 `DesignContextManifest` 整体求 content hash。
- materialize 后重新扫描预期路径，构造同 schema 的 actual manifest；`materializationHash = canonical_json_hash(actualManifest)`。actual manifest 中的 bytes/hash 与 expected manifest 任一不一致都阻止模型启动。
- 最后才组装 `DesignContextManifest { payload, contentHash, artifactManifest }` 并写入 state；读取时必须重算 artifact manifest hash 并与 payload 中的值比较。外层 manifest 不参与上述任一 artifact/hash 输入。

`briefHash` 固定定义为 `canonical_json_hash(serde_json::to_value(&brief))`，使用 Runtime `Brief` 结构本身而不是渲染后的 Markdown；首期 `briefSchemaVersion = "brief@1"`。`briefVersionId` 仅进入 Run Binding，用于定位存储记录，不进入内容 hash。同一 Profile revision、同一冻结 Brief、template target、compiler version 与 verification policy 必须得到相同 `contentHash`；同一 run snapshot 重放时允许产生新的 `materializedAt`，但不得改变 `contentHash`。Brief 正文只存在于现有受控输入，不出现在 manifest/diagnostics。

`resolvedRuntimeTokens` 只包含经 template capability/style contract 验证后可执行的 token name/value，按 token name 稳定排序；`resolvedTokenSnapshotHash = canonical_json_hash(resolvedRuntimeTokens)`。它既是首次 `project.init` 应用的目标值，也是后续 Profile sync 的 **base/target 设计意图快照**，不能在 sync 时重新从已变化的 Profile 推导旧 base。它不是当前项目 token 文件的替代品：同步时的 `current` 必须重新从实际 `state/style-contract.json` 指向的 tokenFile 解析。

### 4.2 Profile 的最小增量字段

本期不强制迁移存量 Profile。V2 Profile 可选择性增加以下字段；缺失时 compiler 生成空 recipe/pack，并固定标记为 `legacy_observe`，而不是推断为 enforced：

```json
{
  "components": {
    "recipes": [
      {
        "id": "button.primary",
        "role": "action",
        "anatomy": ["label", "optional-icon"],
        "states": ["default", "hover", "focus-visible", "disabled"],
        "tokenBindings": {
          "background": "color.primary",
          "foreground": "color.primaryContrast",
          "radius": "radius.control"
        },
        "accessibility": ["visible-focus", "disabled-semantic"],
        "verification": [
          { "kind": "dom", "selector": "a.button-primary, button.button-primary", "minMatches": 1 },
          { "kind": "computed-style", "selector": ".button-primary:focus-visible", "property": "outline-style", "comparator": "not-equal", "expected": "none" }
        ]
      }
    ]
  },
  "websiteContext": {
    "enforcementMode": "enforced",
    "craftPacks": ["accessibility-baseline", "responsive-layout"],
    "sourcePriority": {
      "token-evidence": "required",
      "component-behavior": "required",
      "visual-reference": "optional"
    },
    "layoutGuidance": [
      { "area": "hero", "intent": "single-primary-action", "priority": "preferred" },
      { "area": "section-rhythm", "intent": "alternate-density", "priority": "preferred" }
    ],
    "extensionPolicy": "allow-with-style-contract"
  }
}
```

`required` recipe 是发布契约，必须含至少一个可执行 verification；`preferred` recipe 仅作为生成/Review guidance。组件配方是 **required/preferred reference inventory**，不是封闭的“允许组件列表”：Brief 合理需要的新组件可创建，但必须遵守 `extensionPolicy`、复用 style contract，并声明语义角色和交互状态。

存量 Profile 的渐进策略必须固定，不能由 compiler 临时猜测：缺少 `websiteContext` 的 Profile 在 master flag 开启后进入 `legacy_observe`，只生成 DCP 和 warning，不新增 required craft-pack/a11y/responsive 发布阻断；V2 Profile 的 declared mode 只有明确 `enforcementMode: enforced` 才可能启用 required verifier，但 effective mode 还必须同时满足 enforcement flag/allowlist。项目可显式 opt-in 将存量 Profile 升级为 declared enforced；master flag 关闭时全部回到既有 Profile/Design Capsule 行为。

未知字段保持 forward-compatible；非法 token binding、未知 craft pack、非法 verification 在 Activate 时做 schema 校验。与具体 `surface/template` 有关的能力判断只能在 StartRun（已知 effective target）执行；模板不支持的 required recipe 返回可解释 capability gap。

## 5. DCP 编译与冻结流程

### 5.1 StartRun / Build

```text
POST /runs (build)
  → resolve DesignProfile
  → resolve effective profile for website + astro-website
  → validate Brief template against profile.allowedTemplates
  → freeze expectedAppRoot=project for a new Website Build
  → resolve verification policy and Runtime verifier availability
  → evaluate template capability gaps
  → compile deterministic payload + expected artifact manifest
  → persist content/artifact identity into AgentRun
  → pre-provision integrity/capability checks
  → provision sandbox
  → materialize DCP files
  → verify actual manifest + persist Run Binding/materialization hash
  → launch model
```

模型启动前的 DCP 编译、能力检查或 materialization 失败必须返回以下四类可操作状态之一；前三类均保留可审计 Run/evidence，但不得进入 agent loop：

- `needs_user_input:design_profile_capability_gap`：Profile 的 required recipe/token 无法由目标模板支持。
- `blocked:design_verification_unavailable`：effective enforced Run 所需 browser/a11y/viewport verifier 未部署或版本不满足 policy；在启动模型前停止，绝不等到 publish 才发现。enforcement flag 关闭时只产生 observe warning。
- `needs_user_input:design_profile_integrity_failed`：effective Profile、source、DCP content hash 或 materialization hash 不一致。
- `invalid_request:design_context_compile_failed`：Profile 格式、recipe schema 或路径不合法。

### 5.2 Edit / Repair

- Edit 继承 parent/current version 的 DCP snapshot，不因 profile 的后续更新静默升级。
- 用户通过明确的“同步到最新 Profile”操作时，BFF/控制面先创建带授权主体、source run、target Profile revision 和过期时间的 `ProfileSyncOperation`；模型既不能创建也不能确认该 operation。
- `ProfileTokenSyncService` 复用 `style.update_tokens` 的 contract/token-file 解析与值校验逻辑：`base=parent DCP 记录的 token value snapshot`、`current=从实际 style contract 指向的 tokenFile 读取的当前值`、`target=new DCP mapping`。三方输入各自生成 canonical snapshot hash；不能把 style contract 的 token-name-to-variable 映射误当成 current token value。
- service 生成逐 token diff：无冲突项可提议 apply；`current != base && current != target` 的项标记 conflict，必须由用户选择 keep-current 或 apply-target。用户确认后才创建 child run，记录 `previousContentHash`、operation id、三方 snapshot hash 与每项决议。
- 目标 Profile 未声明的 token 一律为 `not_managed`：本期同步不删除 CSS 变量，也不以“目标中缺失”推断删除意图。删除/迁移 token 是独立、显式且可回滚的 style-contract migration，不得混入 Profile sync。
- 生成 plan 前必须同时校验 source run 已完成 DCP materialization 与 style-contract verification，且当前 `template/appRoot/contract schema/token-name-to-CSS-variable mapping` 仍与 source run 的冻结契约一致。任一 mapping 漂移、缺变量、重复声明、非法值或 tokenFile 不可读均返回 `profile_sync_precondition_failed`，不尝试猜测写入位置。
- confirmed operation 只能被同一 source run 的一次 child-run 创建流程消费。`RunLifecycleService` 在模型启动前调用 `ProfileTokenSyncService.apply_confirmed_operation`：写入前再次比较 current snapshot hash 与冻结 contract identity；写入后重新读取并校验已决议的目标 token、记录 before/after evidence，全部成功后才进入 agent loop。若第二次比较失败，operation 进入 `rejected`，child run 不得启动；若写入已发生但审计/读回未完成，operation 进入 `recovery_required`，child run 保持不可启动。该能力不注册为 sandbox/model tool，不暴露 `syncOperationId` 给模型，也不依赖 agent 是否记得调用工具。
- Repair 继承被审查版本的 DCP；只要求读取与 finding 类型相关的 DCP artifact，避免无关上下文阻塞紧急修复。

`ProfileSyncOperation` 是控制面持久化对象，而不是一次 HTTP 请求的临时结果。最低状态机为：`planned -> confirmed -> applying -> applied`；任何 precondition 失败进入不可消费的 `rejected`，任何写入后未完成审计的进程中断进入 `recovery_required`。`applied` 与 `rejected` 为终态，终态 operation 不得被重新打开或覆盖；`recovery_required` 只能依据原 operation 的 plan、before/after evidence 与当前读回做幂等恢复，不能重新编译 target DCP。

operation 保存 source run、target Profile revision/effective hash、冻结 contract identity、三方 snapshot、plan hash、逐 token resolution、授权主体、plan/confirm 各自的 idempotency key、过期时间、child run id 与 before/after evidence。相同授权主体、source run、target hash、source content hash、**plan** idempotency key 只能复用同一未过期 plan；相同 operation、plan hash、冲突决议、**confirm** idempotency key 只能复用同一 child run。任一相同 key 对应不同不可变输入时必须返回 `idempotency_key_reused`，不能静默覆盖。Runtime 没有跨 workspace 写入与审计存储的单一数据库事务时，必须以持久化 apply intent + 幂等 recovery 的 saga 实现，禁止在 crash 后靠重新生成 plan 继续写入。

### 5.3 与现有 run 字段的关系

在 `AgentRun` 新增以下可选快照字段：

```rust
pub design_context_package_version: Option<String>,
pub design_context_content_hash: Option<String>,
pub design_context_artifact_manifest_hash: Option<String>,
pub design_context_materialization_hash: Option<String>,
pub design_context_compiler_version: Option<String>,
pub design_context_brief_hash: Option<String>,
pub design_context_verification_policy_id: Option<String>,
pub design_context_expected_app_root: Option<String>,
pub design_context_resolved_token_snapshot_hash: Option<String>,
pub design_context_style_contract_identity_hash: Option<String>,
```

保留既有 `design_profile_hash` 与 `design_profile_effective_hash`；前者回答“绑定的 Profile 是什么”，DCP content hash 回答“实际要求模型读取的上下文集合是什么”，materialization hash 回答“本次 sandbox 是否正确落盘”。`resolved_token_snapshot_hash` 是 Profile 意图 token 的可重放 base/target，`style_contract_identity_hash` 是同步写入位置仍可信的前置条件；两者都不能替代实际 tokenFile 的 current snapshot。

## 6. Sandbox materialization 与读取门禁

### 6.1 文件写入

在现有 `AgentLoop::prepare_workspace_inputs` 中，Profile materialization 后调用。首期 Website Build 由 Runtime 在 StartRun 固定 `expectedAppRoot=project` 并写入 DCP/Run snapshot；不再允许模型为新 Build 自由选择第二个 appRoot。`inputs/template-style-contract.json` 使用该冻结值，由与 `project.init` 相同的纯 `StyleContractSpec::render` 生成；不得提前伪造或写入真正的 `state/style-contract.json`。

```rust
let dcp = self.design_context_compiler.compile(&run, &materialized_profile, &brief)?;
self.write_design_context_package(run, &dcp).await?;
```

文件写入成功后，`state/design-context-manifest.json` 的每项 hash 必须与落盘内容一致。任何不一致都应阻止 run 启动。Build 在 `project.init` 成功后验证实际 `state/style-contract.json` 与同一模板/Run 的预期样式契约一致；Edit/Repair 不重新 init，而是在各自成功读取实际 contract 时执行同一 identity 比较并持久化结果。比较范围仅包含标准化的 template id/version、appRoot、contract schema 和 token-name-to-CSS-variable mapping，不把可变 token CSS 值误判为契约漂移；parse 或 identity 不一致时三种 phase 都阻止后续 mutation/publish。

`project.init` 的 Runtime validation 必须读取 Run snapshot：省略 `path` 时使用 `expectedAppRoot`；显式传入其他 path 时返回 `project.app_root_mismatch`，不能先初始化再由 post-init gate 发现。Edit/Repair 继续以已存在的 `state/project.json.appRoot` 为真相源，并验证它与继承的 Run snapshot 一致。所有读取/写入 tokenFile 的 parser 必须与 `style.update_tokens` 共用同一套“单一 CSS variable declaration、分号终止、值校验”内核；不允许 DCP/sync 另写一个更宽松的 parser。

### 6.2 required-read 规则

| Phase | mutation 前的最小读取集合 |
|---|---|
| Build（`project.init` 前） | `brief.md`、`design-profile.json`、`design-profile-usage.md`、`component-recipes.json`、`template-style-contract.json` |
| Build（`project.init` 后） | 必须再读实际 `state/style-contract.json`，才可 `style.update_tokens`、源码 mutation、`project.build` 或 `preview.publish` |
| Build + source_fallback | 对应阶段集合 + source index；小 source 读 source，大片 source 读所有 required sections |
| Edit | `design-profile.json`、`design-profile-usage.md`、`component-recipes.json`、实际 `state/style-contract.json`；任何源码/token mutation、build 或 publish 前都必须重新验证 contract，不只在改 token 时读取 |
| Repair | `design-profile-usage.md`、`component-recipes.json`、实际 `state/style-contract.json` + 与 finding 对应的 token/source evidence；不要求读取所有 source |

门禁继续放在工具执行层，而不是靠 Prompt 提醒：`project.init` 使用 bootstrap read gate；`style.update_tokens`、源码 mutation、`project.build` 与 `preview.publish` 使用 post-init read gate。两者共用同一个 manifest，但不能要求尚不存在的 `state/style-contract.json`。read evidence 只允许写入已经通过冻结 identity 校验且 `materializationHash` 精确等于 artifact manifest hash 的 Run；Profile Sync、诊断或其他控制面在 AgentLoop 前执行的 `fs.read` 不得计入 Agent required-read，也不得提前把 style contract 标记为 verified。

### 6.3 Source 语义索引

扩展现有 `DesignSourceIndexSection`，新增：

```json
{
  "id": "section-components-button",
  "heading": "Buttons",
  "purpose": ["component-behavior", "token-evidence"],
  "priority": "required",
  "recipeIds": ["button.primary"],
  "requiredByRuleIds": ["button-focus-visible"],
  "sha256": "..."
}
```

这不会降低当前 raw source 的权限控制：`profile_only` 仍不可读取 raw source；`source_fallback` 仍要求 imported、verified artifact，并继续受字节预算和 index-only 大文件规则限制。

### 6.4 Runtime 能力与工具增补

本方案不以“给模型更多自由工具”为目标。DCP 的编译、hash 校验、required-read 与发布裁决必须保留为 Runtime 内部能力；agent 只获得完成具体工作所需、输入受限且可审计的工具。

| 分类 | 能力 / 工具 | 处理方式 | 边界与产物 |
|---|---|---|---|
| 扩展现有内部能力 | `design_context_read_gate` | 拆为 bootstrap / post-init 两阶段 | 复用同一 manifest；返回缺失 artifact、阶段与建议动作，不把 gate 判断交给 Prompt |
| 强化 Runtime 启动恢复 | `ProjectInitRecoveryCoordinator` | 启动时逐条恢复未终态 transaction；不可达旧 sandbox 转为 typed recovery state，不拖垮整个 Runtime | 保留 journal、旧 binding、checkpoint、失败原因与建议动作；禁止以删除 PVC/journal 作为恢复策略 |
| 强化 Runtime verifier | `VerifierRegistry` + browser collector runtime | 同时绑定/probe Chromium executable、Node collector executable 与真实 headless launch；能力快照冻结到 Run | 最终镜像必须交付两种 executable；enforced 缺任一 capability 在 StartRun fail closed；不注册任意 browser evaluate 工具 |
| 新增 Runtime 内部事务 | `ProfileTokenSyncService` | 控制面确认后、agent loop 前执行三方 token sync | 复用 style contract/tokenFile parser；记录三方 snapshot hash、决议与 before/after；不注册为 model tool |
| 扩展现有发布门禁 | `preview.publish` fidelity pipeline | 对 required recipe 统一执行 token、DOM、computed-style、ARIA/a11y、viewport verifier | 发布结果写入 rule/recipe id、selector、severity、evidence 与仅指向 `appRoot` 的 repair context |
| 新增只读诊断工具 | `diagnostics.accessibility` | 输出结构化 a11y finding | 仅检查 Runtime 内部 preview；输出 rule id、严重度、selector、证据，不执行任意页面脚本 |
| 新增受限预览审计工具 | `preview.audit_responsive` | 使用 Runtime 固定 viewport matrix（首期 375 / 768 / 1440） | 检查横向溢出、关键 CTA/导航、recipe 声明的布局断点；保存每个 viewport 的截图和结构化 finding |
| 新增只读状态工具 | `design_context.status` | 汇总 DCP hashes、read status、当前 gate 与 capability/fidelity outcome | 供 agent、Run detail 和诊断复用；不可泄露 raw source 或扩大 profile visibility |
| 强化证据聚合与校验 | RC/canary evidence collector + validator | 从 Runtime、Sandbox、browser worker、policy 与 Provider 记录聚合机器证据 | validator 只验证已有证据，不能生成结论；缺 digest、run id、approval 或观察窗时 fail closed |
| 后续能力，非本期阻塞 | image/reference artifact + `design_reference.inspect` | 支持不可变的截图/视觉稿引用 | 仅在多模态链路与权限模型完成后开放；不得把图片原始内容转为可执行指令 |

`style.update_tokens`、`design_source.read_sections`、`project.init`、`preview.publish` 继续是核心复用点：不新增 CSS 自由写入或 source 复制工具。Profile sync 复用 `style.update_tokens` 的底层解析/写入 helper，但从 sandbox tool registry 隔离。尤其不增加通用 `browser.evaluate`：它会扩大任意脚本执行、预览数据读取和安全审计面。

发布门禁与诊断工具的关系必须明确：`preview.publish` 是唯一有权阻断 required 规则的裁决点；`diagnostics.accessibility`、`preview.audit_responsive` 和 `design_context.status` 只提供复现、定位与修复证据，不能绕过或覆盖发布结论。

## 7. Prompt 组装

### 7.1 目标

将 `system_prompt_for_run` 从“phase 大字符串”演化为一个单一的 `PromptContextAssembler`。它保持现有 ModelRequest 接口，但以可测试的 section 列表输出：

```text
1. Runtime policy / untrusted-reference boundary
2. phase workflow
3. run identity + profile/DCP snapshot
4. bootstrap / post-init DCP read instruction
5. source-fallback instruction（仅适用时）
6. fidelity repair instruction
```

Profile、tokens、recipe 仍由 sandbox files 读取，不把整份 JSON/CSS inline 到每一 turn 的 system prompt。

### 7.2 Website Build 指令变更

Build Prompt 需新增明确但短小的约束：

```text
Before project.init, read the bootstrap-required files listed in state/design-context-manifest.json. After project.init, read the actual state/style-contract.json before mutation or publish.
Treat inputs/component-recipes.json as the required/preferred component reference inventory; new components are allowed only under the declared extension policy.
Use only token names declared by state/style-contract.json through style.update_tokens.
Do not copy raw source or create a second token system. If a required recipe is unavailable,
report the capability gap instead of inventing a substitute.
```

`design-profile-usage.md` 承担详细读序和 recipe-specific guidance，防止 system prompt 无限增长。

### 7.3 可观测性

每个 ModelRequest / run checkpoint 保存以下 redacted metadata，不保存用户原文或完整 Profile 内容：

```json
{
  "promptSections": ["runtime_policy", "build_workflow", "dcp_read_gate"],
  "designContextContentHash": "...",
  "requiredReadPaths": ["inputs/component-recipes.json", "state/style-contract.json"],
  "readPaths": ["..."],
  "readSectionHashes": ["..."]
}
```

这能在回归失败时判断是“上下文没生成”“模型没读”“模板不支持”还是“读了但没有遵守”。

## 8. Website Craft Packs

### 8.1 定位

Craft pack 是 template-aware、可复用、低于 Profile 的规则集合；它不定义品牌 token，也不能覆盖 `DesignProfile`。每项 required rule 必须明确对应 DOM、computed style 或浏览器 a11y verifier；无法机检的规则只可作为 Review warning。本期内置：

| Pack | 硬规则示例 | 适用面 |
|---|---|---|
| `accessibility-baseline` | focus-visible、语义按钮/链接、图片 alt、色彩对比 | 所有 Website |
| `responsive-layout` | 小屏栅格、overflow、触控目标、导航折叠 | Astro Website |
| `form-states` | default/focus/error/disabled/loading 状态 | 包含表单时 |
| `anti-generic-ui` | 限制默认渐变、重复卡片、无来源的 accent/emoji | 仅 warning，不可单独阻断发布 |

### 8.2 执行方式

- compiler 根据 Profile 声明、template 与 Brief 内容选择 pack。
- pack 的 required rule 被编译入 `component-recipes.json`、`design-profile-usage.md` 和 fidelity rule set，并带有 verifier kind/selector/property 或 a11y rule id。
- 不满足 `accessibility-baseline` / `form-states` 的 required rule 时由 `preview.publish` 阻断；`anti-generic-ui` 首期只形成 review finding 和 telemetry，避免把主观审美伪装为硬安全门禁。

### 8.3 页面编排指导

`layoutGuidance` 解决“组件正确但整页仍缺乏设计秩序”的问题。它只描述 hero 信息层级、CTA 数量、section 节奏、内容密度、导航折叠和移动端优先级；由 Brief 决定页面具体内容与信息架构。它默认是 preferred，除非 Profile 已提供可验证的 signature rule，避免把创作约束成固定 landing page 模板。

## 9. 模板能力模型

能力模型必须扩展既有 `TemplateSpec.style` 与 `TemplateCapabilities`，而不是引入平行的 `TemplateDesignCapabilities` 真相源。当前 token 支持能力应直接来自注册模板的 `StyleContractSpec.tokens`；现有 `design_profile_capability_gaps` 中的硬编码列表需要迁移到这个统一来源。

扩展后的能力信息至少应包含：

```rust
pub struct TemplateCapabilities {
    // Retain existing fields; add DCP-relevant declarations here.
    pub supported_runtime_token_keys: Vec<String>,
    pub supported_component_roles: Vec<String>,
    pub supported_states: BTreeMap<String, Vec<String>>,
    pub supported_craft_packs: Vec<String>,
    pub supported_verifier_kinds: Vec<String>,
    pub global_css_file: String,
}
```

在 StartRun 编译时将 Profile recipe/token 与这个统一 template contract 进行 diff。对 required recipe，还必须验证它声明的 verifier 能否在模板的 preview/runtime 中执行：

- 未支持但非 required：写入 warning / capability report。
- 未支持且 required：进入 `needs_user_input:design_profile_capability_gap`。
- 已支持：生成 precise repair context，供 `preview.publish` 的 fidelity failure 修复使用。

此机制应复用现有 `design_profile_capability_gaps` 与 fidelity report，而非平行实现一套 gate。

模板支持与 Runtime 可用性必须分开判断：`TemplateCapabilities` 回答某 selector/组件是否可验证；`VerificationEnvironmentBinding` 回答 browser worker、a11y engine、viewport matrix 是否在本次运行环境可用且版本满足 `VerificationPolicySnapshot`。effective enforced Run 缺任一 required verifier 时，StartRun 返回 `blocked:design_verification_unavailable`；enforcement flag 关闭或 `legacy_observe` 时只记录 warning，避免存量项目在灰度时被突然阻断。

Runtime 可用性的唯一真相源是新增的 `VerifierRegistry`，不能由 DCP compiler 或工具临时读取环境变量猜测。`RuntimeConfig` 分别从 `RUNTIME_BROWSER_EXECUTABLE` 与 `RUNTIME_BROWSER_COLLECTOR_EXECUTABLE` 读取 Chromium 和 collector runtime 路径，并作为实例配置注入 lifecycle。Runtime 镜像必须同时交付 `/usr/bin/chromium` 与固定 Node runtime；仅有脚本源码不构成 collector capability。StartRun 前的 probe 必须同时完成三项检查：Chromium `--version`、Node `--version`，以及使用与生产 collector 一致安全参数的真实 headless `about:blank` 启动；只做版本探测不能证明浏览器在 non-root、`allowPrivilegeEscalation=false`、capabilities drop-all、`RuntimeDefault` seccomp 的 Pod 中可运行。当前容器以 `--no-sandbox` 启动 Chromium，安全边界由 Pod 隔离策略承担；该取舍必须保留在镜像、部署清单和威胁模型评审中。

probe 通过后生成稳定排序的 capability snapshot/hash，并把 browser/collector executable、worker/version detail 写入 `VerificationEnvironmentBinding`。effective enforced Run 只能使用该 Run 已绑定的 executable，不能在 worker/collector 丢失后静默回退到另一份本机 Chrome/Node，也不得为 fixture/test 修改进程全局环境来影响其他并发 Run。若任一 runtime 在 Run 启动后失效，collector 必须返回带截断、脱敏 stderr 的启动错误，`preview.publish` 返回 `design_verification_runtime_lost` 并保留同一 candidate，不得把环境故障记录为页面 fidelity failure。registry version、snapshot hash 与实际 engine version 进入 Run Binding/evidence，不进入 DCP content hash。本轮 deployed audit 的 capability snapshot hash 为 `129fa6f5b8469b1c74fd65fb61e6110f7ad7e77d7a08499ca86f2c364da10323`。

## 10. API、BFF 与 UI

### 10.1 Runtime API

本期不新增 `POST /runs` 字段。Run 查询响应增加只读字段：

```json
{
  "designContextPackageVersion": "design-context@1",
  "designContextContentHash": "...",
  "designContextBriefHash": "...",
  "designContextVerificationPolicyId": "...",
  "designContextExpectedAppRoot": "project",
  "designContextDeclaredEnforcementMode": "enforced",
  "designContextEffectiveCompatibilityMode": "observe",
  "designContextResolvedTokenSnapshotHash": "...",
  "designContextStyleContractIdentityHash": "...",
  "designContextVerifierCapabilitySnapshotHash": "...",
  "designContextMaterializationHash": "...",
  "designContextArtifactManifestHash": "..."
}
```

新增只读诊断路由。授权首先要求调用者具备 Run 所属 project 的 read 权限；随后对冻结快照执行身份校验：Profile artifact 若声明 `scope.projectId`，必须与 Run project 一致。历史 Run 不依赖当前 Profile store 中该 revision 仍为 active/存在，避免归档或解绑破坏审计可读性；但当前 Profile 状态也不得放宽冻结快照的 project scope：

```text
GET /runs/{runId}/design-context-manifest
GET /runs/{runId}/design-context-diagnostics
```

manifest 响应只返回 artifact path/kind/bytes/hash 与 package/policy/version 摘要；read status 位于 diagnostics。HTTP diagnostics 返回 required/read/missing 集合、gate、materialization、style-contract 状态、verifier capability availability，以及绑定 Run 最新 fidelity outcome 的最小披露摘要。fidelity 摘要只允许 bounded rule/recipe/kind/route/selector/property/viewport、passed/reason、受控 current/target summary、required failed ids 和规范化为 workspace-relative 的 repair target/instruction；不返回完整 report、preview URL、raw actual/source/HTML、绝对 workspace path、browser executable、完整 environment、runtime token mapping 或 token secret。Agent 修复流程仍通过 Runtime-owned report 和 `diagnostics.accessibility` / `preview.audit_responsive` 获取按类型过滤的执行证据，浏览器摘要不得替代工具 gate。

HTTP manifest/diagnostics 与 sandbox `design_context.status` 必须共用同一冻结身份校验：重算 manifest payload hash 与 artifact-manifest hash，逐项核对冻结 artifact 的 path/bytes/SHA-256，验证 effective Profile artifact 的 id/version/hash/project scope，并核对 Run 保存的 base/effective Profile、surface/template、DCP/Brief/policy/appRoot/mode/warnings 字段。任一漂移均 fail closed；HTTP 返回冲突，tool 返回 typed integrity error，不允许以部分可读状态继续。

所有同步路由必须使用 Runtime 从认证上下文解析出的授权主体；不得接受、信任或回显客户端提交的 `principalId`。plan 和 confirm 均要求 project write 权限，且在读取历史 Run 前先校验该 Run 所属 project 的权限与该 Run 冻结 Profile 的可见性。错误响应应使用稳定机器码（至少包括 `profile_sync_precondition_failed`、`profile_sync_operation_expired`、`profile_sync_plan_mismatch`、`profile_sync_conflict_decision_required`、`idempotency_key_reused`、`profile_sync_recovery_required`），并且不得返回 token 原文、raw source 或内部 workspace 路径。

实现使用兼容的可选 `errorCode` 字段承载这些机器码，保留既有 `error` 文本；BFF 透传 `RuntimeApiError.payload.errorCode`。真实 Router fixture 已验证格式非法的浏览器输入由 BFF 返回 400，而 Runtime 的有效 plan hash 不匹配与 confirm 幂等键复用分别以 `profile_sync_plan_mismatch`、`idempotency_key_reused` 返回 409。

Profile 同步使用控制面专用路由，不改变 `POST /runs` 的 request shape：

```text
POST /runs/{runId}/design-profile-sync-plan
POST /runs/{runId}/design-profile-sync-operations/{operationId}/confirm
```

plan request 必须显式固定目标与并发前提：

```json
{
  "targetDesignProfileId": "dp_...",
  "targetDesignProfileVersion": 4,
  "targetEffectiveProfileHash": "...",
  "expectedSourceContentHash": "...",
  "idempotencyKey": "client-generated-stable-key"
}
```

plan response 返回 `operationId`、状态、过期时间、冻结 contract identity、base/current/target snapshot hash、逐 token diff/conflict 与 `planHash`。相同主体、source run、target hash、expected source hash、idempotency key 必须返回同一未过期 operation；source DCP、style contract identity 或当前 token snapshot 已变化时返回 `profile_sync_precondition_failed`，不生成过期 plan。对可展示的 diff，只返回 token 名、状态和经权限审查后的值摘要；默认不把完整 token 值写入通用 Run diagnostics 或事件流。

confirm request 必须绑定不可变 plan，并携带所有 conflict 决议：

```json
{
  "planHash": "...",
  "conflictDecisions": {
    "color.primary": "keep-current",
    "radius.control": "apply-target"
  },
  "idempotencyKey": "client-generated-confirm-key"
}
```

confirm 需要具备项目编辑权限的用户确认。Runtime 必须在同一逻辑事务边界内持久化“confirmed + child-run intent”，再进入 workspace 写入；它校验 operation 未过期/未消费、plan hash、全部 conflict 决议、current token snapshot hash、冻结 contract identity 和 target Profile hash。重复 confirm 只有在 confirm idempotency key、plan hash 与决议完全相同时才幂等返回同一 child run；否则拒绝。`RunLifecycleService` 在模型启动前消费 operation 并执行 token sync；operation 的授权主体、目标 Profile hash、三方 snapshot、决议、过期、消费状态与 before/after 均需审计。若 workspace 写入与审计持久化无法处于同一数据库事务，必须按 5.2 的 saga 状态恢复，不得把“创建 child run 成功”视为“token 已同步”。

### 10.2 Web BFF

Build 页面不必新增复杂的 profile 选择器才可受益：项目 binding 已能驱动 DCP。本期 BFF 已实现并必须保持以下边界：

```text
GET  /api/projects/{projectId}/runs/{runId}/design-context
POST /api/projects/{projectId}/runs/{runId}/profile-sync
POST /api/projects/{projectId}/runs/{runId}/profile-sync/{operationId}/confirm
```

- 三个 BFF 路由都先以本地 project/run ownership 拒绝越权请求，再以 Runtime principal 调用对应的 public Runtime 路由；只读路由使用 `project.read`，plan/confirm 使用 `project.write`。
- confirm 返回 `childRunId` 时，BFF 必须在响应前以当前 product project 记录该 child Edit Run ownership；该写入按 run id 幂等。否则 ProjectShell 切换到 child Run 后，自己的 events/design-context 路由会返回 404，表现为 DCP 卡片消失和事件流断开。
- `design-context` 将 Runtime manifest 与 diagnostics 合并，并从当前 project binding 解析唯一的 active sync target（profile id/version/effective hash）。浏览器不自行计算 effective hash，也不提交 principal id；`profile-sync` BFF 只接受幂等键，必须在服务器端从冻结 DCP 和当前 binding 派生 target/source 条件后再调用 Runtime plan。
- ProjectShell 显示冻结 Profile、DCP/materialization、style-contract、required-read 与最新 fidelity outcome。失败 outcome 必须链接到稳定 rule/recipe detail，并显示 element、viewport、property、受控 current/target summary 和 workspace-relative repair context；没有 report 时明确显示 `NOT RUN`，不得把缺失解释为通过。同步操作先调用 plan，展示受控的 token 状态摘要；每个 `conflict` token 必须选择 `keep_current` 或 `apply_target`，确认后才创建 child Edit Run。界面不展示原始 token 值，不提供“全部覆盖”默认动作。
- BFF 目前**不**向 Build/Edit 的 `POST /runs` 注入 `designProfileId` 或 `designFidelityMode`；同步目标是当前 project binding，但 Runtime 仍对目标可见性、source DCP hash、style contract 与 token snapshot 重新校验。

`test:design-context-bff` 仍是 **BFF contract smoke**：它使用 mock Runtime 验证 owner/run 关系、浏览器不能指定同步目标、BFF 只转发五个 plan 字段、confirm 的代理形状，以及 confirm 后 child Run 已进入 ownership 表并可读取 DCP。它不能单独报告为“Profile Sync 已端到端通过”。

同一 smoke 支持只用于本地浏览器回归的长驻 fixture；该模式会模拟 Profile revision 漂移、一个 `color.primary` 冲突和 terminal child Run event：

```bash
BFF_SMOKE_HOLD_OPEN=1 \
BFF_SMOKE_PROFILE_DRIFT=1 \
BFF_SMOKE_PROFILE_CONFLICT=1 \
npm --prefix apps/web run test:design-context-bff
```

2026-07-15 的独立 mock-Runtime 浏览器操作已断言：DCP READY/materialized/style verified 可见；漂移时出现“三方 diff”入口；未选择冲突决议时 confirm 被 UI 拦截；选择“应用 Profile 值”后可确认；child Run terminal event 到达后 DCP 卡片继续可读，浏览器 console 无 error/warn。该历史记录仍只证明 UI/BFF 边界；下面的联合 L4 才证明相同浏览器路径能够连接真实 Router，但两者都不能替代部署 canary。

真实 L4 fixture 已落在 `services/runtime/tests/http_api/cases/bff_runtime_e2e.rs`，执行命令为：

```bash
cargo test --manifest-path services/runtime/Cargo.toml --test http_api \
  bff_profile_sync_bff_to_real_runtime -- --ignored --nocapture
```

该用例启动真实 `http_api::router_with_state`、Next BFF 与无额外 npm 依赖的 headless Chrome/CDP worker；test-only seed 仅在 Router 外层建立 Brief、冻结 DCP、binding 和 tokenFile，浏览器的 plan/confirm 始终经过 ProjectShell → BFF → 公开 Runtime HTTP 路由。seed 直接复用 compiler 输出的 `inputs/template-style-contract.json` 和 `artifactManifestHash`，只把不参与 identity 的 tokenFile 路径转换为 HTTP 边界允许的 workspace-relative 形式；不得手写弱化的 contract 或先把 `verified=true` 当作验证替身。已断言：

1. BFF 创建的 project/run ownership 与 Runtime principal 的 project id 一致；
2. 无冲突 plan → confirm 后，实际 tokenFile 发生预期变更，且仅创建一个 child Edit Run；
3. 人工变更一个 token 后，真实浏览器能看到 READY/materialized/verified 的源 DCP，以及失败 fidelity 的 rule/recipe、element、375px viewport、current/target 与 bounded repair context；Profile 漂移入口与三方 diff 可用，缺失决议在 UI 内被阻断，选择 `apply_target` 后才能确认；
4. 相同 confirm 请求重放返回同一 child Run；格式非法的 `planHash` 由 BFF 返回 400，有效但不同的 `planHash` 与不同 confirm 幂等键均由 Runtime 返回 409；
5. confirm 后浏览器继续读取 child DCP ownership/alignment；新 child 在 Agent required reads/style-contract verification 完成前显示 `READ REQUIRED` / `pending`，shared/BFF 接受 Runtime 的 `verified=null`，不得伪装为 verified 或错误返回 400；
6. 测试输出机器可读 checks 与最终页面 screenshot SHA-256，并由 Rust 侧解析该证据、回读三组 tokenFile 与 child Run 唯一性；正式 canary 仍须补 operation id、plan hash、before/after hash、截图 URI/digest 和 Runtime/BFF image digest 到第 15.1 节证据包。

该 L4 的签收边界是“浏览器状态机 + BFF + Profile Sync Runtime 控制面”，不宣称 fixture child Edit 的模型执行、candidate 或 publish 成功；这些由 L3、真实 Provider 与部署 RC 分别证明。截图 SHA-256 只证明本次浏览器输出的完整性，不能替代可回看的截图 URI、像素级视觉验收或可访问性报告。

真实冻结 contract 的 `appRoot=/workspace/project` 与 manifest 的 `expectedAppRoot=project` 是同一工作区位置。Runtime 只允许这一前缀规范化后相等；不会因此放宽 template、schema、token mapping、CSS variable 唯一性或 token value 校验。plan 在读取实际 contract/tokenFile 后重新获取 Run 状态，实际 contract identity 漂移会在 plan 阶段返回 409，不会先给出可确认操作再延迟到 confirm。该规则已有定向 lib/HTTP 回归，并由上述 L4 用真实编译产物覆盖。

上述 fixture 不应通过给生产 Runtime 增加 seed/debug API 实现；测试可在 Router 外层挂载仅测试进程可达的 fixture handler，或直接在同一测试进程预置 `RuntimeStore`。

后续若扩展 API 再增加：

- 未来若 Build 请求需要显式传 `designProfileId` 或 `designFidelityMode`，必须另立 API-freeze 变更并补齐兼容矩阵；它不属于本期，也不能借由 BFF 私自扩展当前 `POST /runs` contract。

### 10.3 安全边界

- DCP files 保持 untrusted design reference 语义；绝不允许 source/recipe 文本授予工具、权限、路径或网络能力。
- `context-index.json` 的读取只能经 allowlisted `design_source.read_sections` 或受控 fs.read。
- DCP manifest / run diagnostics 避免回传 content source、完整 source、token secret 或用户附件正文。

## 11. 分阶段实施计划

### Phase 0：契约与纯函数（1 个 PR）

状态：`implemented`。后续变更只能作为 schema/version 迁移，不得重新定义现有 hash、排序或 manifest 边界。

交付：

- `design_context` module：DCP content/run binding types、canonical hash、compiler、usage renderer、component recipe validator。
- 固定无自引用 payload/artifact/outer manifest、`briefHash`、`expectedAppRoot`、craft-pack/verifier policy version、legacy_observe/enforced 兼容矩阵与 canonical serialization；明确实际 worker version 仅进入 Run Binding/evidence。
- 定义两个独立开关：`RUNTIME_DESIGN_CONTEXT_PACKAGE_V1` 控制 DCP 编译/materialization，`RUNTIME_DESIGN_CONTEXT_ENFORCEMENT_V1` 控制新增 required-read/a11y/responsive 阻断；两者初始均默认关闭，后者不能在 Phase 3 验证完成前开启。enforcement 为 true 仍不足以让任意 Run 进入 enforced：`RUNTIME_DESIGN_CONTEXT_ENFORCEMENT_ALLOWLIST_JSON` 必须提供精确 `{projectId,designProfileId,designProfileVersion}` 条目；空名单或未匹配条目一律生成 observe DCP，条目字段/JSON 非法则在 Runtime startup 校验失败。
- 将 DCP capability 声明并入既有 `TemplateSpec`；删除/迁移 capability gap 的硬编码 token 列表。
- fixture：手工 Profile、imported Profile、包含 required recipe 的 Profile、能力缺口 Profile。
- 纯单测：相同输入 content hash 稳定；run id/timestamp 不影响 content hash；Brief/policy/version/surface/template override 均影响 hash；非法 recipe/token/verification 拒绝；Profile 缺少新字段进入 legacy_observe。

签收条件：无 Runtime 行为改变；所有 DCP compiler test green，并记录 schema/hash golden fixture。该条件只证明纯函数正确，不能作为 read-gate 或 enforcement 的发布依据。

### Phase 1：Build materialization 与 snapshot（1 个 PR）

状态：`provider-verified`；核心 Website lifecycle 已在 clean RC 完成部署签收，并在同一 candidate 由真实 Provider 完成 Build/Edit/Repair required-read、fidelity 与 publish 遵从性；下一层门禁为真实 canary。

交付：

- StartRun Build 时先 compile DCP payload/expected artifact manifest 并写入内容身份字段；sandbox materialize 校验成功后再持久化 Run Binding/materialization hash，禁止在落盘前伪造 binding。
- 接入 `VerifierRegistry` 启动 probe 与 capability snapshot；在启动模型前解析 `VerificationEnvironmentBinding`。
- 只在 master flag 开启的内部流量编译/materialize DCP；enforcement flag 在本阶段强制保持关闭，Profile 声明的 enforced 记录为 declared mode，但 effective mode 为 observe，不新增 publish/read 阻断。
- workspace 写出 usage、component recipes、context index、template style contract、manifest。
- prebuild 检查 content hash、artifact manifest hash、落盘 materialization hash。
- run progress/evidence 增加 DCP 摘要。

签收条件：两个 flag 默认关闭时现有 Website Profile Build 的 workspace/prompt/行为保持兼容；仅 master flag 开启时 run 可读到 manifest/hash，但不会因新增 verifier/gate 被阻断。HTTP/MockModel 驱动真实 AgentLoop/Node build 已证明 Build→Edit→Repair 的 DCP materialization/inheritance；持久化 Store restart 后 Edit 也已证明 Runtime source snapshot、冻结 DCP/source snapshot/binding 恢复到独立 workspace。

### Phase 2：读取门禁与 Prompt assembler（1–2 个 PR）

状态：`provider-verified`；读取门禁与 Prompt assembler 已进入 clean RC，同候选真实模型的 enforced DCP Website mutation/publish/Repair E2E 已签收；下一层门禁为真实 canary。

交付：

- 将 `system_prompt_for_run` 内部重构为 section assembler，不变更外部 request。
- `design_context_read_gate` 按 DCP manifest 工作；拆为 project.init 前 bootstrap gate 和 init 后 mutation gate。
- 增加只读 `design_context.status`，供 Run detail、agent repair loop 与诊断复用同一份 hash/read/gate 事实。
- source index 增加 purpose/priority/recipe references。
- 新增 run diagnostics route。

签收条件：未读 recipes 或 style contract 的 mutation 被拒绝；读取后可继续；source fallback 大文件仍需 exact required sections。style contract identity 比较由共享 parser 提供：Build 的 `project.init` 验证初始化结果，Edit/Repair 在成功读取 `state/style-contract.json` 时将实际内容与冻结的 `inputs/template-style-contract.json` 比较并持久化 `verified=true|false`；三种 phase 的后续 mutation/publish 都要求 `verified=true`。sandbox DCP fixture 与 HTTP/MockModel 驱动的真实 AgentLoop/Node build 已证明 Build 的“读取 → `project.init` → 实际 style contract 读取 → token/source mutation → publish”，继承 Edit 的重新 materialize/read → source patch → publish，以及从 review finding 发起的 Repair 重新 materialize/read → source patch → publish；source_fallback 覆盖 small source 的受控 raw Build read，以及 large source 的 Build/Edit/Repair index + exact required section read，均保持 artifact 不变。enforced fixture 已补齐 pass、required failure→report read→source patch/rebuild→新 candidate promotion，以及 worker-lost candidate retention/no-promotion。持久化 Store restart + 独立 workspace fixture 已覆盖 source snapshot、冻结 DCP/Profile/Brief identity 与 binding 的恢复和重新 materialize/read。同候选真实 Provider 又证明 Build/Edit/Repair required reads、源码变更、publish、新 source snapshot、finding fixed 与 clean deployed RC 身份链路；尚未覆盖任意崩溃时序或 browser worker 跨主机恢复，这些不得由当前 recovery matrix 过度外推。

### Phase 3：Fidelity 与 craft packs（1–2 个 PR）

状态：`provider-verified`（控制面、工具链、成功/修复/worker-lost fixture，以及 clean fresh-cluster 的真实 Provider enforced Build→Edit→Repair、fixture-seeded Review、真实 headless capability、Runtime restart、项目 Pod egress、供应链 preflight 与机器证据校验均已通过）。

交付：

- `accessibility-baseline`、`responsive-layout` 初版。
- 新增控制面 `ProfileSyncOperation` 与 Runtime 内部 `ProfileTokenSyncService` 三方 diff/apply 事务；从实际 tokenFile 读取 current，冲突项必须逐项决议，并在 agent loop 前完成写入与 audit evidence。
- 先交付 shared style-contract/tokenFile parser 与纯三方 merge planner；再接入 operation 状态机、幂等 confirm、apply recovery 和 RunLifecycle，避免控制面事务与 CSS 解析同时变更而无法定位失败边界。
- 将 `preview.publish` 扩展为 required a11y/ARIA 与固定 viewport matrix 的最终裁决；新增只读 `diagnostics.accessibility`、`preview.audit_responsive` 作为修复证据入口。
- capability/fidelity report 关联 recipe/rule ids、实际 verifier 结果与 repair context。
- preview.publish 对 required rule 形成真实 source mutation 后才允许重试成功。
- enforcement flag 仍默认关闭；只在专用 fixture/canary 中显式开启并验证 enforced/worker unavailable/runtime lost 语义。

实现顺序与不可合并条件：先完成共享 parser + planner 的纯函数/状态机测试，再落地授权 plan route，然后落地 confirm 的 durable child-run intent 与 RunLifecycle apply/recovery，最后接入 a11y/viewport verifier 和 `preview.publish` 阻断。缺少前一项时不得以 UI 或 feature flag 掩盖后一项的失败；尤其不得在 confirm route 尚未验证幂等/recovery 前开放“同步到最新 Profile”按钮。

签收条件：enforcement flag 开启的 fixture 可稳定触发并修复至少一个 token、组件 state、响应式/a11y失败；flag 关闭时只产生 observe finding，不改变发布结论。该 fixture 必须由实际 `preview.publish` 生成完整 evidence，且至少覆盖一次 worker-lost 后 candidate 保留与重试；没有这些证据不得进入 canary。

### Phase 4：BFF/UI 与 rollout（1 个 PR）

状态：BFF 控制面与 UI 交互均为 `verified-fixture`；真实 Next、真实 Runtime Router 与 headless Chrome 已形成同次 L4 证据，rollout 仍为 `planned`，因为部署 canary 的 image/config identity、观察窗和回退记录尚未产生。

交付：

- 已完成：Build/Edit Run detail 的 DCP 卡片显示冻结 Profile、DCP/materialization、style-contract、required-read 与 fidelity outcome；失败卡片锚定具体 rule/recipe，并显示 element、viewport、current/target 与 bounded repair context。BFF 已代理 manifest/diagnostics 及 sync plan/confirm，且复用项目 ownership 与 Runtime principal。
- 已完成：明确的“同步到当前 project binding 的最新 active revision”操作；同步经控制面 `sync plan` 的三方 diff、逐 token 用户确认和短时 operation id 后 apply，不得从自然语言猜测同步意图。
- 保持：本期不支持 BFF 私自覆盖 Build/Edit 的 profile/fidelity 字段。若需显式 profile/fidelity 覆盖，必须另立 API-freeze 与兼容矩阵，而不是把它混入同步入口。
- 分别灰度 master/enforcement flag：先为 Website Build 开启 DCP observe，再按 Profile/project allowlist 开启 enforcement；不得一次性把两个默认值同时切到 on。**本期已实现配置型与持久化精确 allowlist**（`projectId + designProfileId + designProfileVersion`，空名单 fail closed 为 observe）。持久化 policy 使用 `PUT /internal/projects/{projectId}/design-context-enforcement`，需要 internal-admin 授权与 `expectedRevision` CAS；可用 `enabled=false` 显式覆盖环境名单并安全回退，JSONL snapshot 在重启后恢复。Build DCP 与 sync target 会复用同一判断；Build audit 记录 allow/observe、policy revision、操作者与 DCP hash。真实 canary、指标阈值与回退演练未完成前，enforcement 仍只能在隔离 fixture/环境中验证。Phase 1 已同步发出简洁的 DCP 摘要 progress evidence，供非 UI 用户核对。

退出条件分为两段，且 BFF 控制面两段均已满足：

1. 已完成的 BFF contract smoke：`npm --prefix apps/web run test:design-context-bff` 覆盖 project/run 越权、manifest/diagnostics 读取、服务器端派生 plan 与 confirm。
2. 已完成的真实 Runtime 联合 E2E：`bff_runtime_e2e` 按第 10.2 节六项断言同时启动真实 Next、真实 Runtime Router 和 headless Chrome，覆盖无冲突 sync、浏览器冲突逐项决议、重复 confirm、token 回读、child Run 唯一性/ownership 与 screenshot digest；它将 BFF 控制面和浏览器路径共同升级为 `verified-fixture`。该测试被标为 ignored，只是为了避免每次普通 Rust 回归都启动 Next dev/Chrome；发布前证据包必须显式执行它。
3. 已完成的独立 mock 浏览器 fixture 仍作为快速 UI 回归保留，但不再承担真实 Router 的证据职责；联合 L4 关闭“UI 与真实 Runtime 从未同次运行”的本地缺口，仍不关闭部署 canary。

此外，master 关闭时 Run 行为、sandbox 输入和既有持久化语义必须兼容当前 Profile/Design Capsule 路径；master on/enforcement off 与 master on/enforcement on 的 E2E 均须通过。新增 observability 字段不承诺字节级输出一致。

#### Phase 4 rollout 的硬前置与顺序

1. **先完成 allowlist 的持久化控制面，而非先改默认值**：已完成：internal-admin 通过 CAS 写入并持久化 `projectId + profileId/revision` policy，重启可恢复，`enabled=false` 能覆盖环境名单回退；Build 审计记录 `runId`、DCP hash、policy revision 与操作者。发布前仍须在真实 canary 中证明策略变更/回退与指标告警的联动，而不是只依赖单测。
2. **先拿到 fixture 闭环，再进入 canary**：同一 Runtime、同一 Website candidate 必须覆盖：正常 Build、从 promoted version 发起的普通 Edit DCP 继承、Repair child 继承、无冲突 sync apply、人工 token conflict 决议、重复 confirm、worker-lost、restart 后 operation recovery，以及 source mutation 后 publish。BFF 测试必须连接该 Runtime，而不是只断言 mock request。任一用例失败，回到 `implemented`，不得开始真实项目灰度。
3. **按固定档位推进**：

| 档位 | flag / policy | 最小样本与观察窗 | 放行条件 |
|---|---|---|---|
| A：observe fixture | `master=on,enforcement=off` | 至少 20 次隔离 Website run，覆盖 Build/Edit/Repair | DCP materialization/read-gate telemetry 完整；不引入无 DCP 路径失败 |
| B：enforced fixture | `master=on,enforcement=on`，仅 1 个 fixture 三元组 | 至少 20 次 run，含故意失败与修复 | 100% 记录 verifier/evidence；所有预期阻断均可复现且修复后通过 |
| C：单一真实项目 | 精确 policy 仅 1 个 project/profile/revision | 连续 7 天且不少于 30 次 publish | 满足下表阈值，并完成 policy disable 回退演练 |
| D：小范围扩展 | 逐个新增精确三元组，不改变全局默认值 | 每批不超过 5 个项目、至少 7 天 | 上一批稳定，且没有未处置 `recovery_required` |

`off/on` 必须在启动期拒绝；`off/off` 是随时可执行的全局回退组合。不得通过扩大环境 allowlist 代替增加精确持久化 policy，也不得在档位 C/D 中修改 DCP schema 或 verifier policy。

4. **以可判定阈值放行或回退**：以下为首轮保守阈值；在进入档位 C 前可基于档位 A/B 的基线调整一次，但调整必须留下决策记录，不能在观察窗内移动门槛。

| 信号 | 放行阈值（每个观察窗） | 立即动作 |
|---|---|---|
| `design_verification_runtime_lost`（enforced） | `0` | 对受影响三元组写入 `enabled=false`，保留 candidate/evidence 并排查 worker |
| `design_context_verifier_unavailable_total`（enforced） | `0` | 停止扩大范围；若发生在真实项目，关闭该项目 enforcement |
| 未预期的 `design_context.read_required` 或 `style_contract_unverified` | `0` 个用户可见失败 | 暂停该档位，依据 diagnostics 修复 workflow/Prompt 后从 fixture 重跑 |
| Profile sync `recovery_required` | `0` 个未在 24 小时内处置 | 停止新增 policy；恢复/人工处置并补齐审计后才可继续 |
| required finding 修复闭环 | 100% 的预期 fixture failure 能在同一 candidate lineage 修复并通过 | 回到 Phase 3，禁止将 finding 降级为 warning 绕过 |
| DCP 引入的 publish 失败率 | 相对 `master=off` 基线增加不超过 2 个百分点，且逐例可解释 | 关闭受影响 policy；若无明确归因，关闭 enforcement |

5. **回滚剧本必须可演练**：

   1. 记录当前 `runId`、`operationId`、DCP/materialization hash、policy revision、verifier snapshot、candidate/evidence URI；不得先清理现场。
   2. 对受影响精确三元组以 internal-admin CAS 写入 `enabled=false`；读取回 policy revision，确认新 Build/sync target 进入 observe。
   3. 如故障跨多个三元组，关闭 `RUNTIME_DESIGN_CONTEXT_ENFORCEMENT_V1` 并重启 Runtime；`master` 保持开启以保留观察证据。
   4. 只有确认 enforcement off 后仍存在 DCP read gate 或不可消费 operation 时，才关闭 `RUNTIME_DESIGN_CONTEXT_PACKAGE_V1`；关闭前先导出未终态 operation 清单，不能以回滚丢弃 saga 恢复证据。
   5. 用无 Profile Website、legacy Edit 和 Docs 各跑一条回归；补充回滚时间、操作者、结果与后续修复归因。该演练至少在档位 C 前完成一次。

6. **每一档保留证据包**：按 15.1 保存实际 run/operation id、DCP/materialization hash、worker/version、截图/evidence URI、发布结果、阈值计算和回退记录；不以 dashboard 截图或模型说明替代。

## 12. 测试与验收矩阵

| 场景 | 断言 |
|---|---|
| 无 Profile Website Build | 保持现有行为；不写 DCP files、不新增 read gate |
| 手工 active Profile | Brief/profile/template/policy 相同才生成相同 content hash；默认 `profile_only`；bootstrap / post-init 读取完成后可发布 |
| Brief 或 verifier policy 改变 | content hash 必须改变；诊断只显示 hash/version，不泄露 Brief 正文 |
| Hash 自引用防护 | payload 不含 content hash；artifact manifest 不含 outer manifest；重建顺序得到稳定 content/artifact/materialization hash |
| 非默认 appRoot | 新 Build 的 `project.init(path != expectedAppRoot)` 在写文件前返回 `project.app_root_mismatch`；省略 path 稳定使用 `project` |
| Master/enforcement flag 矩阵 | HTTP flag matrix 已验证 off/off 保持无 DCP；on/off materialize + observe；on/on 在空 allowlist 时 fail-closed 为 observe，精确 allowlist 命中再由 enforced verifier preflight 决定是否启用新增 gate；off/on 配置启动失败 |
| 存量 Profile 缺少 `websiteContext` | master on 进入 `legacy_observe`，只产生 warning，不新增 publish 阻断；显式升级且 enforcement flag/allowlist 开启后才启用 required verifier |
| imported active Profile | 默认 `source_fallback`；source hash/index/required section 受现有保护 |
| imported source 的真实生命周期 | HTTP/MockModel 对 small source 的 Build 必须先读取 DCP bootstrap 与已验证 `inputs/design-source.md`；对 large source 的 Build/Edit/Repair 必须重新读取 `design-source-index.json` 并经 `design_source.read_sections` 读取 exact required section，不能 direct-read raw source；两者才能 init/mutate/publish，均记录 source hash 且保持 immutable artifact bytes 不变 |
| Profile surface/template override | manifest 中 surface/template/effective hash 正确；recipe 反映 override |
| unsupported optional token/recipe | Run 可继续，fidelity report warning 可见 |
| unsupported required recipe | sandbox mutation 前进入 capability gap 状态 |
| Build 未读 recipe | `project.init` 前缺 bootstrap read 被拒绝；init 后缺实际 style contract read 时 mutation/publish 被拒绝 |
| Edit/Repair style contract 漂移 | 每个继承/恢复 Run 清空旧 verification；读取实际 `state/style-contract.json` 后与冻结 template contract identity 比较，任一 parse/identity drift 写入 `verified=false`，后续 mutation/publish 以 `design_context.style_contract_unverified` fail closed |
| Edit 普通内容修改 | 不自动重置 token；只读取 edit 最小集合 |
| Edit DCP 继承 | `baseVersionId` 必须解析到同 project 的 `createdByRunId`；若该 Run 有有效冻结 DCP，Edit 复制 DCP/Profile/Brief identity 和 artifacts，但清空 materialization、read、style-contract evidence，并以当前 verifier snapshot 在恢复 workspace 后重新 materialize/read；无 DCP 的历史 version 保持 legacy Edit 行为 |
| Edit DCP 继承的真实生命周期 | HTTP `POST /runs` 已回归 `prepare_run_context → baseVersion source run 校验 → DCP inherit → workspace restore`；随后 `continue` 进入真实 AgentLoop，重新 materialize DCP，读取 Edit 所需 profile/usage/recipes，并对恢复的页面执行真实 source patch、Node build、`preview.publish` 和新版本晋升。用例断言 DCP/Profile/Brief/artifacts 继承、workspace evidence 清空及 source snapshot 恢复；持久化 Store restart 后也验证 source run/version/binding/DCP 恢复且继续完成 read pass；另有 master 开启下历史无 DCP version 保持 legacy Edit 的回归。跨 project `baseVersionId` 在创建 Run 前以 409 拒绝 |
| Edit 请求同步 Profile | 控制面 plan 从 parent DCP/实际 tokenFile/new DCP 得到 base/current/target；只有用户 confirm 后才创建 child run，RunLifecycle 在 agent loop 前 apply；旧 run 仍可 replay |
| Sync 与人工 token 修改冲突 | `current != base && current != target` 的 token 标记 conflict；没有逐项决议时 confirm 被拒绝，任何决议都有 audit evidence；模型不可见 operation id |
| Sync 目标移除 token | 标记 `not_managed`，不删除 CSS variable；必须走显式 style-contract migration，不能由 sync 静默删除 |
| Sync 时 style contract 漂移 | template/schema/token mapping、CSS variable 缺失/重复或 tokenFile 非法任一变化均返回 `profile_sync_precondition_failed`，不会猜测写入位置；appRoot 仅把冻结 contract 的 `/workspace/<root>` 与 manifest 的 `<root>` 视为同一规范位置，其他漂移仍拒绝 |
| Sync 并发与重复确认 | current snapshot 改变时 precondition failed；相同 idempotency key/planHash 的重复 plan/confirm 返回同一 operation/child run，不重复写 token |
| Web BFF / UI sync | 非 owner 不能读取或 plan/confirm；浏览器只能使用 BFF 提供的 binding target/hash；UI 必须逐项决议全部 conflict 后才发送 confirm，且不会展示 raw token value 或静默覆盖。`test:design-context-bff` 负责快速 BFF 边界；联合 L4 同时启动真实浏览器、Next 与 Runtime Router，覆盖 DCP READY/materialized/verified、fidelity failed rule/recipe/element/viewport/current-target/repair context、漂移入口、冲突 plan、缺失决议拦截、显式 apply-target、confirm 后 child DCP 的 READ REQUIRED/pending 三态、真实 token 写回、重复 confirm 的 child Run 唯一性、格式非法 `planHash` 的 400、有效但不同 `planHash`/confirm 幂等键的 409，以及最终 screenshot digest。仍须补部署 canary 的 operation/token/image identity 与观察窗 |
| Run fidelity outcome 最小披露 | 绑定 Run 无 report 时返回 `fidelity=null` 并由 UI 显示 NOT RUN；有 report 时只返回 latest status、bounded rule/recipe/finding、受控 actual/expected summary 与 workspace-relative repair context。不得返回 raw actual/HTML/source、preview URL、完整 designContext/environment、browser executable、token mapping 或绝对 workspace path；真实 browser-backed report 与恶意 metadata fixture 均有 HTTP 回归 |
| Sync apply 中断恢复 | 已回归写入后、完成前进入 `recovery_required` 的 crash window：重启 Store 后若 tokenFile 已等于确认目标，只补齐 operation/audit 收敛为 `Applied`，不重复写入且复用同一 child Run；其余 current/contract 漂移仍 fail closed，不得新建 plan 覆盖 |
| Diagnostics 权限与完整性 | manifest/diagnostics 无凭证返回 401，跨 project 或错误 owner 返回 403；具备 Run project read 权限且冻结 Profile scope 合法时返回 200。Profile revision 不在当前 store 也不影响历史 Run 审计读取；冻结 Profile scope 与 Run project 不一致的新 DCP 在 attach 前拒绝，遗留/持久化的 manifest/artifact/Profile/Run identity 漂移由 HTTP 映射为 409、`design_context.status` 返回 typed integrity failure。响应只能包含冻结摘要/availability，不得含 raw Profile/source/Brief、runtime token mapping、browser executable 或完整 verifier environment；sync plan 另需 project write 权限与自己的 precondition 校验 |
| DCP read evidence provenance | 只有冻结 identity 有效且已 materialize 的 Run 才能记录 required-read/style-contract evidence；Profile Sync/diagnostics/control-plane `fs.read` 不计入 Agent 读取。materialization hash 非 artifact manifest hash、materialize 前 read、style verified 无 materialization 均 fail closed |
| Repair child 冻结身份 | Review/Repair child 继承父 Run 的 Profile revision、effective hash、DCP artifacts/source mode，StartRun 不得按当前 active binding 重新 attach；只有显式 Profile Sync operation 可以生成新 target DCP |
| required a11y failure | `preview.publish` 阻断并输出 rule id/severity/selector；`diagnostics.accessibility` 可复现同一 finding，不能改变发布结论 |
| required responsive failure | 375 / 768 / 1440 viewport matrix 中任一 required 断言失败即阻断；产物包含每个 viewport 的截图 hash 与 finding |
| browser worker unavailable | effective enforced Run 在 StartRun 返回 `blocked:design_verification_unavailable`，不启动模型；observe mode 记录 warning，不得在 publish 时才意外失败 |
| collector runtime 缺失或 Chromium 无法真实启动 | VerifierRegistry 同时 probe 固定 Node collector 与真实 headless Chromium；仅 `--version` 成功不得宣告 computed-style/a11y/viewport available；enforced StartRun fail closed |
| browser worker 启动后丢失 | publish 返回 `design_verification_runtime_lost` 并保留 candidate；不生成页面 fidelity failure，不静默放行 |
| verifier/viewport policy 版本变化 | 新 DCP content hash 改变；旧 run replay 仍使用原 policy snapshot，并记录当时实际 worker evidence |
| stale project-init journal 启动恢复 | 不可达 sandbox/DNS/connection error 只隔离对应 Run，保留 journal 并写 typed `recovery_required` 审计/事件；健康 Run 继续恢复且 Runtime readiness 不被单条坏 journal 阻断 |
| rollout allowlist 与回退 | 空 allowlist 不允许 enforcement；未命中 project/profile 只能 observe；命中、拒绝、变更和回退均有审计。关闭 enforcement/master 后不得遗留 DCP read gate 或不可消费 sync operation |
| policy 优先级与重启 | 精确持久化 policy 优先于环境 allowlist；`enabled=false` 必须降为 observe；CAS 冲突拒绝且重启后仍读取相同 revision/操作者 |
| Fidelity required failure | 修复只发生在 `appRoot` 下、被页面导入的源码；imported raw design source 的内容/hash 不变；无 app source 变化的重试不能通过 |
| 同一 candidate lineage 的 DCP 闭环 | sandbox tool-chain 与 HTTP/MockModel 驱动的真实 AgentLoop 均覆盖 DCP materialize、bootstrap reads、`project.init`、实际 style-contract read、`style.update_tokens`、appRoot 源码 mutation 与 `preview.publish`；HTTP fixture 还断言 Build→继承 Edit→review finding Repair 的 Node build output、candidate/promotion、DCP hash/read/fidelity evidence，并覆盖 small raw 与 large indexed imported source；large source 对 Build/Edit/Repair 均重新读取 index + required section。同候选 clean Provider RC 又覆盖真实 Build/Edit/Repair mutation、fixture-seeded Review、finding fixed、worker/restart、preflight/egress/auth、image/config identity 与 release validator；技术签收已关闭，下一层只接受 canary 证据 |
| 可演练回退 | 对单一精确 policy 写入 `enabled=false` 后，新 Run/sync target 均为 observe；关闭 enforcement/master 后分别验证无 Profile Website、legacy Edit、Docs 不发生新增阻断；未终态 operation 仍可审计/恢复 |
| 恶意 source 文本 | 不改变工具权限、路径、网络能力；仅作为 untrusted design reference |

除 Rust 单测外，必须扩展现有 Website HTTP lifecycle 与 design-profile fidelity matrix：

- `services/runtime/scripts/run-design-profile-fidelity-matrix.mjs`
- `services/runtime/scripts/run-design-md-website-http-e2e.mjs`
- `services/runtime/tests/astro_build_agent.rs`
- `services/runtime/tests/edit_agent.rs`

测试分层必须与发布结论对应，避免将快测试的通过率当成真实灰度依据：

| 层级 | 最低执行面 | 允许证明的结论 |
|---|---|---|
| L1：纯函数 / lib | compiler、hash、policy precedence、sync merge、style parser | schema、确定性、状态机局部正确 |
| L2：Runtime HTTP / sandbox | route auth/CAS、materialization、read gate、publish/fidelity tool | 接口与 Runtime 边界正确 |
| L3：真实 Runtime Website fixture | AgentLoop、真实 workspace、browser worker、candidate lineage | 可进入 `verified-fixture` |
| L4：真实浏览器→BFF→真实 Runtime | DCP/fidelity UI 状态与规则详情、冲突决议、ownership、plan/confirm、写入回读、重复 confirm、截图 digest | Web 控制面与浏览器路径可和 Runtime 一起签收为 `verified-fixture` |
| L5：canary | 精确 policy、指标、告警与回退演练 | 可进入 `canary-verified` |

L1/L2 可以并行并快速回归；L3/L4/L5 必须串行保留证据。任何 L3+ 失败都不能通过增加 mock 或放宽 assertion 来关闭。

## 13. 可观测性与成功指标

首期的**目标指标**只记录低基数、非内容字段。下表定义指标后端最终需要聚合的语义；它不应被误读为每一个名称都已经接入生产指标系统：

| 指标 | 含义 |
|---|---|
| `design_context_package_compiled_total` | compiler 成功/失败及原因 |
| `design_context_required_read_block_total` | 哪个 phase/tool 因缺读被阻断 |
| `design_context_capability_gap_total` | required/optional capability gap 数量 |
| `design_context_fidelity_pass_rate` | 首次 publish 与 repair 后 publish 的 pass rate |
| `design_context_recipe_rule_fail_total` | 各 recipe/rule 的 failure 分布 |
| `design_context_source_sections_read` | source fallback 的 section 读取数量与字节数 |
| `design_context_a11y_required_fail_total` | required a11y finding，按稳定 rule id/severity 聚合 |
| `design_context_responsive_required_fail_total` | required responsive finding，按 viewport preset/rule id 聚合 |
| `design_context_profile_sync_total` | token sync 的 plan、confirm、apply、rejected 原因数量 |
| `design_context_verifier_unavailable_total` | enforced/observe 模式下缺失 verifier 的数量与原因 |

当前 Runtime 已实现上述十类**可审计 Run 事件出口**。compiler 事件记录 passed/failed 与稳定 reason；公共 DCP metric helper 固定补齐 `mode=observe|enforced`、`surface=website` 与 `phase`；各决策点只追加低基数维度：read gate 为稳定 reason/tool 与缺失计数，capability 为 required/optional 与 gap kind，source 为 raw/indexed、section 数与 bytes，fidelity 为 passed/failed 和 initial/repair，rule failure 为稳定 rule/recipe/kind/priority，a11y/responsive 为 rule/severity 或固定 viewport，Profile Sync 为 plan/confirm/apply 的 status 与机器 error code，verifier unavailable 为固定 capability 枚举或 `runtime_lost`。`design_context_fidelity_pass_rate` 的事件值是一次判定样本；internal-admin canary metrics export 现按明确分母从持久化事件聚合 rate，并用 Run-frozen persistent policy binding 将 observe/enforced 归入精确 release cohort。导出器只接受这十个固定 metric name，并拒绝与 frozen Run mode/surface 不一致的事件，避免异常持久化记录扩大 label 或泄露内容。

事件明确不包含缺失文件列表、workspace/source 路径、artifact/operation/request id、Profile 名称、token 名称/值、Prompt、用户内容、错误正文、browser executable 或完整 verifier environment。HTTP/工具/browser fixture 已覆盖 plan precondition drift、confirm plan mismatch、initial fidelity failure→repair pass、required a11y/responsive failure、worker runtime-lost、indexed source read、required read block 与 capability gap；完整本地 harness 已通过。Runtime durable-store 聚合、阈值告警判定、不可覆盖的采集 fragment、一页报告生成器和签名 HTTPS paging dispatcher 已实现并回归，但真实值班目的地尚未绑定/验收，部署后的每日采集、observe warning 的人工运营归因和真实 7 天观察窗仍未发生；这些运行项完成前不得把“聚合能力已实现”写成“生产监控已通过”。

建议比较 master/enforcement flag 矩阵的 Website fixture：

- 首次 `preview.publish` 通过率；
- required fidelity rule 的平均 repair 次数；
- token/recipe 未读取导致的工具阻断率；
- 组件状态、a11y 与 responsive 类 finding 数量；
- prompt/context 体积与 Build 时间。

不在 metrics label 中放 Profile 名、Prompt、文件路径全文、用户内容或 request id。

### 13.1 指标语义、归因与告警

生产指标后端中的每个计数必须带 `mode=observe|enforced`、`surface=website`、稳定 `reason/ruleId`（如适用）和 release cohort；不得以高基数 run id 作为 metric label。上述当前 Run 事件只承载各自已实现的最小元数据，允许采集层补齐固定维度；run/operation id 只进入受控审计事件，用于从聚合指标回查。分母必须明确：

| 指标 / 结论 | 分母与计算 | 主要归因源 | 告警级别 |
|---|---|---|---|
| publish 失败率增量 | `DCP 导致的失败 publish / 所有 publish`，与同 cohort 的 `master=off` 基线对比 | `preview.publish` verdict、DCP mode、fidelity reason | 超过 2pp：停止扩大；无明确归因：关闭 enforcement |
| verifier 不可用 / 丢失 | `unavailable 或 runtime_lost / enforced run` | VerifierRegistry health、publish error | 任意非零：页面并升级，关闭受影响精确 policy |
| read gate 误阻断 | `非预期 read_required 或 style_contract_unverified / DCP run` | design-context diagnostics、tool error | 任意用户可见：暂停本档位 |
| sync 恢复债务 | `超过 24h 未终态 recovery_required operation` | durable operation store | 非零：禁止新增 allowlist policy |
| required finding 修复率 | `同一 candidate lineage repair 后 pass / 预期 required failure` | rule/evidence/candidate lineage | 小于 100%：回到 fixture 修复，不扩大 |

观察窗开始前必须登记 cohort、基线时间段、Runtime/image/config digest、policy revision 和阈值版本；观察窗结束后生成一页结论：样本数、每项阈值、实际值、异常 run/operation id、是否执行回滚及决策人。没有该结论页不得把 `canary-verified` 写入发布记录。

## 14. 风险与缓解

| 风险 | 缓解 |
|---|---|
| DCP 让 Profile 变成第二个 source of truth | 明确 `design-profile.json` 是唯一 source；DCP content/run binding 分离、可确定性重建，记录 compiler version/hash |
| content/manifest hash 自引用或落盘后不可重建 | 分离 payload、artifact manifest、outer manifest；固定 artifact → artifact hash → payload → content hash → outer manifest 顺序，outer manifest 永不参与自身 hash |
| `project.init` 前无法确定带路径的 style contract | StartRun 固定 `expectedAppRoot=project`；bootstrap/actual contract 使用同一冻结值，project.init 在写文件前拒绝 path mismatch |
| Context 文件增加导致模型不读 | tool-layer required-read gate；Prompt 只提示 manifest，不复述全部内容 |
| 组件 recipe 过度约束创作 | 区分 required/optional；只有 template 支持且 Profile 标为 required 时才阻断 |
| 直接引入 CSS token 复制破坏模板 | 所有 token change 继续只能走 style contract/style.update_tokens |
| Profile sync 覆盖用户已完成的 Edit | BFF/control plane 创建带授权/过期的 operation；current 从实际 tokenFile 读取；contract identity 漂移 fail closed；RunLifecycle 在 agent loop 前执行三方 sync，冲突 token 必须逐项决议、并发变更 fail closed 并审计 |
| Sync 在写入与审计之间崩溃 | operation 记录 durable apply intent，进入 `recovery_required`；基于同一 plan 做幂等读回/审计恢复，未恢复前 child run 不可启动 |
| a11y/responsive gate 被环境差异误判 | VerifierRegistry 提供健康 capability snapshot；policy 固定 ruleset/viewport matrix；Run Binding 记录 engine version 与截图/document hash；effective enforced 缺 worker 在 StartRun 明确 blocked，observe 仅告警 |
| 存量 Profile 灰度时行为突变 | 缺 `websiteContext` 固定为 legacy_observe；只有 declared enforced 且 enforcement rollout 允许时才新增 required gate，并覆盖 master/enforcement flag 矩阵回归 |
| 新 browser 工具扩大攻击面 | 不提供任意 `browser.evaluate`；诊断只访问 Runtime preview，输入与输出均为 allowlisted、结构化数据 |
| Chromium 在受限 Pod 中依赖 `--no-sandbox` | 明确 Pod 是隔离边界：non-root、禁止提权、drop all capabilities、RuntimeDefault seccomp、受控 preview URL 与结构化 collector；任何放宽 Pod securityContext 或开放任意 URL/evaluate 都必须重新安全评审 |
| collector 脚本存在但解释器未进入最终镜像 | Node 使用 immutable digest 的独立镜像 stage 复制到最终 Runtime，构建期执行版本检查，VerifierRegistry 真实 probe，Run Binding 冻结 browser/collector executable；缺任一项 enforced fail closed |
| Sandbox 绕过 npm proxy 直连公网 | `SandboxTemplate.networkPolicyManagement=Unmanaged`，避免 controller 默认生成公网 allow policy；仓库默认拒绝 egress，仅允许 DNS、Runtime channel 与批准的 npm proxy；网络检查必须在 Runtime 项目 Pod 仍绑定时执行并连同 Pod UID/IP 写入 `npm-proxy-gate@2`，不能对任意 warm Pod 或仅凭 lockfile/node_modules 判定 |
| PVC helper 绕过镜像锁 | K3s local-path-provisioner 自带的 BusyBox helper 也进入 `images.lock.json`；任何 Sandbox PVC 创建前重写 `helperPod.yaml` 为 `ref@digest` 并重启/探测 provisioner |
| source 内容提示注入 | 保持 untrusted reference 标签、allowlisted read、权限不从 content 派生 |
| Build/Edit 行为漂移 | master/enforcement 双开关从 Phase 1 就存在；先 observe 后 enforce；无 Profile 路径不变，每 phase 有独立回退开关 |
| Prompt assembler 重构引入回归 | section-level snapshot tests；同一 run 入口只允许一个 assembler |

## 15. 最终验收（Definition of Done）

本方案完成的标准不是“多生成了几个 context 文件”，而是同时满足：

1. Website Build 对 active Profile 生成并冻结无自引用的 payload/artifact/outer manifest 与 Run Binding；Profile、canonical Brief、template、policy 相同才可确定性重建 content hash，run 时间与实际 worker version 不影响内容 hash。
2. StartRun 固定 `expectedAppRoot=project`；模型在 project.init 前被 Runtime 强制读取 bootstrap context，project.init 拒绝其他 path；Build 由 init 验证实际 style contract，Edit/Repair 由实际读取重新验证冻结 identity，三种 phase 在 mutation/publish 前都必须 `verified=true`。
3. required recipe/token 与模板能力不兼容时，在 sandbox mutation 前给出具体 capability gap。
4. `preview.publish` 能把失败关联到可执行的 Profile rule / recipe verifier / source evidence，并给出只指向 appRoot 源码的 repair context。
5. VerifierRegistry 提供可审计 capability snapshot；effective enforced Run 所需 verifier 不可用时在 StartRun 被明确 blocked，required a11y/ARIA 与 responsive rule 在固定 policy/Runtime preview 环境中可复现，`preview.publish` 是唯一阻断点。
6. Profile token 同步由控制面显式发起，base 来自 parent DCP、current 来自实际 tokenFile、target 来自 new DCP；contract identity 漂移或 token parser 歧义时 fail closed。RunLifecycle 在 agent loop 前按用户冲突决议以可恢复 saga apply，重复确认幂等，普通 Edit 和 agent 均不得隐式重置或删除 token。
7. master/enforcement flag 的 off/off、on/off、on/on 行为均有证明；缺少 `websiteContext` 的存量 Profile 固定走 legacy_observe，只有 declared enforced 且 enforcement rollout 允许时才启用 required gate。
8. `profile_only` 与 `source_fallback` 的原有安全边界不退化。
9. 无 Profile、Docs、现有 Build/Edit run 的兼容性有回归证明。
10. Run 详情能解释本次 Website 实际使用的 design context package、hash、policy/version、required/read status、token sync 与 fidelity outcome；失败 outcome 可定位 rule/recipe、element、viewport、current/target 与 bounded repair context，未运行时明确显示 `NOT RUN`。
11. 同一真实 Website candidate 已证明从 DCP materialization、required reads、token 或源码修改、required finding 阻断/修复到 `preview.publish` 通过的闭环；该证据不能由手工创建 `dist`、mock Runtime 或分散单测替代。
12. Web BFF 已在同一 `bff_runtime_e2e` 中由真实浏览器连接真实 Runtime Router，完成 fidelity failure detail、Profile Sync 的无冲突写入回读、冲突逐项决议、缺失决议拒绝、重复 confirm 幂等、child DCP 回读与 screenshot digest；Rust 侧解析浏览器证据并回读 tokenFile 与唯一 child Run。mock-Runtime smoke 不能替代此项；canary 仍须保存 operation id、plan hash、tokenFile before/after hash、截图 URI/digest 与 Runtime/BFF image digest。
13. 至少完成档位 C 的精确 policy canary：达到第 13.1 节阈值、保留指标结论页，并成功演练一次 `enabled=false` 精确回退。达到此前，任何 “release-ready” 或默认开启 enforcement 的结论都无效。

### 15.1 发布前证据包

每次准备开启 enforcement allowlist 时，必须附带可审计的最小证据包，而不只是 PR 的单测摘要：

- `cargo fmt --check`、DCP/profile-sync 相关单测、HTTP contract-manifest 测试与 Website lifecycle 测试的命令、版本与结果；
- 一条 persistent policy `enabled=false` 的 observe Website run，以及将同一精确 project/Profile revision CAS 更新为 `enabled=true` 后的 enforced Website run；分别记录前后 policy revision、updatedBy、DCP content/materialization hash、verifier capability snapshot 与发布结论；
- 至少一个 required a11y 或 responsive failure 从阻断、修复到通过的同一 candidate lineage；
- 至少一个 profile sync 的无冲突 apply、一个人工 token 冲突确认、一个并发快照失配拒绝，以及一个 `recovery_required` 恢复/人工处置证据；
- 一条真实浏览器→BFF→真实 Runtime 的 Profile Sync 联调记录：DCP/style-contract 三态、BFF project/run ownership、principal project id、operation id、plan hash、tokenFile before/after hash、冲突决议、唯一 child Run id，以及最终页面 screenshot URI/SHA-256；
- 无 Profile Website、Docs Build、普通 Edit/Repair 的回归结果，以及 flag 回退后不遗留新的 read gate 或 mutation 阻断的证明。
- 未跳过的供应链 preflight、Runtime/Sandbox/browser/collector/local-path helper image manifest/config digest，以及 Sandbox 只能经批准 npm proxy 获取 tarball、无法直连 public registry 的网络证据；网络证据必须在项目 Pod 绑定期间采集并包含 project id、Pod UID/IP、lockfile hash 与 proxy tarball request，任意 warm-pool Pod 的事后探测不被接受。
- 部署 RC 的 `release-evidence@1` 必须保留 `enforcedDcpFixture.designContextEnforced` 的 Build/Edit/Repair 三阶段 diagnostics、`reviewRepair` 的 Review/Repair/finding lineage，以及三阶段相同的 content/materialization hash；finding 必须来自该 Review 且为 `fixed`。`validate-release-evidence.mjs` 对任一缺失、run id 复用、hash 漂移或 finding 未修复均 fail closed。
- Website/Docs 的 release artifact 必须在 Sandbox 释放后用 fresh、project-scoped principal 读取并记录 `artifactAccessAfterRelease`；只记录匿名 `401` 或只验证释放前 URL 均不得通过。内容断言必须绑定实际发布路由：Website 为 `/`，Docs 为 `/docs/`，同时保存 content hash 与 computed-style 结果。
- Provider Build 与 Edit 必须使用不同的验收内容，且 Edit 的源码 mutation 必须发生在首次 `preview.publish` 之前；不得通过放宽 candidate freeze 来兼容提前 publish。长 Provider 等待后的所有 project API 与 artifact probe 必须刷新短时 principal，证据中不得保存 JWT 或 Provider secret。
- 一次 canary 结论页：cohort、观察窗、样本数、Runtime/image/config digest、policy revision、全部第 13.1 阈值的实际值、异常项归因与精确 policy 回退演练记录。
- 将该结论页整理为不含凭证、token 原文或 raw source 的 `design-context-canary-evidence@1` JSON，并通过第 15.2 节校验器；校验失败即不得申请扩大 allowlist。

以上证据必须按 run/operation id、hash 和稳定 rule id 关联；不可用 Prompt 文本、截图主观判断或“模型自述已修复”替代。

部署 RC 的机器证据回归入口为：

```bash
node services/runtime/scripts/test-release-evidence-validator.mjs
```

该命令本身只证明校验器对有效/缺 Repair/未 fixed finding 等 fixture 的判定语义。2026-07-15 的同候选 clean Provider release RC 已产生 `releaseEligible=true` 的 `release-evidence.json` 并通过 `validate-release-evidence.mjs`，因此批次 C、D 均已构成对应层级的机器证据；它仍不能替代批次 E canary。

### 15.2 DCP canary 证据门禁

`services/runtime/scripts/validate-design-context-canary-evidence.mjs` 是对第 15.1 节的机器可读补充，不能生成或伪造证据。它只接受已由真实环境采集的 JSON，并 fail closed 校验以下不可替代项：

- 经批准的真实 Provider/model/approval reference；
- 精确 `projectId + designProfileId + version + observePolicyRevision + policyRevision(enforced)` cohort，以及 Runtime/BFF 的 image manifest/config digest；
- 同一 Website 的 observe 与 enforced 成功 Run，包含 DCP content/materialization/verifier snapshot hash、worker provider/version、required-read 与 publish verdict；
- required finding 的“阻断 → repair → 新 candidate promotion”链路；
- clean apply、冲突显式决议、plan mismatch 拒绝和 `recovery_required` 复用 child Run 的 Profile Sync 证据；
- BFF→Runtime 的 project/principal 一致性与 child Run 唯一性；
- 指标观察窗、enforced 样本数、verifier unavailable/runtime-lost 为零、publish 失败率增量不超过 2pp、无告警；
- 对同一精确 policy 写入 `enabled=false` 后的新 Run 为 observe、无新增 read gate，且 operation recovery 审计仍保留；
- 无 Profile Website、Docs Build、legacy Edit/Repair 的兼容回归。

校验器按档位 C 的退出条件硬编码最低观察窗 `10080` 分钟和至少 `30` 次 enforced publish，并用起止时间重新计算窗口；只修改汇总数字不能通过。Run、candidate、operation、child Run、cohort、BFF principal 与 policy rollback 之间均做交叉引用校验，不能用同一条成功记录重复填充多个证据槽位。

真实 canary 完成后执行：

```bash
node services/runtime/scripts/validate-design-context-canary-evidence.mjs \
  /secure/evidence/design-context-canary-evidence.json
```

校验器的 fixture 回归可以在无 Provider 凭证的开发机执行，但它不构成 canary 通过：

```bash
node services/runtime/scripts/test-design-context-canary-evidence-validator.mjs
```

### 15.3 Canary 采集账本与执行方式

只在观察窗结束时手写一份汇总 JSON 不构成真实 Canary 证据。`services/runtime/scripts/design-context-canary-ledger.mjs` 提供 `init / append / status / finalize` 四个命令，将每份来源片段写入受锁保护的 NDJSON hash chain；每条记录包含顺序号、前序 hash、来源 URI、来源文件 SHA-256 与记录 hash。`init`、每次 `append` 与最终 `finalize` 都在返回成功前对对应文件执行同步，且新 ledger/最终 evidence 使用 `wx + 0600`，降低进程已报成功但记录仍未稳定落盘的风险。工具在 append 时拒绝 credential-like 内容和不满足 cohort/Run/Sync/指标/回退语义的片段，finalize 时重新计算基线/Enforced publish 样本数、观察窗分钟数和 failure-rate delta，再调用第 15.2 节校验器。修改历史 payload、顺序、来源 hash 或汇总数字都会 fail closed。

观察窗开始前准备不含凭证的 `design-context-canary-session-config@1`：

```json
{
  "schemaVersion": "design-context-canary-session-config@1",
  "recordedAt": "<observation-start-iso-time>",
  "sourceUri": "<immutable-session-evidence-uri>",
  "provider": {
    "mode": "approved-real",
    "name": "<provider>",
    "model": "<model>",
    "approvalReference": "<approval-reference>",
    "credentialPresent": true
  },
  "cohort": {
    "projectId": "<exact-project>",
    "designProfileId": "<exact-profile>",
    "designProfileVersion": 1,
    "observePolicyRevision": 1,
    "policyRevision": 2,
    "policyUpdatedBy": "<operator>",
    "thresholdVersion": "website-dcp-canary-thresholds@1"
  },
  "images": {
    "runtime": { "ref": "<ref>", "manifestDigest": "sha256:<64-hex>", "configDigest": "sha256:<64-hex>" },
    "bff": { "ref": "<ref>", "manifestDigest": "sha256:<64-hex>", "configDigest": "sha256:<64-hex>" }
  },
  "window": {
    "baselineStartedAt": "<iso-time>",
    "baselineEndedAt": "<iso-time>",
    "observationStartedAt": "<iso-time>"
  }
}
```

初始化后，每个来源文件使用 `design-context-canary-event@1`，顶层固定为 `type + recordedAt + sourceUri + payload`。允许的 type 为：`website.run`、`required-finding.repair`、四类 `profile-sync.*`、`bff-runtime`、`publish.samples`、`metrics.snapshot`、`alert.destination-probe`、`alert.delivery`、`rollback`、`compatibility`。每个 `publish.samples` sample 必须有不同的 `sampleId`、Run、模式、时间、publish verdict、DCP 归因和精确 cohort；它只能由最终 Runtime operational export 一次生成并 append，避免逐日累计导出产生重复 sample。`metrics.snapshot` 只接收 Runtime durable-store 聚合导出的运营计数，不接受人工填写的样本数、观察分钟数或 failure-rate delta。账本允许按日追加多份 metrics snapshot，包括 `alertsTriggered=true` 的失败/进行中记录；每份 snapshot 都必须跟随一条匹配的投递决策。finalize 使用最后一份快照，但历史告警和投递结果不会从 hash chain 消失，最终未达阈值或缺少外部通知证据仍 fail closed。

```bash
node services/runtime/scripts/design-context-canary-ledger.mjs init \
  --ledger /secure/evidence/design-context-canary.ndjson \
  --config /secure/evidence/session-config.json

# 首个 Canary workload 和指标采集前向真实值班目的地发送一次签名 readiness probe；URL/密钥只经环境注入。
CANARY_ALERT_WEBHOOK_URL=<hidden-https-webhook> \
CANARY_ALERT_WEBHOOK_SECRET=<hidden-hmac-secret> \
node services/runtime/scripts/dispatch-design-context-canary-alerts.mjs probe \
  --output /secure/evidence/fragments/alert-destination-probe.json \
  --source-uri <immutable-alert-probe-uri> \
  --destination-id <stable-oncall-destination-id> \
  --operator-id <operator-id>

node services/runtime/scripts/design-context-canary-ledger.mjs append \
  --ledger /secure/evidence/design-context-canary.ndjson \
  --event /secure/evidence/fragments/alert-destination-probe.json

# 每日采集：Runtime 管理面从 durable Run/event/operation 聚合；token 只经环境注入。
RUNTIME_ADMIN_TOKEN=<hidden-admin-token> \
node services/runtime/scripts/collect-design-context-canary-metrics.mjs \
  --base-url <runtime-url> \
  --session /secure/evidence/session-config.json \
  --observation-ended-at <current-iso-time> \
  --conclusion-recorded-by <operator-id> \
  --export-output /secure/evidence/daily/<date>-operational-export.json \
  --metrics-output /secure/evidence/fragments/<date>-metrics.json \
  --metrics-source-uri <immutable-daily-metrics-uri>

node services/runtime/scripts/design-context-canary-ledger.mjs append \
  --ledger /secure/evidence/design-context-canary.ndjson \
  --event /secure/evidence/fragments/<date>-metrics.json

node services/runtime/scripts/render-design-context-canary-report.mjs \
  /secure/evidence/daily/<date>-operational-export.json \
  /secure/evidence/daily/<date>-report.md

# 每份 metrics snapshot 都必须产生对应投递决策；无告警时不调用 webhook，但仍记录 not-required。
CANARY_ALERT_WEBHOOK_URL=<hidden-https-webhook> \
CANARY_ALERT_WEBHOOK_SECRET=<hidden-hmac-secret> \
node services/runtime/scripts/dispatch-design-context-canary-alerts.mjs dispatch \
  --input /secure/evidence/daily/<date>-operational-export.json \
  --output /secure/evidence/fragments/<date>-alert-delivery.json \
  --source-uri <immutable-daily-alert-delivery-uri> \
  --destination-id <stable-oncall-destination-id>

node services/runtime/scripts/design-context-canary-ledger.mjs append \
  --ledger /secure/evidence/design-context-canary.ndjson \
  --event /secure/evidence/fragments/<date>-alert-delivery.json

# 观察窗结束时重新采集一次，并同时生成唯一的最终 publish.samples fragment。
RUNTIME_ADMIN_TOKEN=<hidden-admin-token> \
node services/runtime/scripts/collect-design-context-canary-metrics.mjs \
  --base-url <runtime-url> \
  --session /secure/evidence/session-config.json \
  --observation-ended-at <final-iso-time> \
  --conclusion-recorded-by <operator-id> \
  --export-output /secure/evidence/final-operational-export.json \
  --metrics-output /secure/evidence/fragments/final-metrics.json \
  --metrics-source-uri <immutable-final-metrics-uri> \
  --samples-output /secure/evidence/fragments/final-publish-samples.json \
  --samples-source-uri <immutable-final-publish-samples-uri>

# 同一 generatedAt 下必须先 append samples，再 append 最终 metrics，使后者成为 latest snapshot。
node services/runtime/scripts/design-context-canary-ledger.mjs append \
  --ledger /secure/evidence/design-context-canary.ndjson \
  --event /secure/evidence/fragments/final-publish-samples.json

node services/runtime/scripts/design-context-canary-ledger.mjs append \
  --ledger /secure/evidence/design-context-canary.ndjson \
  --event /secure/evidence/fragments/final-metrics.json

CANARY_ALERT_WEBHOOK_URL=<hidden-https-webhook> \
CANARY_ALERT_WEBHOOK_SECRET=<hidden-hmac-secret> \
node services/runtime/scripts/dispatch-design-context-canary-alerts.mjs dispatch \
  --input /secure/evidence/final-operational-export.json \
  --output /secure/evidence/fragments/final-alert-delivery.json \
  --source-uri <immutable-final-alert-delivery-uri> \
  --destination-id <stable-oncall-destination-id>

node services/runtime/scripts/design-context-canary-ledger.mjs append \
  --ledger /secure/evidence/design-context-canary.ndjson \
  --event /secure/evidence/fragments/final-alert-delivery.json

node services/runtime/scripts/design-context-canary-ledger.mjs append \
  --ledger /secure/evidence/design-context-canary.ndjson \
  --event /secure/evidence/fragments/<event>.json

node services/runtime/scripts/design-context-canary-ledger.mjs status \
  --ledger /secure/evidence/design-context-canary.ndjson
```

此时 `status` 仅用于检查采集进度，**不得先执行 `finalize`**。最终校验要求账本中恰好包含一条真实 rollback event，因此观察窗结束后必须先完成下面的 CAS 回退与 post-rollback Run 验证。

回退必须通过 Runtime CAS 执行，而不是只在 JSON 中把 `enabled` 改为 false。回退工具分两段运行，确保 post-rollback Run 的 `run.started` 晚于 policy disable：

```bash
RUNTIME_ADMIN_TOKEN=<hidden-admin-token> \
node services/runtime/scripts/run-design-context-canary-rollback.mjs disable \
  --base-url <runtime-url> \
  --project-id <project-id> \
  --design-profile-id <profile-id> \
  --design-profile-version <version> \
  --expected-revision <enforced-policy-revision> \
  --updated-by <operator-id> \
  --state /secure/evidence/rollback-state.json

# 通过正常产品/API 路径创建并完成一个新的 Website Run 后：
RUNTIME_PRINCIPAL_TOKEN=<hidden-project-principal> \
node services/runtime/scripts/run-design-context-canary-rollback.mjs verify \
  --base-url <runtime-url> \
  --state /secure/evidence/rollback-state.json \
  --post-run-id <new-run-id> \
  --ledger /secure/evidence/design-context-canary.ndjson \
  --output /secure/evidence/fragments/rollback.json \
  --source-uri <immutable-rollback-evidence-uri>

node services/runtime/scripts/design-context-canary-ledger.mjs append \
  --ledger /secure/evidence/design-context-canary.ndjson \
  --event /secure/evidence/fragments/rollback.json

# rollback 已进入 hash chain 后，才允许生成最终 evidence；缺任一门禁都会 fail closed。
node services/runtime/scripts/design-context-canary-ledger.mjs finalize \
  --ledger /secure/evidence/design-context-canary.ndjson \
  --output /secure/evidence/design-context-canary-evidence.json
```

`disable` 在发出 Runtime CAS 请求前先以 `wx + 0600` 独占预留状态文件；路径已存在时不得调用 Runtime，避免“策略已关闭但审计状态无法落盘”的分叉。请求成功后要求返回同 project/Profile revision、`enabled=false` 且 revision 精确 `+1`，写入后执行文件同步；请求或身份校验失败则清理本次空占位。`verify` 要求新 Run 的 diagnostics 冻结 persistent disabled binding、新 revision、effective observe、read gate ready，并从 ledger hash chain 验证既有 `profile-sync.recovery` 仍保留。Runtime 非成功响应只记录方法、路径和状态码，不回显响应正文；Admin/principal token 只允许经环境注入，不进入命令参数、状态文件、事件或最终 evidence。

`status` 只报告已采集记录、elapsed minutes 与 publish 数，不给出提前通过结论。`finalize` 只有在 7 天、至少 30 次 Enforced publish、全部 required 事件、阈值和回退都成立时才创建新输出文件；输出已存在时拒绝覆盖。账本 hash chain 能发现本地篡改，但不替代外部可信时间戳或不可变存储，因此 session config、每日 fragment、ledger head hash 与最终 JSON 仍须按日复制到受控 evidence store，并使 `sourceUri` 可由审计者回查。

## 16. 剩余交付批次与执行顺序

既有核心实现已进入工作区。批次 A、B 已关闭；批次 C 已由 detached clean SHA 的严格 release RC 关闭；批次 D 已在 detached clean candidate `a0d036f3d8ad` 上完成同候选真实 Provider DCP Build/Edit/Repair、fixture-seeded Review、finding fixed 与 image/config/DCP identity 绑定，证据等级升级为 `provider-verified`。包含 Batch E metrics/paging/ledger/rollback 与 registry plain-EOF 修复的后续 clean candidate `1c0cd85b64db` 已完成 fresh-cluster fixture audit RC 和部署态 metrics route `401/200` 核验；它证明实现可部署，不是 Provider release 或真实 canary。批次 E 的生产观察窗/rollback 尚未开始。剩余工作继续按阻塞依赖执行；前一批未关闭时不得把后一批结果写成发布通过。

| 批次 | 当前状态 | 目标 | 主要改动面 | 退出门禁 |
|---|---|---|---|---|
| A：启动恢复 | **已完成** | 关闭 stale project-init journal 导致 Runtime CrashLoop 的 P0 | project-init transaction recovery、typed error/state、restart tests | 已有坏 journal 隔离单测、审计/事件与同持久卷 Runtime rollout；不得删除失败 journal 来“保持通过” |
| B：供应链与网络恢复 | **已完成，已被批次 C 同次引用** | 恢复不可跳过的 release preflight，并阻断 Sandbox 直连 public npm | Debian/Node/BusyBox immutable lock、完整错误聚合 preflight、exact-digest approved mirror fallback、模板 `Unmanaged`、项目 Pod UID 级网络证据 | clean RC 已保留 lock hash、11/11 prefetch、429 fallback reason、canonical pull 与 egress 结果 |
| C：部署态 enforced RC | **已完成，已被批次 D 同次重放** | 在目标部署形态验证 Runtime/Sandbox/browser worker | RC gate、RBAC、固定 Runtime/Sandbox/browser/collector image、短时 JWT 刷新、artifact project-principal probe、evidence aggregator/validator | clean image 的 enforced Build/Edit/Review/Repair + finding fixed + 8 类 recovery 通过，供应链/egress 未跳过，`releaseEligible=true` |
| D：真实 Provider | **已完成，`provider-verified`** | 证明模型读取并遵守 enforced DCP mutation/repair | exact allowlist、fixture-seeded Review finding、真实 Provider Build/Edit/Repair、批准引用、Website `/` 与 Docs `/docs/` artifact assertion、mutation-before-publish | candidate `a0d036f3d8ad` 已由 release validator 绑定 provider/model/approval、image/config、DCP diagnostics、run/finding lineage、artifact 与 review provenance；允许申请单项目 canary |
| E：单项目 canary | **Runtime 聚合、逐日采集/报告、签名 paging、ledger、policy binding、CAS rollback verifier 与 validator 已完成；clean audit RC 已部署验证 route；真实观察窗待执行** | 验证指标、运营与回退 | exact policy、Run-frozen policy revision、durable-store metrics export、阈值告警、外部目的地 readiness probe、逐 snapshot 投递决策、一页运营报告、hash-chain ledger、来源片段校验、样本重算、两阶段精确回退、交叉引用型 canary evidence validator；candidate `1c0cd85b64db` 已在 fresh k3d 验证 Runtime identity、route 鉴权与 operational-export schema | 从正式分支 commit 重建并生产部署；绑定/验收真实值班目的地；7 天且不少于 30 次真实 enforced publish；阈值通过；`enabled=false` 回退演练通过；机器证据校验无复用/漂移/篡改；零样本 audit export 不计入退出门禁 |

### 16.1 当前工作区的 clean RC 提交边界

主工作区仍是大型未暂存组合变更；文件数会随验证产物与后续修复波动，不作为证据等级。为避免污染用户工作区，Provider RC 使用 alternate index + `git commit-tree` 生成 detached clean 候选 `a0d036f3d8ad9a16343260bd6861ff736b04111b`，并在 `/tmp/zerondesign-dcp-provider-rc-a0d036f3d8ad` 验证；包含其后 Batch E 与 registry 修复的最新实施候选同样通过 alternate index 生成 `1c0cd85b64db6008dd5a746b544d90786a73e192`，并在 `/tmp/zerondesign-dcp-canary-1c0cd85b64db` 完成 clean audit RC。前者仍是最新 Provider release 证据，后者只是最新实现的 fixture deployability 证据；两者都尚未替代正式提交、review 与分支归档。本段之后的文档回写不在 `1c0cd85b64db` 对象内。后续提交仍必须按以下依赖顺序拆分；`conversation.rs`、`types.rs`、`config.rs`、`agent_loop.rs`、`run_lifecycle/start.rs`、`tools/runtime.rs` 和大型 HTTP lifecycle 测试同时承载多个切面，必须按 hunk 归属，不得用整文件暂存把后续能力提前卷入前序提交。

| 顺序 | 建议提交 | 当前改动归属 | 提交前最小验证 |
|---|---|---|---|
| 1 | `fix(runtime): isolate stale project init recovery failures` | `project/workspace_transaction.rs`、`runtime/bootstrap.rs` 及对应 recovery audit/event/test hunk | startup recovery 定向 lib 测试；`cargo fmt --check` |
| 2 | `feat(runtime): compile and freeze Website design context packages` | `design_context.rs`、`style_contract.rs`、template capability、Run/DCP snapshot、贯穿 attach/materialization/mutation/inheritance/sync/fidelity 的共享冻结 identity/artifact/hash 校验、Agent-only read evidence provenance、Repair child frozen identity、Prompt assembler、required-read/source-index、read/capability/source 低基数指标、三项只读诊断工具的注册与 sandbox contract v2、manifest/diagnostics 授权及最小披露回归，以及 `lifecycle_website` 主闭环 | `cargo test --lib`；DCP Build/Edit/Repair 与 project authorization HTTP 定向测试；Profile Sync pre-AgentLoop read 负例；sandbox contract/tools/permission gate |
| 3 | `feat(runtime): add confirmed DesignProfile token sync` | `profile_token_sync.rs`、`run_lifecycle/profile_token_sync.rs`、public/internal sync/policy routes、CAS/idempotency/recovery/errorCode、plan/confirm/apply/rejected 指标、HTTP contract 与 `design_context_sync` | Profile Sync lib/HTTP/recovery；contract manifest |
| 4 | `feat(web): expose DCP diagnostics and confirmed profile sync` | `packages/shared` API/client（含 style-contract null/false/true 三态与 bounded fidelity summary）、三个 Web BFF 路由、ProjectShell DCP/fidelity/Profile Sync 卡片、CSS 与稳定 browser test contract、product DB test override、BFF error mapping、mock smoke，以及真实 Chrome→Next→Runtime 联合 L4 fixture | shared tests/typecheck；Web typecheck/build/smoke；授权最小披露 HTTP；真实浏览器→BFF→真实 Runtime L4 |
| 5 | `feat(runtime): enforce browser-backed DCP fidelity and provider repair evidence` | VerifierRegistry、Chromium/Node collector、style-contract/fidelity/publish gates、fidelity/rule/a11y/responsive/runtime-lost 低基数指标、`diagnostics.accessibility`/`preview.audit_responsive` 的真实 report 语义、browser fixture 的具名 remote-fs boundary、Runtime image、真实 Provider Build/Edit/Repair/finding evidence 与 summary | fidelity/worker-lost HTTP；remote-fs boundary；collector smoke；provider summary fixture |
| 6 | `test(infra): pin supply chain and close deployed DCP RC gates` | `infra/agent-sandbox` lock/preflight/network/RBAC/RC/gateway、release evidence aggregate/validator、k3d/security tests | `PREFETCH_IMAGES=0/1`；sandbox security；release validator；fresh audit RC |
| 7 | `test(runtime): fail closed on incomplete DCP canary evidence` | internal-admin durable-store metrics export、精确 cohort/policy revision 过滤、阈值告警与一页报告、不可覆盖采集器、签名 HTTPS paging/readiness probe、逐 snapshot 投递覆盖、可保留失败快照的 canary hash-chain ledger、来源 credential/shape 校验、样本/时间窗/failure-rate 重算、finalize、7 天/30 publish 与 cohort/Run/operation/rollback 交叉验证、local-gate 接入 | Runtime HTTP + route contract；Node syntax；collector/report/alert dispatcher/ledger end-to-end 与错误 revision/告警/投递失败/篡改/密钥负例；canary validator 正负例；provider/release validator 回归 |
| 8 | `docs(runtime): record Website DCP implementation and rollout gates` | 仅本文档 | Markdown fence/whitespace；文档状态与实际证据逐项核对 |

当前未暂存整树已确认这些切片的测试入口有效：startup recovery `3/3`、DCP/compiler/read gate `15/15`、Profile Sync `10/10`、project authorization `4/4`；Web typecheck/build/BFF smoke、真实浏览器联合 L4、供应链 preflight、provider/release/canary validator 也均已通过。完整 `run-runtime-harness-local-gates.sh` 已在组合状态与前一 clean candidate 上以退出码 `0` 通过：sandbox contract v2、sandbox tools `93 passed / 0 failed / 1 ignored`、preview promotion `20/20`、AgentLoop `29 passed / 0 failed / 1 ignored`、HTTP `110 passed / 0 failed / 3 ignored`、template build `5/5`、shared `30/30`、Fumadocs real build、canary collector/report/alert dispatcher/ledger/rollback/final validator、provider/release validator 与 computed-style smoke 均为 green。最新 `1c0cd85b64db` 相对该 clean candidate 只增加 registry fallback policy/test 与 preflight/harness 接线；其 policy 定向测试、独立真实 preflight、fresh-cluster audit RC、Runtime recovery gate 和部署态 metrics route 均通过，但没有再次声称对该精确 SHA 重跑整套本地 harness。严格 Provider clean RC 继续验证真实 Provider Website/Docs、enforced DCP Repair、preflight/egress/auth/transport/recovery 与 secret scan。它们证明当前组合状态可回归，不证明未来每个 hunk commit 独立可编译；实际提交时仍须在每个暂存切片上重跑对应命令。

默认排除 `2026-07-11-harness-runtime-product-optimization-spec.md` 与 `2026-07-12-design-profile-lifecycle-closure-spec.md`：两者是独立长规格，不属于本次 Website DCP clean RC。`.runtime-evidence/`、`services/runtime/target/` 和部署运行产物继续不进入提交。每次暂存后必须依次执行 `git diff --cached --check`、`git diff --cached --stat`、`git diff --cached --name-only`；发现跨切面 hunk 即停止，不以“后续会修”作为提交通过条件。

每批完成后只更新第 0.3 节对应一行，并在第 15.1 节证据包中追加记录；不要重写历史失败事实。本轮历史链必须同时保留：stale journal CrashLoop、collector 缺 Node、Chromium sandbox 启动失败、Edit style verification 为 null、enforced fixture 缺依赖、public npm 直连失败、PVC helper 未入 lock、Docker Hub 429/EOF、长上下文写入、Repair chunk capability、target finding、stale snapshot promotion 与空修复 completion。严格 RC 阶段新增并关闭的六项也必须保留：长等待后短时 JWT 过期；429 exact-digest fallback；并发 `fs.read` 丢 read lease；匿名 artifact probe 误报 401；Docs route 误查 `/`；Edit 在 mutation 前提前 publish 导致 candidate 冻结。最新 audit 阶段还必须保留：node_modules symlink 被 fingerprint 当目录、CloudFront plain EOF、Debian auth plain EOF、旧分类器漏识别 plain EOF，以及 8 GiB Docker Desktop 下约 42 分钟 release build。批次 C 已由 clean 同次证据关闭；批次 D 已在 candidate `a0d036f3d8ad` 合并真实 Provider 与 enforced DCP Repair，升级为 `provider-verified`；`1c0cd85b64db` 只升级最新实现的 clean deployability 证据，只有批次 E 真实通过后才可将 rollout 升级为 `canary-verified`。
