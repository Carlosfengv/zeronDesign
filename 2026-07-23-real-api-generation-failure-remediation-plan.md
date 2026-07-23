# 真实 API 生成失败修复计划

日期：2026-07-23
范围：Runtime、Provider Gateway、Agent Sandbox、模板工作流、发布控制面、Ingress，以及真实 API 端到端验证。

## 1. 背景与目标

本计划针对两类真实项目（Monorepo Docs 与 Shanghai Citywork）共四次主要生成测试中的失败：

| 测试 | Run | 结果 | 直接失败点 |
| --- | --- | --- | --- |
| Docs 首次 Build | `run-676527888` | 失败 | 冻结 Build 身份缺少 `templateId` |
| Citywork 首次 Build | `run-676527900` | 失败 | 冻结 Build 身份缺少 `templateId` |
| Docs 再生成 | `run-676528159` | 部分完成 | Agent 在 Fumadocs 文件工作流中无进展 |
| Citywork 再生成 | `run-676528171` | 失败 | Provider Gateway 返回 `400 invalid_turn_request` |

另有一次 Docs 构建已实际进入 `next build`，但在 ARM64 Sandbox 中加载 Next SWC 原生模块时触发 `SIGBUS`（退出码 135）。这说明即使 Agent 和网关问题修复，发布仍会被构建环境阻断。

目标是在不降低真实 API、真实模型、真实 Sandbox 与真实发布链路覆盖度的前提下，稳定完成：

1. Brief 确认后启动 Build；
2. 生成源码并完成依赖安装；
3. 通过项目生产构建与 Preview 验证；
4. 创建 Release 并返回可访问 URL；
5. 为每一步提供可关联的 Run、Provider Request 与 SSE 证据。

## 2. 总体实施顺序

```text
P0 Generation Context 冻结链路与失败证据
        ↓
P0 Sandbox Next/SWC 兼容性验证与镜像修复
        ↓
P1 Provider 400 最小重放与协议修复
        ↓
P1 Agent 受控工作流与恢复策略
        ↓
P2 URL 基础设施与真实 API 端到端回归
```

前两项是硬门槛：前者决定 Run 能否获得完整、不可变的生成输入，后者决定生成结果能否完成生产构建。Provider 与 Agent 修复必须先取得可重放证据，再修改协议或恢复策略；最后补齐发布 URL 的解析、TLS 与探测闭环。

## 3. 工作流 A：Generation Context 编译与冻结完整性

### 问题

首次 Docs 和 Citywork Build 在 Sandbox 初始化前失败：Run 的冻结 Generation Context 没有可用的 `templateId`，后续也出现过 `appRoot` 缺失。现有 Runtime 已具备从 Brief、Design Profile、DCP 和 Project Runtime State 编译这些字段的逻辑，因此问题不是缺少另一套持久化 Build Identity，而是 `StartRun → Generation Context 编译 → Run 冻结 → Agent Bootstrap` 链路没有保证同一份完整上下文被原子地保存并消费。

### 目标状态

Generation Context 是 Build Run 唯一的冻结输入。各字段的事实源和必填条件如下：

| 字段 | 事实源 | 必填条件 |
| --- | --- | --- |
| `briefId` / Brief revision | 已确认 Brief | 所有 Build |
| `templateId` | 有效 Design Profile 模板，否则 `brief.recommended_template` | 所有 Build |
| `appRoot` | DCP `expectedAppRoot`，否则 Project Snapshot，最后为模板默认值 | 所有 Build |
| Content Approval / Content Plan identity | Run 的审批状态与已冻结 Content Plan | 启用强制 Content Plan 的 Build |
| Design Context revision/hash | 已冻结 DCP | 使用 DCP 的 Build |
| Project Runtime State snapshot | Project Runtime State | 已初始化或继承项目状态时 |

不得在 Brief 侧新增一份与 Generation Context 并行的 Build Identity。Agent Bootstrap 只能读取已冻结 Generation Context；不得在执行过程中重新查询可变数据来改变模板或根目录。

### 实施项

1. 建立失败链路的回归夹具。
   - 分别覆盖 BFF 发起和直接 Runtime API 发起的 Build。
   - 重现 `templateId`、`appRoot` 丢失，并记录 Run 创建、Generation Context 编译和 Sandbox 创建的顺序。
   - 对照 `run-676527888`、`run-676527900` 和中间诊断 Run 的事件结构固定回归输入。

2. 统一 Generation Context 编译入口。
   - 所有 Build 入口都调用同一编译函数，不允许 BFF 与直接 Runtime API 使用不同路径。
   - 在创建可运行 Run 和 SandboxClaim 前完成 Brief、模板、Content Approval、DCP 与 Project Snapshot 校验。
   - 编译结果与 Run 的不可变身份在同一持久化边界内保存；启动 Agent 前重新读取并校验保存后的结果。

3. 增加条件化完整性校验。
   - 缺所有 Build 必填字段时返回 `generation_context_identity_incomplete`。
   - 只有启用 Content Plan 或 DCP 的 Run 才要求对应 identity；不能阻断未启用这些能力的合法 Build。
   - 错误响应包含缺失字段、字段事实源、Brief ID、Run ID（若已分配）和可恢复操作。
   - 校验失败时不得创建 SandboxClaim，也不得向 Provider 发起请求。

4. 收紧 Agent Bootstrap。
   - Bootstrap 只能从 `run.generation_context.payload.identity` 读取 `templateId` 和 `appRoot`。
   - 当前从 Brief 或默认 `project` 回退的代码仅保留在迁移 feature flag 下，并记录指标。
   - 无冻结 identity 时终止为明确的 Runtime 数据错误，不能静默选择模板。

5. 审计历史 Run，而不是迁移 Brief。
   - 枚举已有 Build Run，报告完整、可重建、歧义三类 Generation Context。
   - 已终止 Run 保持不可变；重新生成时创建新 Run 并通过统一编译器冻结新上下文。
   - 不修改已确认 Brief 来伪造历史 Run 当时不存在的身份。

### 涉及模块

- `services/runtime/src/run_lifecycle/start.rs`
- `services/runtime/src/generation_context.rs`
- `services/runtime/src/content_plan_approval.rs`
- `services/runtime/src/agent_loop.rs`
- Run、Brief 与 Project Runtime State 的持久化实现

### 验收标准

- BFF 与直接 Runtime API 启动的 Build 使用相同的 Generation Context 编译路径。
- Docs 与 Citywork Run 在启动 Agent 前均具有完整、已冻结的条件化 identity。
- 缺失字段的测试返回确定错误，不产生 SandboxClaim 或 Provider Request。
- Build 初始化不再出现 `missing templateId` 或 `missing appRoot`。
- Agent Bootstrap 不再从可变 Brief 数据重新推断身份。
- 历史 Run 审计报告可重复执行且不修改历史记录。

### 回滚

以 feature flag 控制“严格冻结 identity”。回滚时只恢复旧 Run 的只读回退，不删除已冻结 Generation Context，也不修改历史 Brief。

## 4. 工作流 B：ARM64 Sandbox 中的 Next.js / SWC 构建环境

### 问题

Docs 在真实 Sandbox 内执行 `next build` 时触发：

```text
Bus error
exit code 135
```

同一 Sandbox 中直接 `require('@next/swc-linux-arm64-gnu')` 也会触发 `SIGBUS`。因此根因位于 Next SWC 原生模块、Node、libc、CPU 架构或镜像打包组合，而非生成的 MDX/JSX 源码。

### 目标状态

每一个发布到集群的 Sandbox 镜像都能在目标架构上加载 Next SWC 并完成一个最小 Next 生产构建。

### 实施项

1. 在镜像 CI 中运行带锁定依赖的构建夹具。

```sh
npm ci
node -e "require('@next/swc-linux-arm64-gnu')"
npm run build
```

上述示例适用于 glibc ARM64 镜像；musl 镜像只验证 `@next/swc-linux-arm64-musl`。CI 必须使用最小 Next fixture 和 Fumadocs fixture 安装项目依赖，不能假设 Sandbox 基础镜像预装 `@next/swc-*`。

2. 建立两层 Smoke Test。
   - 镜像 CI：在目标 CPU 架构与 libc 上执行最小 Next 和 Fumadocs 生产构建。
   - Runtime：`project.ensure_dependencies` 完成后、生产构建前，加载与当前 libc 匹配的 SWC 模块。
   - Runtime 预检失败时直接分类为环境故障，不让 Agent 修改源码。

3. 固定并验证兼容矩阵。
   - 锁定 Node、npm、Next、`@next/swc-*` 与 libc 版本。
   - 生成 lockfile，并验证包完整性与目标架构。
   - 禁止模板使用会在安装时漂移的 `^` 版本作为生产构建的唯一约束。

4. 选择并落实一个生产方案。
   - 优先：发布经过 ARM64 验证的 Sandbox 基础镜像与兼容的 SWC 包。
   - 若短期无法解决：将 Next 生产构建调度到已验证的 x86_64 Builder，再把静态产物交给既有发布链路。
   - 仅在经过完整 Smoke Test 后采用 SWC WASM 回退；不得将其作为未经验证的临时开关。

5. 在依赖恢复后、`project.build` 或 `preview.publish` 内部 Build 前做快速预检。
   - 失败分类为 `environment.next_swc_unavailable`。
   - 返回架构、Node/Next/SWC 版本与预检命令，不要求 Agent 修改源码。

### 涉及模块

- `infra/agent-sandbox/`
- `services/runtime/src/tools/sandbox/project/build.rs`
- `services/runtime/src/tools/sandbox/preview/`
- `services/runtime/src/templates/fumadocs_docs/files/package.json`
- `services/runtime/src/templates/next_app/files/package.json`

### 验收标准

- ARM64 真实 Sandbox 在依赖恢复后能加载与 libc 匹配的 SWC 模块。
- 最小 Next、Fumadocs Docs、Next App 三个构建 Smoke Test 均通过。
- 真实 Docs Run 通过 `preview.publish` 内部拥有的生产 Build 和 Candidate 验证。
- 不再产生 `SIGBUS` / exit 135，且构建环境问题不会被误归类为源码错误。

### 回滚

镜像使用不可变 tag 与灰度部署；预检失败时阻止流量切换并保留上一个通过 Smoke Test 的镜像。

## 5. 工作流 C：Provider Gateway 的 Tool-call 轮次协议

### 问题

Citywork 在已写入页面源码后，Provider Gateway 返回 `400 invalid_turn_request`。当前证据只能确认供应商拒绝了该轮请求，尚不能确定是 tool-call 顺序、工具 schema、重复 ID、角色组合、请求大小还是 Provider 适配问题。

### 目标状态

无论工具成功、失败、超时、被策略拒绝或 Run 恢复，发送给模型的消息序列都保持提供商可接受且可审计。

### 实施项

1. 先完成证据采集与最小重放。
   - 保存供应商错误码、脱敏错误正文、Provider Request ID、Run ID、turn 和消息结构指纹。
   - 保留脱敏后的 role 序列、tool-call ID、tool schema hash、请求字节数和 token 估算。
   - 将失败请求缩减为可在测试环境重放的最小夹具，并先确认具体违反的供应商契约。

2. 根据证据在 Gateway 适配层建立请求序列校验器。
   - 每个 assistant tool call 必须具有唯一 call ID。
   - 每个 call ID 必须获得恰好一个同 ID 的 tool result。
   - 一个含多个 tool call 的 assistant turn 后可以跟一组连续 tool results；进入下一个非 tool 消息前必须结算全部 call ID。
   - 恢复/重试不得重复发送已结算的 call ID。

3. 统一工具异常的协议化结果。
   - 对策略拒绝、超时、参数错误和内部异常，均生成提供商格式的 tool result。
   - tool result 包含稳定错误码和安全摘要，不能仅在 Runtime 本地记录失败而省略回传。

4. 将 Provider-specific 转换与领域状态分离。
   - Runtime 保存规范化 tool timeline。
   - Gateway 负责将 timeline 转为 DeepSeek 所要求的消息格式。
   - 在发出请求前执行纯函数校验，失败则在本地返回可诊断错误，而不是向提供商发送无效负载。

5. 强化可观测性与可重放性。
   - 记录 Run ID、turn、Provider Request ID、模型资源 ID、工具 call ID 列表和脱敏的结构摘要。
   - 对 400 保存结构指纹与校验器输出；不得记录密钥或完整用户敏感内容。
   - 只有本地校验器能够证明并修正结构问题时才允许一次安全重试；未知 400 不自动重试。

### 涉及模块

- `services/runtime/src/model_gateway.rs`
- Provider Gateway 的 DeepSeek 适配与请求审计模块
- `services/runtime/src/agent_loop.rs`
- `services/runtime/src/tools/streaming.rs`

### 验收标准

- 失败请求能够以脱敏夹具稳定重放，并得到明确根因分类。
- 单工具、多工具、工具拒绝、超时和恢复五种序列均有适配层单元测试。
- 发送前校验能拦截无效 tool-call 序列。
- 真实 Citywork Build 不再收到 `invalid_turn_request`。
- SSE 事件可通过 Run ID 与 Provider Request ID 关联到网关审计记录。

### 回滚

先上线脱敏证据采集，再以只读审计模式运行校验器；确认根因和误报率后启用阻断与规范化。保留旧适配器开关以便紧急切换。

## 6. 工作流 D：Agent 受控工作流与无进展恢复

### 问题

Fumadocs Docs 模板的实际可编辑文件为 `.jsx`，但模型尝试读取不存在的 `.tsx` 文件。Runtime 已拒绝部分不允许的工具调用，但这些拒绝仍消耗回合，最终触发 `no_progress`，导致生成提前中断。

### 目标状态

Agent 可获得准确、受约束的模板表面；一次无效读取不会导致 Run 报废；系统在模型偏离工作流时能提供确定的恢复路径。

### 实施项

1. 从模板生成可编辑表面清单。
   - `project.init` 返回模板的真实文件、扩展名、必需路由、允许写入范围和关键组件映射。
   - 将清单作为结构化数据注入 Generation Context，不依赖模型从自然语言提示推断。

2. 让工作流约束可执行。
   - 对未授权路径、已知不存在路径、禁止的工具阶段，返回下一步可执行工具与精确候选路径。
   - 这些策略拒绝不计入“连续无实质进展”。
   - 对同类违规设上限，超过后进入一次受控恢复，而非无限重试。

3. 建立恢复状态机。
   - 在首次策略拒绝后，追加一条结构化恢复指令。
   - 在多次拒绝后，重新给出最小必需源文件观察和未完成验收项。
   - 只有在恢复后仍没有源码变更、构建结果或候选版本时才标记 `partial`。

4. 提升进展判定质量。
   - 合法的源码写入、构建完成、Preview 发布、恢复快照创建均计为实质进展。
   - 无效读取、重复读取、被拒绝的目录遍历不能单独推动或终止进展计数。
   - 在终止前保存可恢复工作区快照和结构化未完成清单。

### 涉及模块

- `services/runtime/src/agent_loop.rs`
- `services/runtime/src/agent_hooks.rs`
- `services/runtime/src/templates/fumadocs_docs/`
- `services/runtime/src/tools/sandbox/`

### 验收标准

- Fumadocs Build 的模型上下文显式包含 `.jsx` 可编辑表面。
- 一次错误路径读取后，Run 能继续完成源码写入与 Preview。
- 策略拒绝不会被错误计为无进展。
- `partial` 事件带有恢复快照与未完成项，能够通过继续 Run 恢复。

### 回滚

先以事件记录形式启用新的进展分类；观察确认后再改变终止计数。恢复状态机须可通过 feature flag 关闭。

## 7. 工作流 E：真实 API 端到端回归、发布与观测

### 目标状态

两个固定场景都通过真实项目 API、真实 Provider、真实 Sandbox、真实 Preview 和真实 Release 完成，并能交付 URL 与 SSE 信息。

### 场景

1. **Monorepo Docs**
   - 模板：`fumadocs-docs`
   - 必需页面：概览、快速开始、架构、工作区与包、开发、部署、API、FAQ。
   - 关键验收：中文内容、路由完整、MDX 生成成功、Preview 可访问。

2. **Shanghai Citywork**
   - 模板：`next-app`
   - 必需内容：`Shanghai Citywork`、`加入社区`、`城市灵感`、`本周精选活动`、`社区故事`。
   - 关键验收：页面可访问、移动端布局、Preview 与发布版本一致。

### 实施项

1. 建立分层真实 API 验收脚本。
   - 创建 Project 与 Brief。
   - 确认 Brief。
   - 验证已冻结 Generation Context 及其条件化 identity。
   - 启动 Build，订阅 SSE。
   - 根据模板生命周期验证 Provider Request、源码、依赖安装、Build/Draft、Preview、Release 与 Publication。

2. 按模板执行正确的生命周期。

| 模板 | Agent 阶段 | 候选/草稿阶段 | 发布阶段 |
| --- | --- | --- | --- |
| `fumadocs-docs` | 修改源码后调用 `preview.publish`；不得预先单独调用 `project.build` | `preview.publish` 内部完成依赖恢复、生产 Build、Preview 验证和 Candidate | 用户发起 Release / Publication |
| `next-app` | 恢复依赖并调用 `project.build` | 启动 managed Dev，等待最新 revision Ready，并形成 Durable Draft | 用户发起 PublishWorkflow，生产构建并生成 WorkVersion |

3. 为每一阶段设定明确门禁和失败分类。

| 阶段 | 成功证据 | 失败归属 |
| --- | --- | --- |
| Brief | confirmed Brief / identity | 产品或 Runtime 数据 |
| Bootstrap | Sandbox ready / `project.init` | Runtime / Workspace |
| Agent | 源码变更与工具事件 | Agent / Gateway |
| Build | 成功的生产构建日志 | 源码 / 依赖 / Sandbox |
| Preview | 候选版本与页面探测 | Runtime / Preview |
| Release | version / public URL | 发布控制面 / Ingress |

4. 保留测试工件。
   - Run ID、Brief ID、Project ID、Provider Request ID。
   - 脱敏 SSE 记录与构建日志路径。
   - Preview URL、Release ID、最终公共 URL。

5. 增加发布后探测。
   - HTTP 200、关键文本与路由探测。
   - 静态资源可加载。
   - 公开 URL 与项目版本绑定正确。

6. 明确 URL 环境与可访问性标准。
   - RC / 本地 E2E 使用 `works.zerondesign.localhost`、显式 HTTPS 端口和测试 CA；测试报告必须给出完整 URL、CA 证据及解析方式。
   - `works.fixture.invalid` 仅能作为不可路由夹具域名，不能计为“可访问 URL”。
   - Staging / Production 必须配置 wildcard DNS、Ingress、有效 TLS 证书和外部 HTTP 探测。
   - 将“集群内可访问”“本机可访问”“互联网可访问”作为三个不同级别记录，最终交付不得混用。

### 验收标准

- 两个场景连续三次完整通过。
- 每次均有本机可访问的 Preview URL 和 Release URL；若目标为 Staging / Production，还必须通过外部 DNS/TLS 探测。
- 没有 `templateId` / `appRoot` 缺失、`invalid_turn_request` 或 SWC `SIGBUS`。
- 失败时 10 分钟内能从 Run 与 SSE 定位到一个明确责任层。

## 8. 发布策略、监控与责任边界

### 发布策略

1. 先发布 Generation Context 冻结校验与观测；
2. 再灰度 Sandbox 镜像预检与兼容矩阵；
3. 以只读模式审计 Provider 消息序列，再启用规范化；
4. 最后启用 Agent 恢复状态机；
5. 通过两个真实场景三连测后扩大范围。

### 监控指标

- `generation_context_identity_incomplete` 次数与字段分布；
- Sandbox SWC 预检失败率；
- Provider `invalid_turn_request` 比率与关联模型资源；
- Agent 工具拒绝率、恢复成功率、`no_progress` 终止率；
- Build 成功率、Preview 成功率、Release 成功率和端到端耗时。

### 责任边界

| 问题层 | 首要责任模块 |
| --- | --- |
| Generation Context | Runtime 生命周期与持久化 |
| SWC / Next 构建 | Agent Sandbox 镜像与构建工具链 |
| Tool-call 协议 | Provider Gateway 适配层与 Runtime 模型网关 |
| 无进展恢复 | Agent Loop / Agent Hooks / 模板元数据 |
| 交付 URL 与可观测性 | 发布控制面、BFF 与 Ingress |

### 交付拆分与预估

以下为单一工程师等效工作日的初始估算，实施前由对应模块负责人确认：

| 交付包 | 建议 PR 拆分 | 责任角色 | 预估 |
| --- | --- | --- | --- |
| A1 | 失败夹具、统一 Generation Context 编译入口与条件校验 | Runtime | 2–3 天 |
| A2 | Bootstrap 严格冻结读取、历史 Run 审计与指标 | Runtime | 1–2 天 |
| B1 | ARM64/glibc fixture、lockfile 与镜像 CI 门禁 | Platform / Sandbox | 2–3 天 |
| B2 | Runtime SWC 预检、错误分类与灰度镜像 | Platform + Runtime | 1–2 天 |
| C1 | Provider 400 脱敏证据、最小重放夹具 | Gateway | 1–2 天 |
| C2 | 基于证据的协议校验、规范化和回归测试 | Gateway + Runtime | 2–3 天 |
| D1 | Editable Surface 强约束、恢复状态机与进展判定 | Agent Runtime | 3–4 天 |
| E1 | 双模板真实 API 回归脚本、URL/TLS 探测与证据归档 | QA / Runtime / Platform | 2–3 天 |

每个交付包独立测试、独立 feature flag 或镜像 tag，不能将所有修复合并为一个不可回滚的大变更。

## 9. 完成定义

本计划完成的标志不是“Run 不再报错”，而是以下条件全部满足：

1. Docs 与 Citywork 均通过真实 API 创建、确认、生成、构建、Preview 和 Release；
2. 两个项目均有符合目标环境可访问性等级的最终 URL；`fixture.invalid` 不计为成功；
3. 每个 Run 均能提供脱敏 SSE 流与 Provider Request 关联信息；
4. 对不完整身份、网关协议错误、Sandbox 原生依赖错误和 Agent 偏离工作流，均有明确、可操作且可观测的失败结果；
5. 两个场景连续三次端到端回归通过。

## 10. 2026-07-23 实施与真实 API 验证记录

### 已实施修复

1. Generation Context 对已确认 Brief 补齐并冻结 `templateId`、`appRoot` 等运行身份，兼容 Brief 中异构 `pageStructure`。
2. ARM64 Sandbox 在 Next.js 构建前校验真实 SWC 原生包，并对不完整依赖恢复执行一次干净重试。
3. Provider Gateway 在发往 Provider 前规范化结构化 Runtime 控制消息，消除 `invalid_turn_request`。
4. Sandbox 释放等待 SandboxClaim、Sandbox 和相关 PVC 全部消失，避免 15Gi 配额被已释放工作区占用。
5. Workspace channel 的终态进程租约可安全重启；Next.js 相同内容的 `404.html` 与 `404/index.html` 采用稳定别名规则。
6. Candidate Preview 的远端 HTTP 探测连接 capture listener 并显式设置 lease Host；重复路由校验复用相同的 Next.js 404 别名规则。
7. Fumadocs Candidate 验证完成后由 Runtime workflow driver 确定性执行 `run.complete`，不再让模型继续读取并耗尽输入预算。
8. Next.js Build 修复流收紧为：读取一次 `diagnostics.build_log`、对明确的 `project/` 源码目标执行一次受控变更、自动重建；成功重建后清理 repair 状态并继续 managed Dev。
9. 恢复本地 RC 的 release packaging host helper，使已有 `reconcile_required` Release 原地继续完成，而不是重新生成项目。

### 真实运行结果

| 场景 | Project | 成功 Run | 结果 | Release |
| --- | --- | --- | --- | --- |
| Monorepo Docs | `978a2782-6861-4f20-b5da-3ac9e68331bf` | `run-676528936` | `completed`；Candidate、8 个文档路由和 requiredText 验证通过 | `release-d1d7d0741eba4e5f755122e8d6965c9b` |
| Shanghai Citywork | `79eb5ebf-538b-43fd-ad27-23c5bd23782f` | `run-676529279` | `completed`；production build、managed Dev Ready、Durable Draft 均通过 | `release-38d0df66f37ae2d5a5eccbb1066e55ff` |

SSE：

- Docs：`http://127.0.0.1:18080/runs/run-676528936/events`
- Citywork：`http://127.0.0.1:18080/runs/run-676529279/events`

发布控制面 URL：

- Docs：`https://w-a126cc559635236708db.works.fixture.invalid`
- Citywork：`https://w-325e11c5e73e501271f9.works.fixture.invalid`

本机直接可访问 URL：

- Docs：`http://127.0.0.1:19081/docs/`
- Citywork：`http://127.0.0.1:19082/`

发布后证据：

- 两个 Deployment 均为 `1/1 Ready`、`1/1 Available`。
- 两个 Work 的 `desiredGeneration=1`、`observedGeneration=1`，状态均为 `published`。
- 使用 RC CA 对发布域名进行 TLS/SNI 验证，两个入口均返回 HTTP 200。
- Docs 的 `/docs/`、`quick-start/`、`architecture/`、`workspaces/`、`development/`、`api/`、`deployment/`、`faq/` 均返回 HTTP 200。
- Docs HTML 包含 `MonoKit`、`核心包概览`；Citywork HTML 包含全部七项 requiredText。
- Runtime 全量 library 回归：333 passed、0 failed、2 ignored。

### 尚未宣称的验收项

本次已经完成一轮连续的双场景真实 API 成功闭环。计划中的“每个场景连续三次端到端通过”仍属于扩大灰度前的稳定性门禁，不能用本次单轮结果替代。`works.fixture.invalid` 仍是 RC 发布控制面域名；对当前本机而言，交付的无 DNS 依赖访问入口是上面的 `127.0.0.1` URL。
