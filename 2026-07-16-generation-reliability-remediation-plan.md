# Website / Docs 生成可靠性修复方案

> 文档状态：本阶段已关闭，持续监控
> 当前评审结论：repair32 已部署；连续三批完整真实回归 5/5 accepted，稳定性门禁 3/3 passed
> 最近更新：2026-07-18

## 0. 执行摘要

当前系统已经具备 Candidate 隔离、构建与浏览器验证、原子 Promote、预算熔断、冻结
Brief 的 Candidate 内容验收、生产 Tool/Run watchdog、Progress Fingerprint、有限
Repair、Provider 重试矩阵和声明式 ModelResource reconcile。2026-07-18 使用真实
`deepseek-v4-pro`、普通用户式 Prompt 连续完成三批 schema v2 五案例回归：Website 3/3、
Docs 2/2、每批合计 5/5 accepted，没有 false success。最后一批实际使用 871,975 Token，
所有 Artifact 均通过 Runtime 内部路由、可见文本、浏览器和 Promote 门禁。

2026-07-18 的最新确定性矩阵已使用
`anydesign/runtime:reliability-m8-repair32-20260718` 通过：Website/Docs 并发 Build+Edit、
真实 Chromium、内容与 computed-style、取消清理、依赖代理、Sandbox/Artifact 隔离、
Runtime restart、DCP Build/Edit/Review/Repair 和 WAL 恢复均为 pass。该结果关闭了本轮
自动化阻塞。机器稳定性审计已达到第 12 节要求的连续 3/3；本阶段从“执行中”转为
“已关闭、持续监控”，后续完整失败批次仍会按规则清零新的连续计数并触发复盘。

本阶段不得把以下措施视为根因修复：

- 单纯提高 Turn、Tool Call 或 Token 上限；
- 仅由外层测试脚本检查最终 HTML 字符串；
- 手工调用 Admin API 更新 ModelResource；
- 只为测试执行器增加总超时，而生产工具调用没有 deadline；
- 修复统计代码后继续引用修复前生成的 evidence。

执行状态按以下顺序跟踪：

1. [x] 建立可审计的配置和证据基线；
2. [x] 将 Brief 验收契约加入 Candidate → Promote 原子事务；
3. [x] 为 Website / Docs 增加无进展检测、生产级 deadline 和有限 Repair；
4. [x] 固化 Provider 错误分类、重试策略、声明式 reconcile 与生产晋级旁路关闭；
5. [x] 连续 3 批完整五案例均 5/5，并通过第 12 节机器稳定性审计（当前 3/3）。

### 0.1 当前修复基线（2026-07-17）

本轮 fixture/k3d 回归没有把失败归因于具体页面文案，而是继续沿 Candidate、浏览器、
持久化恢复和并发控制四条平台链路排查。已经定位并修复以下通用缺陷：

| 缺陷 | 影响范围 | 根因 | 平台修复 | 验证状态 |
| --- | --- | --- | --- | --- |
| 验证器检查 Runtime 首页而不是 Candidate | Website / Docs | 浏览器路由使用 `new URL("/", candidateUrl)`，丢失 `/previews/<lease>/` 前缀 | 路由解析保留 Candidate base path，并用真实 Chromium 集成测试隔离根页与候选页 | 自动化通过 |
| 内部验收 Candidate 返回 401 | Website / Docs | 验证器使用对外鉴权 URL，却没有 Principal 上下文 | Runtime 内部验收改用受信任的 loopback `preview-captures/<lease>/`，继续复核 lease 和 manifest | Website、Docs 定向 fixture 通过 |
| Edit 无法完成 Candidate 验收 | Edit / Repair | API 只传 `baseVersionId` 时没有从基线版本继承冻结 Brief | Edit 启动时从 immutable base version 的 creator run 继承已确认 Brief；显式不匹配继续失败关闭 | 生命周期回归通过 |
| 长期运行后并发文件操作冻结 | 所有 Sandbox 生成 | 每个操作重复读取并重放 24.5 MB `channel-leases.jsonl`，同时每次访问都持久化 heartbeat | lease 进程内只 hydrate 一次；ready heartbeat 60 秒合并持久化 | 保留旧日志的并发 fixture 已恢复正常吞吐 |
| 历史 Run 增长后 Runtime API 与并发构建冻结 | Website / Docs / 所有 Run | `update_run_status`、Run 列表和恢复热路径在全局写锁内反复重放约 17 MB `runs.jsonl`；并发轮询与构建发生锁饥饿 | `runs.jsonl` 每个 Runtime 实例只 hydrate 一次，热路径使用内存索引；状态追加同步落盘；崩溃截断的最后半行会先截断再继续写，完整坏记录仍失败关闭 | repair16 失败现场可复现；repair18 在相同历史 WAL 上完整矩阵与二次重启通过 |
| Runtime 重启后 `run.complete` 永久等待 | 重启恢复后的 Website / Docs | 冷缓存路径在持有 `brief_run_ids` 读锁时调用会获取写锁的 `content_sources()`，形成自锁 | 在任何 await/write 之前显式释放读锁，并增加持久化 Brief 重载超时回归 | 定向重启测试、repair18 恢复矩阵通过 |
| 并发 Chromium 截图永久等待 | 并发 Website / Docs | 多个 headless Chromium/crashpad 子进程竞争时可能在浏览器退出后继续持有 stdio；截图 `.output()` 没有子进程 deadline | 所有截图与 computed-style collector 共用 Runtime 浏览器进程锁；截图增加 30 秒 timeout 与 drop-kill | 10 项 browser 回归、repair18 并发矩阵通过 |
| Docs 导航/搜索与 Website 移动端误报 | Website / Docs fixture | 测试产物自身没有在目标 Docs 路由复用导航/搜索，Website 使用浏览器默认 body/h1 样式造成横向溢出 | fixture 复用完整 Docs shell；统一 viewport、box-sizing、显式字号、换行和 overflow 基线 | Website、Docs 定向 fixture 通过 |
| 复用镜像的版本校验误拒绝 | RC / CI | `/version` 返回完整 Git SHA，门禁只接受短 SHA | 版本验证同时接受完整 SHA 和唯一短 SHA | 契约与单测通过 |
| 验证失败后重复发布未变 Candidate | Website / Docs Repair | Generation/Acceptance 失败证据没有持久化源码 fingerprint，模型可重复构建语义相同的 Candidate 并消耗 Repair 预算 | 持久化验证状态、源码 fingerprint、失败 check IDs 和 Candidate 身份；`preview.publish` 构建前比较，未变化时类型化 fail-fast | 重启恢复、无新增 build log、修改源码后恢复发布的集成测试通过 |
| 真实回归误连 Fixture Gateway | Website / Docs Real Provider | 基础 Runtime Deployment 硬编码 Fixture URL；只校验 `MODEL_PROVIDER=internal_gateway`，未校验 `MODEL_GATEWAY_URL`；摘要缺少 `model.execution` 时仍可能按案例状态结束 | 基础清单移除环境 URL，Fixture 由显式 overlay 注入并删除真实 auth env；统一 Real 入口检查 Service/Secret、幂等切换、等待 rollout 并生成模式证据；随后精确校验 URL；每个已执行 Run 必须含匹配 ModelResource 的 `model.execution`，否则整批 failed | Fixture→Real、错误 URL、缺失上游证据测试通过；当前 k3d 实际 Fixture→Real、Ready Pod 和 auth Secret 证据通过；Zenova 1/1 accepted |
| Runtime 滚动更新与共享存储恢复竞争 | 所有持久化 Run | `RollingUpdate` 让新旧 Pod 同时恢复同一 PVC/WAL | Runtime Deployment 改为 `Recreate`，只允许单 Pod 挂载和恢复持久化状态 | 多次 k3d 重部署与本轮 5/5 真实批次通过 |
| Repair 观察预算互相误伤 | Website / Docs Repair | 读取预算耗尽时把搜索一并从 Tool snapshot 移除，反之亦然 | 全局与 Repair 的 read/search 预算按工具类别独立过滤；写入工具不受观察预算影响 | Agent Loop 回归与真实 Docs 收敛通过 |
| Provider 返回工具策略提示文本 | 所有真实模型 Run | 普通文本被计为空回合，3 次后错误触发 empty-turn fuse | 增加有界 Tool Policy Recovery 协议；恢复提示最多 5 次且不占普通空回合计数 | Provider/Runtime 回归与本轮 5/5 通过 |
| Docs 验收误测 `/` | Docs | Website 和 Docs 共用根路由浏览器规则 | Validation Contract 按 ArtifactType 选择 Website `/`、Docs `/docs/` | Docs 2/2 真实通过 |
| Candidate 前缀改写破坏 Next.js hydration | Docs / 绝对路径应用 | 路径式 capture 为 `href/src` 注入前缀，客户端树与静态 HTML 不一致且内部链接 404 | 内部浏览器改用 `<lease>.preview.local` 独立 Host，资源保持原始根路径；Chromium 显式解析到 loopback | Quickstart console error 清零并 accepted |
| 模型自行发明入口路径 | Docs / 多语言内容 | Brief 没有明确冻结 manifest 的 expectedRoute/expectedText | 真实执行器把验收契约显式注入 Brief，禁止派生替代入口 | Runbook 定向回归与完整批次均 accepted |
| 密钥扫描污染当前批次结论 | 真实回归执行器 | 扫描整个历史 runs 根目录，旧诊断文本可让新批次误失败 | 只扫描当前 suite 目录并收紧 Bearer 形态；真实密钥仍失败关闭 | 执行器测试和完整批次收尾通过 |
| Docs 复合 MDX 语法导致构建失败 | 所有 Fumadocs Docs | 模型自然使用 `<Steps.Step>` 等复合写法，而模板只映射扁平组件；Repair 可能创建阴影 MDX 配置并引用包内部路径 | `fumadocs-docs@runtime-p5` 同时支持扁平/复合 Steps、Tabs、Accordion；禁止阴影 `src/mdx-components.*`，只允许公共组件入口；保留 p3/p4 历史模板 | 真实 Quickstart 定向通过；真实 Next build 冒烟通过；随后连续三批完整 5/5 |

上述熔断只阻止“同一次失败后、源码完全未变”的重复发布，不把失败永久缓存：源码变化
后允许继续 Repair；失败记录跨 Runtime 重启恢复；fail-fast 发生在构建和 Chromium 启动
之前，因此不会生成新的 build log，也不会错误增加 Acceptance Repair 次数。

### 0.2 证据使用规则

- 定向 fixture 通过只证明对应根因修复，不替代完整矩阵；
- 完整 fixture 矩阵证明确定性生命周期、重启、并发与门禁，不等同于真实模型稳定性；
- 真实 Provider 批次只能按实际 `accepted/rejected/timeout/cancelled` 结论引用；
- `/health`、`/version` 和镜像字体冒烟是环境证据，不是 Website/Docs 质量通过证明；
- 本轮按用户授权不设置完成目标 Token；执行器保留 20,000,000 suite、2,200,000 per-run
  的防失控上限，Provider 每日输入配额为 100,000,000。最后一轮实际使用 871,975 Token，
  未触发任何预算门禁。`realProviderVerified=false` 的 fixture 漂移批次不计入真实累计。

### 0.3 真实生成 Prompt 规则

真实案例必须模拟普通用户使用，Prompt 只包含业务目标、受众、内容边界、交付类型以及
用户实际会提出的设计要求。以下内容不得注入用户 Prompt：固定 Token 消耗目标、要求故意
扩写、重复读取、制造长上下文或为了压测而增加页面/章节。Token 上限属于执行器防失控
配置，不属于作品质量指标。

机器验收信息采用独立通道：执行器在冻结 Brief 时附加模板类型、`expectedRoute`、
`expectedText` 和禁止占位内容；Agent Build Prompt 只补充平台契约，例如优先修改业务内容、
使用模板内置 MDX 映射、不得创建阴影 `src/mdx-components.*`、不得引用依赖包内部 `dist`
路径。模型输出仍由 Validation/Acceptance 决定是否可 Promote，测试脚本不能把额外文案
偷偷拼入用户作品来“帮助通过”。

## 1. 背景

当前 Runtime 已能够通过真实模型生成 Website 和 Docs，并完成构建、截图、候选版本与预览发布。但现有链路仍存在若干平台级风险：

- Provider 对工具名称、严格 Schema、并行调用等协议约束不一致，可能在任务中途才暴露。
- `preview.publish` 同时承担构建、验证和正式版本提升，导致后续步骤失败时仍可能留下已提升版本。
- 构建成功、渲染成功、质量验证成功和 Run 完成之间缺少统一事务边界。
- 浏览器运行环境缺少完整多语言字体时，截图可能出现缺字但仍通过“非空截图”检查。
- Website 与 Docs 的完成条件分散在提示词和工具实现中，缺少模板声明的统一交付契约。
- 产物、Gateway 状态或本地预览依赖临时目录、`emptyDir` 或手工配置时，无法稳定恢复和复现。
- 模型轮次、工具调用和 Token 缺少统一预算，协议错误可能造成无效消耗。

本方案不针对某个页面、产品内容、语言或模型，目标是建立适用于所有 Website 和 Docs 生成任务的统一可靠性基线。

## 2. 最终目标

平台必须保证：

```text
Run Completed
    ⇕
Required Validation Checks All Passed
    ⇕
Candidate Version Atomically Promoted
```

任何必需检查失败或不可用时：

- 当前正式版本保持不变；
- 候选产物和诊断证据可以保留；
- Run 不得标记为 Completed；
- 候选版本不得成为 current version；
- 用户得到结构化失败原因，而不是仅收到通用 Provider 或构建错误。

## 3. 统一生成状态机

### 3.1 Run 状态

```text
Queued
→ Running
→ CandidateReady
→ Validating
→ Completed | Failed | Cancelled
```

### 3.2 Version 状态

```text
Candidate
→ Validated
→ Promoted | Rejected
```

### 3.3 核心规则

1. `preview.publish` 只生成 Candidate、Artifact、截图和验证证据，不修改 current version。
2. 验证结果统一使用 `passed`、`failed`、`unavailable`。
3. 必需检查只有全部为 `passed` 才允许提升。
4. `run.complete` 调用 Runtime 的原子完成操作，同时提交 Run 完成和 Version 提升。
5. Run 失败、取消或超预算时，当前正式版本不变化。
6. 原子完成操作必须支持幂等和 CAS，避免并发 Run 覆盖新版本。

## 4. Generation Contract

每个可注册模板必须提供 Generation Contract 和 Validation Profile。

### 4.1 公共结构

```json
{
  "schemaVersion": "generation-contract@1",
  "artifactType": "website",
  "templateKey": "next-app",
  "build": {
    "command": ["npm", "run", "build"],
    "outputDirectory": "dist"
  },
  "requiredChecks": []
}
```

### 4.2 Website 基线

```json
{
  "requiredChecks": [
    "build",
    "artifact-integrity",
    "desktop-render",
    "mobile-render",
    "font-coverage",
    "accessibility",
    "responsive-layout",
    "link-integrity",
    "console-errors",
    "metadata"
  ]
}
```

### 4.3 Docs 基线

```json
{
  "requiredChecks": [
    "build",
    "artifact-integrity",
    "mdx-compile",
    "navigation",
    "duplicate-slugs",
    "internal-links",
    "heading-anchors",
    "code-blocks",
    "search-index",
    "desktop-render",
    "mobile-render",
    "font-coverage",
    "accessibility",
    "console-errors"
  ]
}
```

新增模板如果没有 Validation Profile，不允许进入可生成模板注册表。

### 4.4 Acceptance Contract

Generation Contract 回答“某类产物如何构建和验证”，Acceptance Contract 回答“本次
Run 是否完成了用户确认的 Brief”。二者必须分离、同时冻结。

当前落地的 v1 最小结构：

```json
{
  "schemaVersion": "acceptance-contract@1",
  "briefId": "brief-123",
  "briefDigest": "<sha256>",
  "contentSourcesDigest": "<sha256>",
  "artifactType": "website",
  "locale": "zh-CN",
  "requiredRoutes": ["/"],
  "requiredText": ["ZStack Zenova", "产品定位与核心理念"],
  "forbiddenText": ["Lorem ipsum", "TODO", "placeholder"],
  "legacy": false,
  "contractDigest": "<sha256>"
}
```

后续若需要“同义词组任一命中”、章节层级或 locale 一致性，应以新 schema revision
显式增加，不能在不改变 digest 语义的情况下暗改 v1。

规则：

1. Acceptance Contract 在 Brief 确认后生成并冻结，保存 `briefDigest`；
2. 验收对象必须是尚未 Promote 的 Candidate；
3. Website 检查解析后的可见 DOM，Docs 检查编译后的正文与导航，不直接匹配原始
   HTML/MDX 字符串；
4. 文本比较使用 NFKC Unicode 规范化、解析后 HTML entity、空白折叠和大小写不敏感
   策略；`script` / `style` 内容不能满足可见文本断言；
5. 必需断言失败时 Candidate 进入 `Rejected` 或有限 Repair，不得调用完成晋级；
6. `run.complete` 在同一原子操作中复核 Generation Contract、Validation Report、
   Acceptance Contract 和 Candidate digest，再提交 Completed + Promote；
7. Repair 不得修改冻结的 Brief 或 Acceptance Contract，只能生成新 Candidate。

## 5. Provider Adapter

Provider 差异必须由 Gateway Adapter 吸收，不能依赖生成提示词规避。

### 5.1 工具名称

每次请求生成独立的双向别名表：

```text
project.write_page
↔ project_write_page__<stable-hash>
```

要求：

- Provider 名称仅包含字母、数字、下划线和连字符；
- 名称长度满足主流 OpenAI-compatible Provider 限制；
- 不同 Runtime 工具不能映射为同一个 Provider 名称；
- 工具定义、历史 assistant tool calls 和返回 tool calls 使用同一映射；
- Provider 返回未知别名时拒绝执行；
- 普通工具和 deferred tools 共用同一命名空间。

### 5.2 Provider 能力

模型资源应声明或通过控制面探针确认：

- 工具名称格式和长度；
- strict tool schema；
- parallel tool calls；
- streaming；
- reasoning content；
- usage 字段；
- 空 tools 行为；
- 最大 Schema 和上下文尺寸。

能力不满足 Generation Contract 时，在启动 Run 前失败，不进入付费生成循环。

## 6. Validation Report

统一报告格式：

```json
{
  "schemaVersion": "validation-report@2",
  "runId": "run-123",
  "candidateVersionId": "version-456",
  "candidateManifestHash": "<sha256>",
  "artifactManifestHash": "<sha256>",
  "generationContractDigest": "<sha256>",
  "templateVersion": "next-app@1",
  "checks": [
    {
      "id": "mobile-render",
      "status": "unavailable",
      "message": "browser worker unavailable",
      "evidence": []
    }
  ]
}
```

规则：

- `required && status != passed` 时禁止提升；
- `unavailable` 不得按成功处理；
- 每项检查必须携带可定位的证据路径或错误分类；
- 报告必须随 Candidate 持久化。
- 完成提升前必须重新校验 Candidate Manifest、Artifact Manifest、Generation Contract
  和模板版本摘要，任一不匹配均按失败处理。

## 7. 多语言渲染基线

Runtime 浏览器镜像至少安装：

- Noto Core；
- Noto CJK；
- Noto Color Emoji；
- Fontconfig；
- UTF-8 locale。

渲染检查不能仅判断截图非空，还必须验证：

- `document.fonts.ready`；
- 页面实际使用字符的字体覆盖；
- DOM 存在有效文本；
- 无明显缺字方框；
- 桌面和移动端均无横向溢出；
- 静态资源无 4xx/5xx；
- 浏览器控制台无阻断性错误。

## 8. 预算、收敛与熔断

每个 Run 应支持：

```json
{
  "maxTurns": 20,
  "maxToolCalls": 60,
  "maxInputTokens": 150000,
  "maxOutputTokens": 30000,
  "maxProviderFailures": 2,
  "maxRepairCycles": 3
}
```

同类不可重试协议错误连续发生两次时，停止模型循环并输出确定性诊断。最终 Run 记录调用次数、Token、Provider 错误和预算终止原因。

预算是最后一道安全边界，不是收敛机制。每个 Run 还必须维护 Progress Fingerprint：

```text
source digest + candidate digest + changed files + completed steps + tool sequence digest
```

建议的初始策略：

- 连续 5 回合 Progress Fingerprint 无实质变化时，判定 `no_progress`；
- 同一路径、同一查询或同一预览结果的重复调用单独计数；
- `no_progress` 后最多进入 2 次局部 Repair，不重新扩张完整上下文；
- Tool 必须同时具有单次操作 timeout 和整个 Tool Call 的 wall-clock deadline；
- Run 同时具有 total timeout 和 idle timeout，任何超时都传播取消信号；
- Website 与 Docs、Brief / Build / Repair 分阶段配置预算，禁止共享一个无限扩张的总池。

真实诊断使用的 `60 turns / 180 tool calls / 600k input tokens` 仅为观测复杂用例的临时
测试上限。生产默认值继续由风险和历史分位数控制；在无进展检测落地前，不再通过扩大
预算处理不收敛问题。

当前实现状态（2026-07-17）：

- Run total/idle timeout 默认 30/5 分钟，触发时写入 `run.watchdog_triggered` 并取消；
- 连续 5 回合 Progress Fingerprint 不变时以 `no_progress` 结束；mutation、Candidate、
  completed step 和最多 24 个首次唯一观察进入 fingerprint，重复路径/查询不计进展；
- 普通 Tool wall-clock deadline 为 120 秒；构建/依赖/发布类组合工具为 300 秒，避免
  冷依赖安装被普通工具阈值误杀；
- Acceptance Repair 默认最多 3 次，失败报告跨 Candidate/重启累计，耗尽后返回不可
  恢复的 `acceptance.repair_exhausted`；
- 可恢复的 Generation/Design/Acceptance 拒绝会将 Run 从 `Validating` 重开为
  `Running`，允许修改源码生成新 Candidate；current version 始终保持不变。
- Build/Edit/Repair 每个回合都会生成 `run.workflow_progress`，向模型提供
  `discovering_inputs → loading_requirements → initializing_project → inspecting_source →
  authoring_content → validating/repairing_candidate → ready_to_complete` 的权威阶段、已完成
  步骤和唯一下一动作；阶段由 Runtime 根据真实工具结果推导，不由模型自报；
- `fs.read`/`fs.list` 共用独立读取预算（默认 36），`fs.search` 使用独立搜索预算（默认
  8）。超限时只拒绝对应观察调用并返回 `run.observation_budget_exhausted`，同一批中的源码
  写入/发布仍可执行；`run.observation_budget` 持久化用量，重启恢复时排除已被预算拒绝的
  调用，避免重复计费。

## 9. 持久化与部署

必须持久化：

- Brief 和 Content Sources；
- Source Snapshot；
- Candidate 和 Promoted Artifact；
- Validation Report；
- 桌面与移动截图；
- Run Events；
- Model Execution 和 Token Usage；
- Gateway 幂等、配额、审计和熔断状态。

正式产物不得只依赖 `/tmp`、Sandbox 工作目录、Docker bind mount 或 Kubernetes `emptyDir`。

本地 k3d 环境应使用声明式清单和 PVC；生产环境建议 PostgreSQL 加 S3 兼容对象存储。

## 10. 实施里程碑

### M1：协议与环境前置保障

- [x] 将 OpenAI-compatible 工具名称改为可逆、无碰撞的请求级映射。
- [x] 增加工具名称往返、碰撞和未知别名测试。
- [x] Runtime 镜像增加常用多语言字体和 locale。
- [x] 增加中文、英文、Emoji 的浏览器字体冒烟测试。

Runtime 镜像构建现在会启动真实 Chromium，通过 DevTools
`CSS.getPlatformFontsForNode` 证明英文、中文和 Emoji 文本分别由 Noto 字体产生非零
glyph，并验证截图为非空 PNG。该检查不是 Dockerfile 字符串断言：任何字体包、
fontconfig 或 Chromium 回归都会让镜像构建失败。镜像
本地 RC 基线 `anydesign/runtime:reliability-m8-repair18-20260717` 已在镜像构建阶段、
运行 Pod 和完整矩阵中通过相同冒烟。该构建 `repositoryDirty=true`，因此仍是本地审计
镜像，不是可发布的干净工作树制品。

### M2：候选与正式发布解耦

- [x] `preview.publish` 只创建并暂存 Candidate，不修改 current version。
- [x] Artifact Publish 增加 `Ready` 状态，失败候选进入可回收状态。
- [x] 新增基于 WAL 的 `complete_artifact_promotion_cas` 原子操作。
- [x] 将 `preview.updated` 和 `run.completed` 移到原子提交成功后的 outbox。
- [x] 增加失败、重启、并发 CAS 和幂等重放测试。

生产晋级旁路已经关闭：Website/Docs 的 Fixture、HTTP 生命周期与 k8s E2E 调用方均已
迁移到 `preview.publish`。`preview.report_candidate` 只为显式 `LocalE2E` 的底层机制测试
保留，非 LocalE2E 模型工具快照不会暴露该工具；旧客户端或恢复请求直接调用时，会在
创建 Candidate 前以 `preview.manual_candidate_retired` 失败关闭。因此生产流程必须生成
Generation Report、Acceptance Report，并由 `run.complete` 执行 Completed + Promote
原子事务，不能再手工报告 Candidate 绕过门禁。

### M3：统一验证框架

- [x] 增加 Generation Contract 核心模型和 Website/Docs 基线。
- [x] 增加三态 Validation Report 与提升门禁核心判定。
- [x] 实现公共浏览器与 Artifact 检查。
- [x] 实现 Website Validation Profile。
- [x] 实现 Docs Validation Profile。

当前实现说明：

- 每个内置模板从 `TemplateSpec.surface`、构建命令和静态输出目录派生不可变
  Generation Contract；
- `preview.publish` 对冻结 Candidate 执行公共浏览器检查，包括桌面/移动截图、
  字体就绪、可访问名称、横向溢出、同源链接、锚点、元数据和控制台错误；
- Website Profile 额外要求响应式布局、链接完整性和页面元数据；
- Docs Profile 额外要求 MDX/静态导出、导航、路由 slug 唯一性、内部链接、
  heading anchor、code block 和搜索入口；
- Validation Report 同时写入工作区 `state/validation-report.json` 便于修复，并以
  `runtime://validation-reports/...` 为身份原子持久化；
- `run.complete` 会重新读取持久化报告并按冻结模板契约复核，缺失、损坏、失败或
  unavailable 的必需检查都不能晋级；
- 验证采集或报告持久化中断时，暂存 Artifact 转为 `GarbageCollectable`，不会遗留
  可被完成动作误提升的 `Validating` Candidate。

### M4：持久化、预算与部署

- [x] Gateway 从 `emptyDir` 迁移到持久化数据库。
- [x] Candidate、报告和截图迁移到 Artifact Store。
- [x] 增加 Run 预算和协议错误熔断。
- [x] 增加 k3d/CI 声明式部署。
- [x] 增加 Website/Docs 回归矩阵。
- [x] 增加真实 Provider 凭据、预算与证据门禁（无需人工审批引用）。
- [x] 增加真实 Provider 定时门禁。

当前 M4 进展：

- 正式 Provider Gateway 清单继续使用 PostgreSQL；本地 k3d 使用单副本 SQLite +
  `ReadWriteOnce` PVC，部署策略为 `Recreate`，避免两个 Gateway Pod 同时写同一个
  SQLite 数据库；
- 增加 `apply-k3d-persistent-sqlite.sh`，迁移时先暂停 Runtime 写入，复制
  `gateway.db`、WAL 和 SHM，通过宿主机 SQLite backup API 压实并执行
  `PRAGMA quick_check`，恢复到 PVC 后才启动新 Gateway；
- 迁移脚本可幂等重放，并同时应用 Runtime → Provider Gateway 的地址和认证
  Secret patch；
- Runtime 的 Source Snapshot、Candidate/Promoted Artifact、Validation Report、
  desktop/mobile screenshot 和 WAL/checkpoint 均位于 `RUNTIME_STORAGE_DIR`，k3d
  清单已将该目录绑定到 `anydesign-runtime-storage` PVC；
- 新增 `RUNTIME_AGENT_MAX_TURNS`、`RUNTIME_AGENT_MAX_TOOL_CALLS`、
  `RUNTIME_AGENT_MAX_INPUT_TOKENS` 和 `RUNTIME_AGENT_MAX_OUTPUT_TOKENS` 四个硬预算，
  默认分别为 20、60、200000 和 40000；恢复执行时从持久化消息、ToolStarted
  和 `model.usage` 事件继续计数；
- 工具预算超限时，Runtime 会为模型已经发出的每个 tool call 写入对应失败
  tool result，再以 `partial` 结束 Run，避免破坏 Provider 协议；
- Token 用量优先使用 Provider Gateway 返回的标准化 input/output/cached usage；
  旧 Gateway、测试 fixture 或直连 Provider 未返回 usage 时，Runtime 使用保守估算并在
  `model.usage` 事件中标记 `estimated=true`。同一 turn 的重复事件按 turn 去重，
  避免恢复重试重复累计；
- Token 预算在执行模型返回的工具调用前检查；超限响应如果携带 tool call，仍先生成
  一一配对的失败 tool result，再将 Run 以 `partial` 结束；
- 新增 `RUNTIME_AGENT_MAX_CONSECUTIVE_PROTOCOL_ERRORS`，默认 3。JSON 参数解析失败、
  streaming 参数过大和 partial tool-call fallback 统一进入连续协议错误熔断器；
  健康响应清零计数，`model.protocol_error` 事件保存连续次数，Runtime 重启后继续
  累计；
- Gateway 已经具备持久化 daily input token quota、并发租约、单资源 circuit
  breaker、请求幂等和审计事件。Runtime Run 级模型轮次、工具调用、输入 Token、
  输出 Token 和连续协议错误预算现已闭环；
- k3d 的关键存储、Provider 连接、Runtime 部署、Secret 引导和 Website/Docs
  回归矩阵已收口为单一入口 `infra/generation-reliability/run-k3d-matrix.sh`；
- GitHub Actions 已增加 contract、fixture k3d，以及手动或定时触发的真实 Provider
  Job。定时执行由仓库变量 `ENABLE_SCHEDULED_REAL_PROVIDER=true` 一次性启用，不依赖
  Environment 人工审批或审批编号；默认关闭，避免在未授权周期性成本时自动消费。
  手动和定时真实门禁都接受不超过 5,000,000 的
  `RUNTIME_RC_REAL_TOTAL_TOKEN_CEILING`。每次真实 Run 启动前按 Runtime 输入/输出硬预算
  预留最坏情况 Token，下一次预留会越界时在发起 Provider 调用前失败关闭；预算证据写入
  `real-provider-token-budget.json` 并嵌入 Runtime RC 证据。

### M5：完成事务副作用闭环

- [x] 原子完成事务同步持久化 Run `completedAt`。
- [x] Repair 完成同步将 scoped finding 更新为 `fixed`。
- [x] 原子完成同步释放空闲 Sandbox binding。
- [x] 原子完成同步过期未处理授权并清理 Run scoped resources。
- [x] WAL 重放可恢复上述完成副作用。

M5 修复了 `preview.publish` 与 `run.complete` 同轮提交时的通用事务缺口。此前
Version、Run 和 outbox 已经原子落盘，但 Repair finding 等终态副作用仍依赖另一条
状态更新路径，导致 Run 显示 Completed 而 release evidence 无法成立。现在这些状态
与 promotion commit 一同写入 WAL，并通过重启回归测试验证。

### M6：统一 k3d / CI 生成矩阵

- [x] 单命令编排 k3d bootstrap、Runtime RC、npm 隔离、恢复门禁和证据聚合。
- [x] Website 与 Docs 使用独立 Run、Sandbox Pod 和 Artifact 并发验收。
- [x] Fixture Gateway 同时支持 legacy 和 versioned Provider Gateway contract。
- [x] 自动选择空闲 Runtime 本地端口，显式端口冲突时 fail closed。
- [x] Provider Secret 仅允许从环境或权限为 `0400/0600` 的 env 文件注入。
- [x] 失败自动收集脱敏的 Pod、事件、工作负载和有限日志。
- [x] 孤儿审计按运行前基线做差集，不误删或误报用户已有 SandboxClaim。
- [x] `auditPassed` 与 `releaseEligible` 分离，Fixture 可通过审计但不可被标记为发布。
- [x] CI 上传结构化 Runtime、Release、Deployment 和矩阵摘要证据。

最新本地验收使用镜像 `anydesign/runtime:reliability-m8-repair18-20260717`，最终摘要为
`services/runtime/target/e2e-evidence/zerondesign-e2e/generation-matrix-summary.json`。
验收结果覆盖 Website/Docs 内容与 computed style、取消清理、依赖代理、事件顺序、
Sandbox/Artifact 隔离、重启恢复、协议预算和 Secret 扫描，结果为 `pass`。

### M7：五个真实 Provider 用例验证

已生成并执行 5 个中文真实用例：3 个 Website、2 个 Docs。用例清单位于
`infra/generation-reliability/real-provider-cases.json`，执行器位于
`infra/generation-reliability/run-real-provider-examples.sh`。执行不要求人工审批；
批次 Token 配置仅作为实际用量安全上限，不是必须消耗的目标。

主要证据：

- `real-provider-examples-bounded-search`：3/5 通过，实际总用量 1,386,767 Token；
- `real-provider-examples-final-pass`：5/5 均完整执行，实际总用量 1,573,612 Token，
  但均被真实质量或可靠性门禁拒绝，没有把失败误报为成功；
- `real-provider-runs/suite-20260717015314630-failed`：schema v2 首批 5/5 完整执行，
  683,558 Token；暴露 Progress Fingerprint 对批量修改和首次唯一观察的误判，已修复；
- `real-provider-runs/suite-20260717021107107-failed`：schema v2 第二批 5/5 完整执行，
  1,975,391 Token；AI 治理控制台通过，其余 4 个有界失败，暴露 Candidate freeze 与
  Repair 生命周期冲突、冷构建 deadline 过短、可选文件探测和证据分类问题，均已进入
  本轮修复；
- `real-provider-runs/suite-20260717025440044-failed`：修复后定向执行 1 个 Website 和
  1 个 Docs，共 706,266 Token；确认 freeze 解锁、300 秒构建 deadline 和失败分类生效，
  并暴露 rejected Candidate 未重置进度窗口；
- `real-provider-runs/suite-20260717030742060-running`：单案例中断诊断，仅用于证明终态
  Tool 错误缺少 `run.completed` 且 SSE heartbeat 会掩盖 idle；未生成 summary，不计入
  完成批次或通过率；
- `real-provider-runs/suite-20260717033605304-failed`：部署工作流阶段和全局 36/8 观察
  预算后定向执行快速开始 Docs，共 404,706 Token，比同案例上次下降约 10.2%；真实产生
  Candidate，但 Generation 拒绝后又读 13 次、搜索 2 次且未修改源码，最终准确分类为
  `no_progress`；
- `real-provider-runs/suite-20260717081323801-failed`：运行环境漂移到
  `fixture-model-gateway`，`model.usage.estimated=true` 且
  `realProviderVerified=false`；该批仅作为 fail-closed 缺陷证据，595,421 估算 Token 不
  计入真实 API 累计；
- `real-provider-runs/suite-20260717081922562-accepted`：切回真实 Provider Gateway 并
  加入 URL/`model.execution` 双门禁后，Zenova Website 定向回归 1/1 accepted，实际
  443,337 Token；第一版内容被 Acceptance 拒绝，有限 Repair 后 24 项 Acceptance 与
  16 项必需 Validation 全部通过并原子 Promote；
- 所有临时 Runtime 预算、Principal Secret 和 SandboxClaim 在执行结束后恢复或清理，
  用户原有 SandboxClaim 保留。

本轮确认已落地的代码或运行修复：

1. Provider 工具名兼容不足：支持精确 Runtime 名、唯一规范化前缀和唯一错误 hash
   前缀，同时继续拒绝歧义或未请求工具；
2. 压缩后的消息窗口可能以孤立 `tool` 消息开头：Gateway 会合成最小配对
   `assistant.tool_calls` 历史；
3. DeepSeek thinking 历史要求导致 400：工具生成请求显式关闭 thinking；
4. `fs.search` 会递归扫描 `node_modules`、`.next` 等大型目录：现已排除依赖和构建
   目录，并限制文件数、字节数、匹配数及单行长度；
5. 真实用例执行器可能无限等待：增加 15 分钟 Run 超时和自动取消；
6. Provider 偶发返回非法工具参数：仅限流、不可用、超时和可恢复结构错误可在资源
   允许时重试一次；未知/未请求工具、策略违规和超大响应不重试，并把低敏错误原因写入
   `provider_attempt.failed` 审计；
7. ConfigMap 同 revision 不会覆盖持久化 ModelResource：仓库现已增加运行资源权威声明
   和审计化 reconcile，当前 k3d 已验证 `deepseek-v4-pro` revision 4、配置 digest、
   `maxAttempts=3`、策略 revision 3 与数据库 current revision 一致；
8. 真实证据中的 Tool 计数使用了错误事件名：已改为统计 `tool.started` 和不可恢复
   `tool.failed`；
9. 真实 Provider CI/RC 门禁已去除 Environment 人工审批和审批编号依赖，继续保留
   Secret、日配额、Run 硬预算、干净工作树与脱敏证据门禁。
10. 真实执行器现在精确校验 Runtime Gateway URL，并要求每个已执行 Run 都包含匹配
    ModelResource 的 `model.execution`；缺失上游证据时即使案例表面 accepted，整批也
    强制 failed。
11. 真实执行器启动前调用统一 Runtime Provider Gateway 模式入口：检查 Gateway Service
    和认证 Secret，应用受控 env patch、等待 Deployment rollout，并生成
    `runtime-provider-gateway-mode@1` 脱敏证据。Fixture→Real、切换后错误 URL 两条测试及
    当前 k3d 幂等验证均通过。
12. Runtime 基础 Deployment 已移除 Fixture Gateway URL；RC Fixture 链路必须显式应用
    `fixture-gateway-env-patch.yaml`，同时删除真实 Gateway auth env。当前 k3d 已实际完成
    Fixture generation 175 → Real generation 176，最终仅一个 Ready Pod，真实 auth Secret
    引用恢复并通过模式证据校验。

本轮临时诊断措施，不计入根因关闭：

- 真实测试配置提高到 60 回合、180 工具调用和单 Run 600k 输入 Token；
- 真实用例执行器设置 15 分钟总超时；生产 Tool deadline 和 Run watchdog 已独立落地；
- 外层执行器使用 `artifactBody.includes(expectedText)` 做最低限度业务门禁；该方式
  仅作为 Runtime Acceptance Contract 之后的外部二次探针。

仍需进入下一阶段的系统性问题：

- **P0 已落地且有真实成功路径：Brief 业务验收进入 Runtime 完成门禁。**
  `brief.write_draft` 现在必须生成结构化 Acceptance Contract；`preview.publish` 对尚未
  Promote 的 Candidate 执行路由和可见文本验收并持久化 `acceptance-report@1`；
  `run.complete` 会复核 Brief、Content Sources、Contract、Report 和 Candidate manifest
  digest。缺失报告、错误内容或身份不一致均不能 Promote；
- **P1 已落地、部分真实验证：Docs Agent 收敛控制。** 工作流阶段和全局读取/搜索预算
  已在真实快速开始 Docs 上生效，Token 降低约 10.2% 并成功产出 Candidate；但证据表明
  Repair 阶段仍会重新探索。现已进一步增加每个被拒 Candidate 默认 6 次读取、2 次搜索
  的 Repair 子预算，超限不阻断同批源码修改，并覆盖重启恢复；最新 Docs 2/2 真实通过；
- **P0 已落地：GitOps reconcile 自动化。** 部署脚本已固化“权威声明校验 → ConfigMap
  digest → Deployment rollout → 审计化 reconcile → current revision/策略/readiness
  复核”，并生成不含 Secret 的结构化证据；revision 或 digest 不一致时失败关闭。

### M8：根因闭环与真实回归

| 优先级 | 工作包 | 状态 | 主要交付物 | 完成证据 |
| --- | --- | --- | --- | --- |
| P0 | 配置与证据基线 | 已完成 | 真实 ModelResource 声明、自动 reconcile、evidence v2 | revision/digest 一致；证据可回溯到 commit 和配置 |
| P0 | Brief 内容门禁 | 已完成且真实成功路径已验证 | `acceptance-contract@1`、Candidate 验收器、原子完成复核 | 错误内容不能 Promote；AI 治理控制台真实通过并 Promote |
| P0 | 生产级收敛控制 | 已完成，阈值继续校准 | Tool deadline 矩阵、Run watchdog、Progress Fingerprint、取消传播 | 慢工具和无进展循环在阈值内结构化结束；误判回归已修复 |
| P1 | 有限 Repair | 已完成，真实生命周期缺陷已修复 | Candidate 局部 Repair、3 次预算、校验失败重开 Run | Repair 不改变 Brief；耗尽 fail closed；源码可在拒绝后继续修改 |
| P1 | Provider 重试矩阵 | 已完成 | 类型化错误分类、表驱动测试、低敏审计 | 可恢复错误重试一次；未知工具/策略/超大响应不重试 |
| P1 | Docs 收敛控制 | 已完成且真实验证 | Runtime 工作流阶段、下一动作、全局与 Repair 分类观察预算及恢复 | 观察超限不阻断写入；Docs 2/2 真实 accepted |
| P0 | 生产晋级旁路收口 | 已完成 | 生产工具快照隐藏 `preview.report_candidate`；执行层类型化拒绝；调用方迁移 `preview.publish` | Production/LocalE2E 快照对照、Website/Docs 生命周期、Preview 24 项与 Fixture 防回退测试通过 |
| P0 | Candidate 验证路径一致性 | 已完成 | 保留 `/previews/<lease>/` 路由前缀；内部 loopback capture；Docs/Website 验证 fixture | 真实 Chromium 路由隔离测试、Website/Docs 定向与并发矩阵通过 |
| P0 | 长期运行与重启恢复 | 已完成 | ChannelLease 单次 hydrate/心跳合并；Brief 冷缓存锁作用域修复；Edit Brief 继承 | 保留 24.5 MB 历史日志并发通过；Runtime restart 后 Edit 原子完成通过 |
| P0 | Run WAL 热路径与崩溃恢复 | 已完成 | `runs.jsonl` 单次 hydrate；同步追加；截断尾记录修复；完整坏记录失败关闭 | repair16 锁饥饿失败证据保留；repair18 在约 17 MB 历史 WAL 上并发/重启矩阵通过；二次重启继续写入通过 |
| P0 | 浏览器进程隔离 | 已完成 | Chromium 全局进程锁；截图 30 秒 timeout；drop-kill | browser 10 项、Website/Docs 并发和 DCP 矩阵通过 |
| P1 | 未改源码重复发布熔断 | 已完成 | 持久化 Generation/Acceptance 失败 fingerprint；未变源码 fail-fast | 同 fingerprint 不再构建或启动浏览器；重启后仍拒绝；修改源码后可继续 Repair |
| P1 | 真实回归 v2 | 已完成 | 真实五案例批次、流式事件、provenance、Gateway 防漂移、机器化稳定性退出审计和问题回灌 | 连续三批完整五案例 5/5 accepted；`generation-real-provider-stability-audit@1` 为 3/3 passed、false success 0 |

定向批次后的补充闭环：唯一 rejected Candidate digest 现在会重置一次进度窗口；终态
Tool 错误由 Agent Loop 幂等补写 `run.completed`；evidence idle timer 只由解析成功的
业务事件刷新，不再由 SSE heartbeat 刷新。三项均有自动化测试，仍需下一轮真实批次
验证组合效果。

最新 Docs 证据又增加一项闭环：进入 `repairing_candidate` 后启用独立 6/2 观察子预算；
第三次超限读取的回归证明不会取消同批 `fs.multi_patch`，持久事件恢复也会同时恢复全局
和 Repair 用量。生产手工 Candidate 晋级旁路也已关闭：`preview.report_candidate` 只允许
显式 LocalE2E 底层测试，生产调用返回 `preview.manual_candidate_retired`。Agent Loop
53 项 Agent、24 项 Preview Promotion、176 项 Runtime lib、25 项 Checkpoint、96 项
Sandbox 集成测试及 10 项 browser 定向回归均通过；Provider 为 42 项通过、2 项忽略。
修复已部署到本地 k3d 镜像
`anydesign/runtime:reliability-m8-repair32-20260718`，Deployment Ready 1/1；最新
`generation-reliability-matrix@1` 摘要结果为 `pass`，Website/Docs 并发执行、恢复门禁和
release audit 均通过。
未改源码的 Generation/Acceptance 熔断、Run WAL 单次 hydrate 和截断尾记录恢复均已加入
PR `contracts` Job 与 k3d recovery gate；`check-contract.sh` 会反向检查这些测试入口仍然
存在，避免以后只保留实现而悄悄移除防回退门禁。
`/version` 的实测 provenance 为 commit `ff7e7dc8713ba72110d0cc68fd99b8e855931258`、
`repositoryDirty=true` 和上述 image ref；文档不把 dirty 工作树描述为可发布构建。

Acceptance Contract 已有以下自动化证明：结构化契约确认后持久化并可在重启后恢复；
正确可见内容可生成 Candidate 并完成原子 Promote；必需文案仅出现在 `script` 时仍按失败
处理；验收失败不会设置 output/current version；删除持久化验收报告后 `run.complete`
无法绕过，恢复同一份报告后才允许完成。

推荐状态机：

```text
BriefFrozen
  → BuildingCandidate
  → ValidatingGeneration
  → ValidatingAcceptance
  → RepairingCandidate (bounded)
  → ReadyToComplete
  → Completed + Promoted (atomic)

任何必需检查失败或预算耗尽
  → Failed | Partial
  → current version unchanged
```

## 11. 后续 PR 拆分与依赖

已完成的 M1-M6 变更继续保留原有提交历史。M8 新工作按以下顺序拆分：

1. `generation-evidence-v2-and-provider-resource-source-of-truth`
2. `provider-resource-reconcile-and-readiness-gate`
3. `brief-acceptance-contract`
4. `candidate-content-validation-before-promotion`
5. `runtime-tool-deadline-and-progress-fingerprint`
6. `bounded-candidate-repair`
7. `provider-response-retry-classification`
8. `unchanged-candidate-validation-failure-fuse`
9. `runtime-wal-single-hydration`
10. `real-provider-regression-v2`

当前 1—10 已完成代码、自动化与真实 Provider 闭环。普通用户式 Prompt 回归先通过
`suite-20260718010215419-accepted`，随后完整批次
`suite-20260718011716291-failed` 暴露复合 MDX 组件兼容缺口；该问题通过通用模板 p5、
写入约束和真实 Next build 冒烟修复，并由定向
`suite-20260718014729601-accepted` 验证。最终
`suite-20260718015800609-accepted`、`suite-20260718021539164-accepted`、
`suite-20260718023350592-accepted` 连续三批均 5/5 accepted，证明 Website/Docs 可在
普通业务需求下完成 Brief、Build、内部验收、有限 Repair 和原子 Promote；稳定性审计
现为 3/3 passed。

每个 PR 必须包含：单元测试、失败注入测试、状态/数据迁移说明、回滚方式、可观测事件
以及对当前正式版本不受失败 Candidate 影响的证明。Owner 和目标版本在进入开发排期时由
项目负责人补充，未指定 Owner 的工作包不得进入 `in_progress`。

## 12. 完成定义

整体修复只有在以下条件全部满足时才算完成：

- Website 和 Docs 共用统一状态机和验证报告；
- Candidate 与 Promoted 完全分离；
- Run Complete 与 Version Promote 原子提交；
- Provider 差异由 Adapter 处理；
- 所有必需检查必须为 `passed`；
- 多语言桌面和移动渲染可验证；
- 重启后产物、状态和计量不丢失；
- CI 覆盖成功、失败、超时、重启和并发场景；
- 真实 Provider 失败不会污染当前正式版本或造成无边界消耗。

上述原则必须通过以下量化退出标准验证：

| 维度 | 退出标准 |
| --- | --- |
| 内容正确性 | 缺少必需内容、错误语言、模板残留的 Candidate 100% 在 Promote 前被拒绝 |
| 事务安全 | 验收失败、超时、取消、重启和 CAS 冲突时 current version 均保持不变 |
| Docs 收敛 | 两个 Docs 真实案例均在配置预算内生成 Candidate；无进展时在阈值内进入 Repair 或结构化结束 |
| Tool 边界 | 慢 `fs.search` 等工具受到整体 deadline 约束，取消后不继续消耗 Run 预算 |
| 重复修复熔断 | Generation/Acceptance 失败且源码 fingerprint 未变时，不重复构建或启动浏览器；源码变化后可继续 Repair |
| 长期运行 | 保留大体量 ChannelLease/Run WAL 时并发 Build/Edit、状态轮询和重启恢复仍在门禁时限内完成；最后半条 WAL 被截断时可修复并继续写，完整坏记录失败关闭 |
| Provider 配置 | 仓库声明、配置 digest、数据库 current revision 和运行时审计记录一致 |
| Provider 重试 | JSON/可恢复结构错误按策略最多重试一次；未知或未请求工具等策略错误不重试 |
| 证据可信度 | evidence schema v2，Tool 计数正确，事件流增量落盘，包含 commit/config/contract digest |
| 真实回归 | 建议连续 3 批五案例均完成并通过 Runtime 内部验收，且 false success 为 0 |

专项关闭前必须完成一次文档复核：所有“已完成”事项都必须有可访问证据；临时诊断措施
不得继续出现在“已修复”列表；真实失败批次不得使用 `pass` 或 `final-pass` 命名。

上述真实回归标准现由 `audit-real-provider-stability.mjs` 机器执行：只计
`generatedCaseCount=5`、`executedCaseCount=5`、`partial=false`、每个 Run 均有真实
ModelResource 证据、5 个案例均 accepted、Artifact HTTP 200 且命中必需文本、预算未越界
的完整批次。定向批次不增加也不清零连续计数；后续完整失败批次会清零。当前权威输出为
`real-provider-runs/real-provider-stability-audit.json`，状态 `passed`、连续通过 `3/3`、
false success `0`、malformed summary `0`。本次通过关闭当前阶段，不取消持续门禁：未来
完整失败批次仍会清零后续连续计数，并要求重新取得三批合格结果后才能再次声明稳定。
