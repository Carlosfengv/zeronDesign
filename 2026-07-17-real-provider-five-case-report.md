# Website / Docs 五个真实 Provider 用例报告

> 报告性质：真实回归与修复验证；连续三批完整五案例 5/5 accepted
> 证据版本：历史批次 schema v1；最新批次 schema v2
> 最近更新：2026-07-18

## 1. 结论

2026-07-18 已使用真实 DeepSeek `deepseek-v4-pro` 和普通用户式 Prompt 连续完成三批
完整五案例回归：`suite-20260718015800609-accepted`、
`suite-20260718021539164-accepted`、`suite-20260718023350592-accepted`。每批均为
Website 3/3、Docs 2/2、合计 5/5 accepted，并通过 Runtime 内部 Validation、Acceptance、
浏览器验证和原子 Promote；`realProviderVerified=true`，false success 为 0。

机器稳定性审计现为 `passed`、连续 3/3。最后一批实际输入 828,709、输出 43,266、
总计 871,975、缓存输入 253,056 Token；Token 只作为实际用量证据，不是生成目标，也未在
Prompt 中要求模型消耗固定额度。20,000,000 suite 与 2,200,000 per-run 配置仅作为
防失控熔断，本轮均未触发。

## 2. 历史失败五个用例结果（schema v2，保留作根因证据）

| 用例 | 类型 | 最终结果 | 暴露的问题 | 应由哪层修复 |
| --- | --- | --- | --- | --- |
| ZStack Zenova 企业智能体云产品页 | Website | 失败 | 先后触发 Generation/Acceptance 拒绝；修复阶段进入重复读取，最终 `no_progress` | 校验失败后自动解除 Candidate freeze；限制探测式读取 |
| 油田智能运营驾驶舱 | Website | 失败 | Acceptance 拒绝后源码写入被 `project.candidate_frozen` 阻断，最终 `no_progress` | 修复 Run 生命周期，允许同 Run 有界修复 |
| 企业 AI 治理控制台 | Website | **通过** | 17 项验收通过，`/` 返回 200，精确标题命中并原子 Promote | 保留为真实成功基线 |
| 企业智能体云快速开始文档 | Docs | 失败 | Generation 拒绝后重复读取和错误路径探测，最终 `no_progress` | 校验失败重开 Run；输入目录先枚举后读取 |
| 私有化 AI 安全运行手册 | Docs | 失败 | 冷启动 `preview.publish` 被统一 120 秒 deadline 误杀；后续修复耗尽 600k 输入预算 | 构建/发布 300 秒 deadline；普通工具仍为 120 秒 |

第二批权威摘要：
`services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-runs/suite-20260717021107107-failed/real-provider-examples-summary.json`。

第一批在旧 Progress Fingerprint 规则下 5 个用例均被误判 `no_progress`；该批暴露出
`fs.multi_patch`、chunk commit 和首次唯一观察未计为进展的问题，随后已修复并保留原始
证据，不覆盖、不重命名。第一批摘要：
`services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-runs/suite-20260717015314630-failed/real-provider-examples-summary.json`。

### 2.1 修复后定向验证

随后定向复跑油田 Website 与快速开始 Docs，摘要位于
`suite-20260717025440044-failed`。2/2 均真实执行，共 706,266 Token。该批确认：

- 校验失败后 Run 已恢复为 `Running`，源码读写不再被 Candidate freeze 阻断；
- 冷依赖安装和组合发布超过 120 秒后没有被误杀，300 秒构建 deadline 生效；
- 外层摘要已正确记录 `no_progress`，不再是 `unclassified_failure`。

该批同时发现 rejected Candidate 尚未计入 Progress Fingerprint，导致 Repair 继承校验前
的无进展次数。现已将唯一 Candidate manifest digest 纳入状态：新拒绝 Candidate 只重置
一次，同一 Candidate 重复发布不重置。

最后一次单案例诊断保留在 `suite-20260717030742060-running`，没有伪造 summary 或改名。
它发现底层调试工具终态失败已把 Run 状态设为 Failed，但未同步写 `run.completed`，且
evidence idle 计时器被 SSE 心跳错误刷新。本次诊断已主动停止并完成环境恢复；两处均已
修复并增加回归验证。该目录是中断诊断证据，不计入完成批次或通过率。

### 2.2 工作流与全局观察预算定向验证

部署 Runtime 工作流阶段事件和全局观察预算后，再次真实执行快速开始 Docs。权威摘要：
`services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-runs/suite-20260717033605304-failed/real-provider-examples-summary.json`。

- 1/1 真实执行，输入 398,502、输出 6,204、总计 404,706 Token；
- 相比同案例上一次 450,702 Token 降低 45,996，约 10.2%；
- `run.workflow_progress` 真实进入 `authoring_content`、`validating_candidate` 和
  `repairing_candidate`，模型完成源码写入并产出 Candidate；
- 冷构建在 300 秒长工具 deadline 内成功；Generation 拒绝被准确分类，current version
  未被污染；
- 最终仍为 `no_progress`：进入 Repair 后又发生 13 次读取和 2 次搜索，读取由 18 增至
  31，但没有实际源码修复。全局 36/8 上限尚未触发，说明只设全局预算不能约束后半程
  Repair 的重新探索。

基于该证据，Runtime 已增加候选修复阶段的独立子预算：每个被拒 Candidate 默认最多
6 次读取、2 次搜索；超限只拒绝观察工具，同批源码修改仍执行。候选再次被拒时按新
Candidate 重置，发布成功后退出 Repair，进程重启可从事件恢复用量。该修复已通过
自动化回归，尚未再次调用真实 Provider 验证；原因是剩余总预算不足以覆盖一个标准
真实案例的最坏情况预留，不能在未获新预算时越权执行。

### 2.3 真实批次后的确定性修复审计

为避免再次消耗真实 Provider 预算，本轮先用相同 Website/Docs 生命周期、真实 Chromium、
k3d Sandbox、Artifact 和原子 Promote 链路执行 fixture 审计。审计发现并修复的通用问题
包括：Candidate 路由前缀丢失、内部候选鉴权、Edit 冻结 Brief 继承、ChannelLease 与
Run WAL 热路径重复 hydrate、Runtime 重启后的 Brief 冷缓存读写锁自锁、并发 Chromium
退出等待，以及验证失败后未变源码的重复发布。Docs shell 与 Website 响应式 fixture 也
已按真实 Validation Contract 修正。

当前本地审计镜像为 `anydesign/runtime:reliability-m8-repair18-20260717`，commit
`ff7e7dc8713ba72110d0cc68fd99b8e855931258`、`repositoryDirty=true`。镜像构建阶段的
真实 Chromium 门禁已通过：英文 `Noto Sans` 36 glyph、中文 `Noto Sans CJK SC` 9
glyph、Emoji `Noto Color Emoji` 2 glyph，截图 9,771 bytes。该镜像已通过完整
fixture/k3d 矩阵，但因 `repositoryDirty=true` 且 Provider 模式为 fixture，只能描述为
本地 RC audit pass，不能描述为正式发布产物或真实 Provider pass。repair18 在浏览器进程
隔离和重启冷缓存修复之上，增加两项平台级收敛控制：

- Generation/Acceptance 报告持久化 `sourceFingerprint`、失败 check IDs 和 Candidate
  身份；同一失败后源码未变化时，`preview.publish` 在构建与启动 Chromium 前分别返回
  `generation.no_source_change_after_validation_failure` 或
  `acceptance.no_source_change_after_validation_failure`；
- `runs.jsonl` 每个 Runtime 实例只恢复重放一次，状态更新与列表热路径使用内存索引，
  避免历史 WAL 在全局写锁内反复解析造成并发锁饥饿；状态追加改为同步落盘，最后一个
  未完成 JSONL 记录会在恢复时截断，完整坏记录仍失败关闭。

repair16 首次完整矩阵在约 16 MB 历史 `runs.jsonl` 上卡于首批并发 `project.build`，失败
现场保存在
`services/runtime/target/e2e-evidence/zerondesign-e2e/failure-20260717T060742Z`。修复后
repair18 保留同一历史存储重新执行，Website/Docs 并发阶段在 38 秒内完成，随后重启、
DCP 和 WAL 门禁全部通过；恢复门禁还注入了截断尾记录，证明修复尾部后再次重启仍能
读取新状态。这一对失败/通过证据用于证明修复针对长期运行退化，而不是通过清空状态
规避问题。

权威矩阵摘要为
`services/runtime/target/e2e-evidence/zerondesign-e2e/generation-matrix-summary.json`，
`result=pass`，Website/Docs 均满足内容、computed-style、取消清理、依赖代理、事件顺序、
发布后 Artifact 可用和相互隔离；Runtime restart、DCP Repair 与 WAL 恢复也通过。
对应 RC 证据为
`services/runtime/target/e2e-evidence/zerondesign-e2e/runtime-rc-ff7e7dc8713b-dirty-3887f2433d2e.json`，
Runtime image config ID 为
`sha256:996f2b3fb1ecd041047ab515daec462ce4ccf777a8559ae25592d8e87ef20dcc`。

这组 fixture 证据不会改写本报告第 2 节的真实通过率。它只说明已针对真实批次暴露的
平台问题补充了确定性防回归；真实稳定性仍须在获得新 Token 授权后重新评估。

### 2.4 下一批真实回归预算准入审计

本次准入重新从权威 summary 和中断批次 NDJSON 计算，而不是沿用文档估算：4 个有摘要
批次实际使用 3,769,921 Token；`suite-20260717030742060-running` 的 20 条
`model.usage` 事件实际使用 235,445 Token；截至定向重生成前累计 4,005,366。随后
`suite-20260717081922562-accepted` 真实使用 443,337 Token，当前累计 4,448,703，原
5,000,000 累计授权剩余 551,297。每个新 Run 的硬预留仍为 680,000，因此现有余额已
不足以启动下一个真实 Run；执行器必须在上游调用前以
`suite_budget_reservation_exhausted` 失败关闭。

按每个案例最近一次实际用量估算下一批，不把缓存 Token 重复计入总量：

| 案例 | 最近实际用量 |
| --- | ---: |
| Zenova 产品页 | 541,255 |
| 油田驾驶舱 | 255,564 |
| AI 治理控制台 | 214,332 |
| 快速开始文档 | 404,706 |
| 安全运行手册 | 640,164 |
| **合计** | **2,056,021** |

按该历史基线，从当前余额完成一批至少还需要新增 1,504,724 Token 授权；加入 15%
波动余量时建议新增约 1,813,126 Token。该数字是准入容量建议，不是用量目标；
执行器仍按实际使用计量并保留每 Run 680,000、单批 5,000,000 的硬熔断。在新增授权前
直接启动会有较高概率在批次中途触发 `suite_budget_reservation_exhausted`，不符合用户要求的
“正常跑完 5 个真实用例”。

### 2.5 真实 Gateway 漂移门禁与 Zenova 定向重生成

首次重生成批次 `suite-20260717081323801-failed` 暴露出环境级旁路：Runtime 的
`MODEL_PROVIDER` 仍为 `internal_gateway`，但 `MODEL_GATEWAY_URL` 被 fixture 矩阵留在
`fixture-model-gateway`。该批次的 `model.usage.estimated=true`、缺少
`model.execution`，摘要明确为 `realProviderVerified=false`；其 595,421 估算 Token 不计入
真实 API 累计，也不能作为真实回归结论。

本次已增加“自动收敛 + 双重 fail-closed”门禁：真实执行脚本先通过统一入口检查 Gateway
Service、认证 Secret，幂等切换 Runtime 并等待 rollout，再精确校验其必须指向
`provider-gateway.provider-system.svc.cluster.local:9000`；切换结果写入
`runtime-provider-gateway-mode@1` 脱敏证据。evidence 汇总还要求每个已执行 Run 都存在匹配
`deepseek-v4-pro` 的 `model.execution`，否则整批强制为 failed。测试覆盖 Fixture→Real、
切换后 URL 仍错误，以及“案例表面 accepted、上游证据缺失”三种路径。

为从配置源头消除复发，Runtime 基础 Deployment 不再携带 Fixture URL；Fixture RC 必须
显式应用 `fixture-gateway-env-patch.yaml`，该 overlay 同时删除真实 Gateway auth env。
当前 k3d 已执行一次完整的无模型调用切换验证：generation 175 明确进入 Fixture，统一
入口随后切回 generation 176 的 Real Gateway，恢复认证 Secret 引用并保持单一 Ready Pod。

切回真实 Gateway 后执行 `suite-20260717081922562-accepted`：

- Provider 证据为 `deepseek-v4-pro@2`、selection policy revision 1，所有 Usage 均为
  `estimated=false`；
- Brief 22,557 Token，Build 420,780 Token，总计 443,337 Token，其中缓存输入
  97,664；
- 第一版 Candidate 缺少 4 项必需可见文案，被 Runtime Acceptance 拒绝；模型在有限
  Repair 中修改源码并重新发布，最终 24 项 Acceptance、16 项必需 Validation 全部通过；
- `/` 返回 200，精确命中“让 AI 成为可治理的企业生产力”，Artifact body SHA-256 为
  `c71d55e64932cee2d544d23b0c39fc7d5ed784f4d4c682ee71f92bbf73d9984c`；
- 浏览器实测标题、H1、6 个主区块、无横向溢出、无 console error；稳定预览为
  `http://127.0.0.1:18081/`。

此前 `real-provider-examples-bounded-search` 批次中，3 个 Website 用例通过，证明系统
存在成功路径；相同类型用例后续又出现内容漂移，也说明真实模型输出具有波动，不能用
一次成功替代持续的 Runtime 内部内容门禁。

`real-provider-examples-final-pass` 是历史目录名，不代表测试通过。该批次五个案例均被
拒绝；evidence v2 必须根据实际结论使用 `accepted`、`rejected`、`timeout` 或
`cancelled` 命名，避免审计误读。

### 2.6 最新完整五案例真实回归（5/5 accepted）

权威摘要：
`services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-runs/suite-20260717122713924-accepted/real-provider-examples-summary.json`。

| 用例 | 类型 | 结果 | Token | 稳定本地 URL |
| --- | --- | --- | ---: | --- |
| ZStack Zenova 企业智能体云产品页 | Website | accepted | 409,881 | `http://127.0.0.1:18082/` |
| 油田智能运营驾驶舱 | Website | accepted | 143,112 | `http://127.0.0.1:18083/` |
| 企业 AI 治理控制台 | Website | accepted | 152,602 | `http://127.0.0.1:18084/` |
| 企业智能体云快速开始文档 | Docs | accepted | 375,545 | `http://127.0.0.1:18085/docs/` |
| 私有化 AI 安全运行手册 | Docs | accepted | 223,611 | `http://127.0.0.1:18086/docs/` |
| **合计** |  | **5/5** | **1,304,751** |  |

五个 promoted artifact 已从 Runtime PVC 复制到
`services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-preview/suite-20260717122713924/`，
由五个 `restart=unless-stopped` 的独立 nginx 容器按原始根路由提供服务。独立端口避免
路径前缀再次破坏绝对 `/docs/` 链接。HTTP 复核均为 200 且命中 expectedText；应用内
浏览器复核均为 `document.readyState=complete`、关键文本可见、样式表已加载，五页
console error/warning 均为 0。

本轮最终关闭或真实验证的通用问题如下：

1. Runtime Deployment 使用 `Recreate`，消除新旧 Pod 对同一 PVC/WAL 的并发恢复；
2. build 失败可进入 Repair，chunk commit/staged chunk 能推进 Progress Fingerprint；
3. Provider 保留历史工具别名兼容，对隐藏历史 Tool 的策略文本执行有界恢复；
4. 全局与 Repair 的 read/search 预算按类别独立耗尽，不再互相隐藏仍可用工具；
5. Tool Policy Recovery 文本不再误触普通 empty-turn fuse；
6. Docs 浏览器规则固定验证 `/docs/`，不再误测 `/`；
7. 内部 capture 改为 lease Host，取消路径前缀改写造成的 hydration 和绝对链接错误；
8. expectedRoute/expectedText 明确注入 Brief，模型不能另造入口路径；
9. Secret 扫描只作用于当前 suite，历史诊断不会污染新批次结论。

本批次 Provider resource revision 为 2、selection policy revision 为 3、配置 digest 为
`e979689eaeb08507ae0cfe4319e50df6867c2b1dcd2ceee1f34ccf327e71a549`。执行器配置
20,000,000 suite 和 2,200,000 per-run safety ceiling，Provider 每日输入配额
100,000,000；这些是防失控门禁，不是消耗目标，本轮均未触发。

### 2.7 普通用户 Prompt 回归、MDX 通用修复与 3/3 关闭证据

本阶段把五个用例恢复为正常用户会提交的业务需求：描述目标、受众、内容和期望入口，
不注入“必须使用多少 Token”“故意扩大上下文”“制造压力”等测试措辞。系统仍在 Brief
冻结后由执行器补充机器可验证的 `expectedRoute` 与 `expectedText`，但不会改写用户需求、
诱导模型拉长输出或绕过 Runtime 验收。

在第一批完整通过 `suite-20260718010215419-accepted` 之后，下一批
`suite-20260718011716291-failed` 的快速开始 Docs 暴露了模板级兼容缺口：模型自然使用
`<Steps.Step>` 复合 MDX 语法，而模板只提供扁平 `<Step>` 映射；Repair 又尝试创建
`src/mdx-components.tsx` 并引用 `fumadocs-ui/dist` 内部路径，最终 4/5。修复没有针对该篇
文档写特例，而是升级通用 Fumadocs 模板：

1. `fumadocs-docs@runtime-p5` 同时支持 `<Step>` / `<Steps.Step>`、`<Tab>` /
   `<Tabs.Tab>`、`<Accordion>` / `<Accordions.Accordion>`；
2. 只使用 `fumadocs-ui/components/*` 公共入口，不依赖包内部 `dist` 路径；
3. 禁止生成 `src/mdx-components.jsx`、`src/mdx-components.tsx` 等阴影映射文件，并在
   Build Prompt 明确复用模板内置 MDX 映射；
4. 保留 p3、p4 冻结模板，历史版本仍可解析；新增真实 Next.js build 冒烟覆盖扁平和复合
   三类组件语法。

定向 Quickstart 真实回归 `suite-20260718014729601-accepted` 通过后，连续执行三批完整
五案例，结果如下：

| Suite | Website | Docs | 结果 | 实际总 Token | 缓存输入 |
| --- | ---: | ---: | --- | ---: | ---: |
| `suite-20260718015800609-accepted` | 3/3 | 2/2 | 5/5 accepted | 999,913 | 279,552 |
| `suite-20260718021539164-accepted` | 3/3 | 2/2 | 5/5 accepted | 1,216,164 | 396,800 |
| `suite-20260718023350592-accepted` | 3/3 | 2/2 | 5/5 accepted | 871,975 | 253,056 |

权威审计
`services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-runs/real-provider-stability-audit.json`
记录 `status=passed`、`currentConsecutiveFullPasses=3`、false success 0、malformed summary 0。

最后一批生成物已从 Runtime PVC 固化到
`services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-preview/suite-20260718023350592/`，
固定访问地址如下：

| 用例 | URL | HTTP / 浏览器验证 |
| --- | --- | --- |
| ZStack Zenova 企业智能体云产品页 | `http://127.0.0.1:18082/` | 200；关键文案可见 |
| 油田智能运营驾驶舱 | `http://127.0.0.1:18083/` | 200；关键文案可见 |
| 企业 AI 治理控制台 | `http://127.0.0.1:18084/` | 200；关键文案可见 |
| 企业智能体云快速开始文档 | `http://127.0.0.1:18085/docs/` | 200；关键文案可见 |
| 私有化 AI 安全运行手册 | `http://127.0.0.1:18086/docs/` | 200；关键文案可见 |

五个固定容器在切换后和主动重启后均再次通过 HTTP/关键文本探针；应用内浏览器逐页完成
真实 DOM 渲染，页面高度非零且控制台 error 为 0。固定 URL 因此指向最新第三轮产物，
不依赖已结束的临时 port-forward。

## 3. 修复状态分类

### 3.1 已落地并有代码或运行证据

1. Provider 工具别名兼容精确 Runtime 名、唯一规范化前缀和唯一错误 hash 前缀；
2. 修复消息压缩后孤立 `tool` 历史；
3. DeepSeek 工具生成请求关闭 thinking，避免历史 `reasoning_content` 约束；
4. `fs.search` 排除依赖和构建目录，并限制扫描文件、字节、匹配和返回行长度；
5. 真实执行器增加 15 分钟 Run 超时及自动取消；
6. 对明确标记为 retryable 的 Provider 无效响应允许有限重试，并记录低敏失败审计；
7. `deepseek-v4-pro` 运行资源初次人工 reconcile 到 revision 2，验证
   `maxAttempts=2` 可以成为持久化 current revision；
8. CI/RC 去除人工审批编号依赖，保留 Secret、配额、预算、证据和干净工作树门禁；
9. 修复真实证据 Tool 事件计数逻辑；
10. 测试结束后 Runtime 默认预算、Principal Secret 和临时 Sandbox 均恢复或清理。
11. 仓库已增加 `deepseek-v4-pro` 权威声明和审计化 reconcile；当前本地 k3d 证据已验证
    resource revision 4、policy revision 3、配置 digest、`maxAttempts=3` 和 readiness；
12. 真实执行器已升级到 evidence schema v2，SSE 逐块增量落盘，支持 total/idle timeout、
    partial evidence、准确的 accepted/rejected/timeout/failed 命名，以及 commit、配置
    revision/digest 和 manifest provenance；
13. Runtime 已落地 `acceptance-contract@1` 与 `acceptance-report@1`：冻结 Brief 后对
    Candidate 的路由和可见文本执行验收，并由 `run.complete` 复核身份与 digest 后原子
    Promote。正确、拒绝、报告缺失防绕过和重启恢复测试均已通过。
14. Runtime 已启用 Run total/idle watchdog、Tool wall-clock deadline、持久化 Progress
    Fingerprint 和 5 回合无进展熔断；首批真实证据暴露的 mutation/唯一观察误判已修正。
15. Acceptance Repair 最多 3 次：前两次返回结构化可恢复错误，第三次以
    `acceptance.repair_exhausted` 失败关闭；失败报告跨 Candidate 和重启累计。
16. Provider 重试矩阵已失败关闭：限流、不可用、超时和可恢复响应结构错误才可重试；
    未知工具、未请求工具、策略违规和超大响应不重试。Provider 全量测试 42 项通过、
    2 项真实网络测试按预期忽略。
17. 第二批真实回归发现并修复 Candidate 生命周期矛盾：Generation/Acceptance
    校验失败会把父 Run 从 `Validating` 重开为 `Running`；`preview.rebuilding` 也会实际
    重开 Run，不再只是写事件。
18. Tool deadline 改为显式矩阵：普通工具 120 秒，`preview.publish`、
    `project.build`、`project.ensure_dependencies` 和 `package.install` 为 300 秒；Run
    total/idle watchdog 仍提供外层硬边界。
19. Build 提示要求先 `fs.list inputs`，只读取清单中存在的可选 Design Context 文件，
    避免对缺失路径的重复探测。证据失败分类现能区分 `no_progress`、Token 预算和 Runtime
    watchdog，不再统一落为 `unclassified_failure`。
20. Rejected Candidate manifest digest 进入 Progress Fingerprint，给真实 Repair 一个新的
    收敛窗口，但相同 digest 不会反复清零计数。
21. 不可恢复 Tool 错误会由 Agent Loop 幂等补写 `run.completed`，确保 SSE 消费者立即
    收敛；真实执行器只在解析到业务事件时刷新 idle timeout，SSE heartbeat 不再掩盖
    停滞。
22. Runtime 已增加 `run.workflow_progress`：Build/Edit/Repair 阶段、已完成步骤和下一
    动作由真实工具结果确定性推导，并随每回合写入事件、注入模型上下文，避免 Docs 在
    已完成步骤间反复探索。
23. `fs.read`/`fs.list` 默认共享 36 次读取预算，`fs.search` 默认 8 次搜索预算；超限只
    返回可恢复的 `run.observation_budget_exhausted`，不取消同批源码修改。预算用量通过
    `run.observation_budget` 审计，并能在重启后恢复且不重复计算已拒绝调用。
24. Candidate 被 Generation/Acceptance 拒绝后启用更小的 Repair 观察子预算，默认读取
    6 次、搜索 2 次；修复预算与全局预算同时审计和持久恢复，预算超限不会阻断同批
    `fs.multi_patch` 等源码修改。
25. GitHub Actions 已具备无人工审批的真实 Provider 定时门禁；周期性调用需通过仓库
    变量一次性启用。RC 门禁在每个真实 Run 启动前按输入/输出硬预算预留 Token，累计
    预留超过当前 20,000,000 suite safety ceiling 时在调用 Provider 前失败关闭，并生成
    结构化预算证据。
26. 生产手工 Candidate 晋级旁路已关闭。非 `LocalE2E` 模型工具快照不再暴露
    `preview.report_candidate`；旧客户端或恢复请求直接调用时仍会以
    `preview.manual_candidate_retired` 失败关闭。Website/Docs Fixture、HTTP 生命周期和
    k8s E2E 已迁移到 `preview.publish`，因此不能绕过 Generation、Acceptance 和
    `run.complete` 原子晋级门禁。真实 RC 提示也已删除“report/promote candidate”歧义，
    明确要求 `preview.publish` 成功后才能 `run.complete`；Fixture 另有完整 Build/Edit
    工具序列防回退断言。
27. Runtime Deployment 改用 `Recreate`，避免新旧 Pod 同时挂载同一 PVC 并恢复 WAL。
28. Provider 历史 Tool alias 在当前工具快照隐藏时仍可解析；策略提示文本通过最多 5 次
    的有界恢复协议重新提示，不再误计为空回合。
29. 全局和 Repair 的 read/search 预算按工具类别独立耗尽，仍可用的观察工具不会被连带
    从 Tool snapshot 移除，写入工具始终保留。
30. Docs Validation Contract 的入口固定为 `/docs/`；Website 继续使用 `/`。
31. 内部浏览器 capture 使用 lease-scoped Host，取消对 HTML 路径前缀的重写，消除
    Next.js hydration mismatch 和绝对内部链接 404。
32. 五案例执行器将 expectedRoute/expectedText 明确注入冻结 Brief，避免模型派生与
    manifest 不一致的替代入口。
33. Secret 扫描限定为当前 suite；历史失败/诊断目录不再污染新批次结论。
34. 最新完整批次 5/5 accepted，证明上述修复能在同一真实批次中组合工作。
35. Fumadocs p5 同时支持扁平/复合 MDX 组件语法，阻止阴影 `mdx-components` 和内部包
    路径写入；修复后定向 Docs 通过，并取得连续三批完整 5/5，稳定性审计 3/3 passed。

### 3.2 临时诊断措施，不计入根因关闭

1. 单 Run 输入上限提高到 600k；
2. 复杂用例提高到 60 回合、180 工具调用；
3. 外层执行器设置 15 分钟总超时；
4. 使用原始 Artifact 字符串包含关系检查业务文本；
5. 通过临时 Admin API reconcile revision 2。

这些措施帮助暴露了问题，但不能保证后续任务不再遇到内容漂移、工具阻塞、无进展循环
或配置漂移。

## 4. 核心修复评审

### P0：将 Brief 验收项纳入 Runtime 完成事务（已落地，真实成功路径已验证）

公共验证之外，Runtime 现在会在确认 Brief 时冻结 `acceptance-contract@1`，当前 v1
最小实现包含：

- 必需路由；
- 必需可见文本；
- 禁止出现的模板占位内容；
- Website / Docs 对应的业务结构和语言；
- Brief、Content Sources、Contract 和 Candidate digest，以及 NFKC、entity、空白、
  大小写规范化规则。

验收已在尚未 Promote 的 Candidate 上执行，任何必需断言失败时不能晋级或报告完成；
验收通过后，由 `run.complete` 复核持久化报告并原子提交 Completed + Promote。受限
Repair 状态机已落地：同一 Build Run 最多 3 次，并使用全局 36/8 与 Repair 6/2 观察
预算限制重复探索。`suite-20260718015800609-accepted` 与
`suite-20260718021539164-accepted` 均留下 `repairActive=true` 的真实事件，Repair 读取
用量按拒绝 Candidate 独立计数并最终 accepted，证明子预算已在真实 Provider 链路生效。

### P0：生产级 deadline 和无进展检测（已落地，真实回归已验证）

已实现 Tool 整体 deadline、Run total/idle watchdog、持久化 Progress Fingerprint、连续
5 回合无进展熔断和取消传播。真实批次进一步校准了进展定义：源码批量修改、chunk
commit 和有限数量的首次唯一读取计为进展，同一路径/查询的重复观察不计为进展。

第二批发现统一 120 秒 deadline 会误伤含冷依赖安装的组合发布工具，现已拆分为普通
工具 120 秒和构建/发布工具 300 秒。Docs 分步计划与全局读取/搜索预算已经真实验证：
同案例 Token 降低约 10.2%，成功写源码并产出 Candidate，但 Repair 后半程仍重复观察。
因此又增加 Repair 6/2 子预算并通过正常、超限、同批写入和重启恢复测试；该子预算随后
在两批完整真实回归中进入 `repairActive=true` 并最终 accepted。这些能力用于提高收敛率，
现有 fail-closed 和有界终止能力保持不变。

### P0：校验失败后的 Candidate 修复生命周期（已修复，真实回归已验证）

第二批证明 Acceptance Contract 能正确拦截错误内容，但同时发现 Run 留在
`Validating` 会让 `fs.write` 被 Candidate freeze 阻断。现已统一处理：可恢复的
Generation、Design Profile 或 Acceptance 拒绝会重开父 Run；`preview.rebuilding`
也会实际更新 Run 状态。源码仍只允许在明确进入 Repair/Rebuilding 后修改，失败
Candidate 和报告继续保留，current version 不变。
连续完整回归中已有 rejected Candidate 进入 Repair、继续修改源码并成功 Promote，证明
freeze 解锁与候选身份复核在真实链路组合生效。

### P0：自动化持久化 Provider 配置升级（已落地）

部署流程应固定执行：

1. 读取仓库中与运行资源一致的无密钥权威声明；
2. 校验 revision 单调递增和配置 digest；
3. 使用审计上下文和幂等键执行 reconcile；
4. 验证 SQLite/PostgreSQL current revision 与声明一致；
5. 移除临时 Admin 凭据；
6. 运行 readiness 和真实错误重试探针。

上述流程已由权威声明、部署脚本和 reconcile 脚本实现；revision 或 digest 不一致时失败
关闭。当前真实 readiness 证据位于
`services/runtime/target/e2e-evidence/zerondesign-e2e/provider-resource-reconcile-m8.json`。

### P1：Provider 重试分类（已落地）

Provider 错误已按以下矩阵类型化处理：

- 网络、解析、空响应和可恢复结构错误：在 deadline 允许时最多重试一次；
- 未知或未请求工具、策略违规和超大参数：不重试；
- 所有尝试记录原因、attempt 和资源 revision，审计内容保持低敏。

表驱动测试已覆盖每类错误的尝试次数、deadline 和最终错误分类；Provider 全量测试
42 项通过、2 项真实网络测试按预期忽略。

## 5. 证据可信度说明

历史 schema v1 evidence 只能证明当时的真实调用、构建、拒绝结果和 Token 量级，不能
作为最终发布证据。2026-07-17 两批 schema v2 已补齐：commit/dirty 状态、Provider
revision 与配置 digest、manifest digest、增量 NDJSON 事件、total/idle timeout、逐 Run
usage 和准确 Tool 计数。

第二批曾有一个证据层缺陷：Runtime 终止摘要最初被外层执行器记录为
`unclassified_failure`。执行器现已增加确定性映射，但历史摘要保持不可变，因此审计时
应结合 `run.completed.summary` 读取真实分类。后续批次将直接输出 `no_progress`、
`run_token_budget_exhausted`、`runtime_idle_timeout` 或 `runtime_total_timeout`。最新 Docs
定向批次已经直接输出 `no_progress`，证明该分类修复生效。

## 6. 现有证据

- `services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-examples-bounded-search/real-provider-examples-summary.json`
- `services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-examples-final-pass/real-provider-examples-summary.json`
- `services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-runs/suite-20260717015314630-failed/real-provider-examples-summary.json`
- `services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-runs/suite-20260717021107107-failed/real-provider-examples-summary.json`
- `services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-runs/suite-20260717025440044-failed/real-provider-examples-summary.json`
- `services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-runs/suite-20260717033605304-failed/real-provider-examples-summary.json`
- `services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-runs/suite-20260717122713924-accepted/real-provider-examples-summary.json`
- `services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-runs/real-provider-stability-audit.json`
- `services/runtime/target/e2e-evidence/zerondesign-e2e/generation-matrix-summary.json`
- `services/runtime/target/e2e-evidence/zerondesign-e2e/runtime-rc-ff7e7dc8713b-dirty-3887f2433d2e.json`
- `services/runtime/target/e2e-evidence/zerondesign-e2e/failure-20260717T060742Z/exit-status.txt`
- `2026-07-16-generation-reliability-remediation-plan.md`

代码级验证：

- `fs_search_skips_generated_dependency_trees_and_reports_bounds`：通过；
- `fs_read_write_list_and_search_are_workspace_bounded`：通过；
- Provider response / alias / retry 回归：6 项通过；
- Release evidence validator：通过；
- Generation reliability contract：通过；
- Agent Loop：53 项通过，1 项真实网络测试按预期忽略；
- Runtime lib：176 项通过；Checkpoint：25 项通过；Sandbox 集成：96 项通过、1 项真实 npm
  构建冒烟按设计忽略；browser 定向回归：10 项通过；
- Review/Repair：20 项通过；
- Repair 观察子预算：验证第 3 次读取被结构化拒绝、同批源码写入仍成功、重启恢复用量。
- 未改源码重复发布熔断：Generation 失败跨重启仍在构建前拒绝且不新增 build log；
  Acceptance 失败不误增 Repair 次数；源码修改后允许继续发布。
- Run WAL 恢复：热路径在首次 hydrate 后不再重放日志；同步追加；截断的最后记录可修复
  并在二次重启后继续写入，完整坏记录失败关闭；repair18 在约 17 MB 历史 WAL 上通过
  并发、轮询和重启矩阵。
- 上述 Candidate 收敛与 Run WAL 回归已进入 PR `contracts` Job 和 k3d recovery gate，
  生成可靠性合同会检查门禁测试入口不可被静默删除。
- Preview Promotion：24 项通过，包含生产拒绝手工 Candidate、失败门禁、CAS、WAL 和
  原子完成；Website/Docs 两条公开生命周期迁移测试通过。

## 7. 下一批回归准入条件

开始新的五案例真实回归前，至少满足：

1. [x] `acceptance-contract@1` 已冻结并在 Promote 前执行；
2. [x] 真实 ModelResource 声明和自动 reconcile 已落地；
3. [x] Runtime Tool deadline、idle watchdog 和 Progress Fingerprint 已启用；
4. [x] 同一 Build Run 支持最多 3 次 Candidate 局部 Repair，校验失败会解除 freeze；
5. [x] evidence v2 使用流式采集、修正 Tool 计数和准确的结果命名；
6. [x] Fixture/集成回归已覆盖成功、内容拒绝、deadline、watchdog、重启、配置漂移和
   Repair 耗尽场景。
7. [x] Generation/Acceptance 失败后，同 fingerprint 未变源码会在构建前 fail-fast；
   修改源码后可继续 Repair，且失败状态跨 Runtime 重启恢复。
8. [x] 保留大体量历史 Run/ChannelLease WAL 时，Website/Docs 并发 Build/Edit 与重启门禁
   在限定时间内通过。
9. [x] 真实回归在调用前精确校验 Provider Gateway URL，并要求每个已执行 Run 均有匹配
   ModelResource 的 `model.execution`；fixture 漂移不能再伪装成真实结果。
10. [x] 真实稳定性退出标准已机器化：只认完整、上游已验证、5/5 accepted、Artifact
    探针通过且预算未越界的批次；定向批次不冒充完整通过，完整失败批次清零连续计数。
    当前权威审计为 `passed`、`3/3`、false success 0、malformed summary 0。
11. [x] 本轮不设置完成目标 Token；20,000,000 suite、2,200,000 per-run 与 Provider
    100,000,000 daily input 仅作为防失控门禁。最后一轮实际使用 871,975，未触发预算退出。

真实批次的退出标准以修复方案第 12 节为准，不能用“全部执行过”替代“全部通过内部
验收”。
