---
date: 2026-07-04
topic: internal-ai-website-docs-generator-architecture
status: draft
source_requirements: ./2026-07-04-internal-ai-website-docs-generator-requirements.md
---

# Internal AI Website / Docs Generator 技术架构方案

## 1. 架构结论

该产品应采用 **Next.js 产品控制面 + Kubernetes/agent-sandbox 执行面** 的双平面架构。

- 产品控制面负责用户、项目、内容源、Brief、对话、权限、状态流和预览入口。
- 执行面负责每个作品独立 agent sandbox 的创建、恢复、工具调用、代码生成、构建、预览、错误修复和导出。
- Kubernetes 是系统状态编排层；`kubernetes-sigs/agent-sandbox` 提供长生命周期、单例、有状态、可暂停恢复的 agent 工作区。
- 每个作品对应一个长期存在的 sandbox，而不是每次请求创建一次临时 job。这样才能支撑“生成后继续对话修改”的产品体验。

核心判断：

> 作品是产品对象，Sandbox 是作品的执行身体，Brief 是生成契约，对话是持续修改入口。

---

## 2. 设计目标

### 2.1 产品目标

- 内部设计师可以通过 prompt、Markdown、附件生成 Website 或 Docs。
- LLM 先整理内容为 Brief，设计师确认后再进入生成。
- 每个作品有独立 agent sandbox，可预览、可对话修改、可导出。
- 数据、附件、代码、预览、构建过程都在公司内部受控环境处理。

### 2.2 技术目标

- 支持 Next.js、Astro、Fumadocs、Docusaurus 四类技术模板。
- 支持长生命周期 agent workspace：持续修改、构建、保存依赖和中间文件。
- 支持隔离执行不可信或半可信代码，避免污染平台控制面。
- 支持可观测、可恢复、可暂停、可清理的生成任务。
- 支持未来扩展 Figma MCP、设计资产库、多用户协作和发布流程。

---

## 3. 参考基础：agent-sandbox 能力映射

`agent-sandbox` 的核心资源是 `Sandbox` CRD，用于管理单个有状态 pod，提供稳定身份、持久存储和生命周期管理。扩展资源包括 `SandboxTemplate`、`SandboxClaim` 和 `SandboxWarmPool`，适合把常用运行环境模板化，并通过预热池降低交互式 agent 的启动延迟。

开工版本假设：

- 使用 `agents.x-k8s.io/v1beta1` 和 `extensions.agents.x-k8s.io/v1beta1`。
- `SandboxClaim` 通过 `spec.warmpoolRef` 指向 `SandboxWarmPool`。
- 即使需要冷启动，也通过 `replicas: 0` 的 warm pool 作为统一 claim 入口。
- 如果选定的 agent-sandbox release 与上述 API 不一致，必须先更新 `docs/product/2026-07-04-anydesign-mvp/2026-07-04-mvp-implementation-plan.md` 的 U5，再开始 sandbox adapter 实现。

对本产品的映射如下：

| agent-sandbox 能力 | 本产品用途 |
|---|---|
| `Sandbox` | 一个作品的独立 agent 工作区 |
| `SandboxTemplate` | Next.js / Astro / Fumadocs / Docusaurus 运行环境模板 |
| `SandboxClaim` | 为某个作品领取或绑定一个可用 sandbox |
| `SandboxWarmPool` | 预热常用模板，降低首次生成等待时间 |
| Persistent storage | 保存作品源码、依赖、生成中间物、上下文文件 |
| Stable identity | 稳定访问 preview server、agent runtime、workspace service |
| Pause / resume / TTL | 空闲作品暂停，长期不用自动清理 |
| gVisor / Kata runtime | 隔离 LLM 生成代码和构建过程 |

---

## 4. 总体架构

```text
┌────────────────────────────────────────────────────────────────────┐
│                           User / Designer                           │
└───────────────────────────────┬────────────────────────────────────┘
                                │
                                ▼
┌────────────────────────────────────────────────────────────────────┐
│                         Next.js Web App                             │
│  - Create Project                                                   │
│  - Brief Review / Edit                                              │
│  - Project Detail: Chat left + Preview right                        │
│  - Export / Handoff                                                 │
└───────────────────────────────┬────────────────────────────────────┘
                                │
                                ▼
┌────────────────────────────────────────────────────────────────────┐
│                    Product API / BFF / Realtime                     │
│  - AuthN / AuthZ                                                    │
│  - Project metadata                                                 │
│  - Attachment upload                                                │
│  - Conversation and event stream                                    │
│  - Preview routing                                                  │
│  - Project version alignment                                        │
└───────────────┬───────────────────────────────┬────────────────────┘
                │                               │
                ▼                               ▼
┌──────────────────────────────┐      ┌──────────────────────────────┐
│       Brief Orchestrator      │      │      Agent Orchestrator      │
│  - Parse content sources      │      │  - Create/claim sandbox      │
│  - Generate structured Brief  │      │  - Run generation loop       │
│  - Revise Brief via chat      │      │  - Track progress/errors     │
└───────────────┬──────────────┘      └───────────────┬──────────────┘
                │                                      │
                ▼                                      ▼
┌──────────────────────────────┐      ┌──────────────────────────────┐
│         Internal LLM          │      │        Kubernetes API         │
│  - Brief model calls          │      │  - Platform CRDs              │
│  - Agent model calls          │      │  - agent-sandbox CRDs         │
└──────────────────────────────┘      └───────────────┬──────────────┘
                                                       │
                                                       ▼
┌────────────────────────────────────────────────────────────────────┐
│                 agent-sandbox Execution Plane                       │
│  SandboxTemplate / SandboxWarmPool / SandboxClaim / Sandbox         │
│                                                                    │
│  ┌──────────────────────────────────────────────────────────────┐  │
│  │ Per-project Agent Sandbox                                    │  │
│  │ - Agent runtime                                               │  │
│  │ - Node/package manager/toolchains                             │  │
│  │ - Source workspace                                            │  │
│  │ - Preview server                                              │  │
│  │ - Build/test/repair loop                                      │  │
│  │ - Persistent volume                                           │  │
│  └──────────────────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────────────────┘
```

---

## 5. 核心领域对象

### 5.1 Product Control Plane 对象

这些对象建议由应用数据库保存，用于产品体验、查询和权限控制。

- `User`：内部用户身份。
- `Project`：作品项目，承载 Website / Docs 的长期生命周期。
- `ContentSource`：prompt、Markdown、附件、未来 Figma 引用。
- `Brief`：生成前确认的结构化作品意图。
- `Conversation`：围绕 Brief 或作品的对话历史。
- `GenerationRun`：一次生成或修改任务。
- `Artifact`：源码包、预览构建结果、导出文件。
- `SandboxBinding`：Project 与底层 `SandboxClaim` / `Sandbox` / workspace PVC 的绑定关系；`sandbox + PVC` 共同定义一个作品的可写 workspace 范围。

### 5.2 Kubernetes Control Plane 对象

这些对象建议由平台控制器或 orchestrator 管理。

- `SandboxTemplate`：按技术模板定义基础运行环境。
- `SandboxWarmPool`：按模板维护预热池。
- `SandboxClaim`：为作品申请 sandbox。
- `Sandbox`：agent-sandbox controller 生成或管理的实际有状态执行单元。
- `NetworkPolicy`：限制 sandbox 网络出口和入口。
- `ResourceQuota` / `LimitRange`：约束 namespace 或项目资源。
- `Secret` / 内部凭证代理：为 sandbox 提供最小可用访问能力。

### 5.3 是否需要自定义业务 CRD

MVP 可以先不急着把所有业务对象做成 CRD。建议分两阶段：

**阶段 1：数据库为主，agent-sandbox CRD 为执行资源**

- App DB 保存 Project、Brief、Conversation、GenerationRun。
- Orchestrator 调 Kubernetes API 创建 `SandboxClaim`。
- `SandboxBinding` 记录业务项目与 K8s 资源关系。
- 优点：实现快，产品迭代灵活。

**阶段 2：引入平台业务 CRD**

- 当需要 GitOps、跨服务编排、审计、平台自愈时，引入 `DesignProject`、`DesignRun` 这类业务 CRD。
- App DB 仍保存查询友好的产品视图，K8s CRD 成为执行状态源。
- 优点：更 Kubernetes-native，但控制器复杂度更高。

建议 MVP 采用阶段 1。不要在产品形状还没稳定时过早设计一套完整业务 CRD。

---

## 6. 关键链路设计

### 6.1 创建项目与 Brief 链路

```text
用户输入 prompt / Markdown / 附件
  -> Product API 创建 Project + ContentSource
  -> Brief Orchestrator 读取内容源
  -> 调用内部 LLM 生成 Brief
  -> Brief 保存为 draft
  -> 前端展示 Brief
  -> 用户继续对话修改或确认
```

关键原则：

- Brief 阶段不创建长期 sandbox。附件解析（文本提取）在控制面处理，不需要 sandbox 隔离；只有当附件需要执行代码（如渲染 PDF、解析复杂格式）时，才考虑隔离执行，但这属于后续阶段能力，MVP 以纯文本提取为准。
- Brief 是生成契约，需要版本化。
- 用户确认后的 Brief 应冻结为一次 `GenerationRun` 的输入，后续修改另开新版本。

### 6.2 首次生成链路

```text
用户确认 Brief
  -> 选择 Website / Docs 与技术模板
  -> Agent Orchestrator 创建 GenerationRun
  -> 通过 SandboxClaim 获取 sandbox
  -> 注入 Brief、design.md、附件索引和系统提示词
  -> Agent 在 sandbox 内生成项目源码
  -> 安装依赖 / 构建 / 启动 preview
  -> 自动修复可恢复错误
  -> 回传状态、日志摘要、预览地址
```

关键原则：

- 生成代理在 sandbox 内拥有文件、shell、包管理、构建、浏览器检查等工具。
- 控制面只下发目标和上下文，不把所有生成步骤写死成后端 workflow。
- 代理必须显式上报 `completed`、`failed`、`needs_user_input`、`blocked` 等终态，避免靠“没有新日志”猜测完成。

### 6.3 对话式修改链路

```text
用户在作品页提出修改
  -> Conversation 追加消息
  -> Agent Orchestrator 恢复或连接该作品 sandbox
  -> 注入当前项目上下文、最近对话、Brief、design.md
  -> Agent 修改文件并重新构建
  -> 左侧 chat 追加 agent 消息、工具摘要和进度事件
  -> 右侧 preview 在新版本可用后刷新
```

关键原则：

- 修改发生在同一个 workspace，不重新生成新项目。
- 每次修改建议形成一次 checkpoint：源码快照、Brief 版本、对话消息、构建结果。
- 如果修改会大幅改变作品方向，系统应提示先更新 Brief。
- 作品详情页是生成后的主工作台：左侧承载 LLM chat message 信息和事件流，右侧承载当前作品预览。
- Chat 消息、运行状态和预览版本必须绑定同一个 `GenerationRun` 或 checkpoint，避免左侧显示已完成但右侧仍是旧版本。

---

## 6.4 作品详情工作台

生成完成后，用户不应进入单独的“结果页”，而应进入可持续修改的作品详情工作台。

```text
┌────────────────────────────────────────────────────────────────────┐
│ Project Detail                                                     │
├──────────────────────────────┬─────────────────────────────────────┤
│ Left: LLM Chat / Events       │ Right: Live Preview                 │
│ - User messages               │ - Current Website / Docs preview    │
│ - Assistant replies           │ - Loading / rebuilding state        │
│ - Tool call summaries         │ - Version badge                     │
│ - Build / repair progress     │ - Open full preview                 │
│ - Confirmation prompts        │ - Export entry                      │
└──────────────────────────────┴─────────────────────────────────────┘
```

左侧消息区职责：

- 接收用户对作品的修改要求。
- 展示 LLM 对修改意图的回应。
- 展示非工程化的工具调用摘要，例如“正在更新页面结构”“正在重新构建预览”。
- 在需要时请求用户确认，例如方向性大改、Brief 冲突或安全策略拦截。
- 持久化用户消息、assistant 回复、重要工具摘要、审批请求、错误摘要和正式 preview 更新；高频底层事件只进入运行日志或折叠摘要。

右侧预览区职责：

- 展示当前 checkpoint 对应的可用预览。
- 在 build 或 repair 中显示 rebuilding 状态，而不是立即清空旧预览。
- 新版本先作为 candidate preview 进入检查和修复流程，通过 Build/Review/Safety gate 后再原子切换为正式预览。
- 提供全屏预览、刷新、导出入口。

---

## 7. Sandbox 模板设计

建议为 MVP 定义四类模板，但不要求第一天全部达到同等质量。

| 模板 | 适用 | 基础能力 |
|---|---|---|
| `nextjs-website` | 官网、Landing Page、复杂交互 | Node、pnpm、Next.js、React、Tailwind、构建检查、preview server |
| `astro-website` | 静态站、内容站、作品页 | Node、pnpm、Astro、MD/MDX、静态构建、preview server |
| `fumadocs-docs` | 现代产品/开发者文档 | Node、pnpm、Next.js、Fumadocs、MDX、搜索/导航生成 |
| `docusaurus-docs` | 成熟团队文档、版本化文档 | Node、pnpm、Docusaurus、Markdown、sidebar 生成 |

模板应包含：

- 固定基础镜像和工具链版本。
- 预装依赖或可缓存依赖。
- 标准 workspace 目录。
- agent runtime 或工具 server。
- preview server 端口约定。
- 构建、lint、页面检查脚本。
- `context.md` / `brief.md` / `design.md` 的约定路径。

`SandboxWarmPool` 应优先为最高频模板预热。MVP 初期建议按以下顺序支持：

1. `astro-website`
2. `fumadocs-docs`
3. `nextjs-website`
4. `docusaurus-docs`

理由：Astro 和 Fumadocs 更贴合 Markdown-first，冷启动和生成复杂度相对可控。开工范围锁定为先实现 `astro-website` 端到端闭环，再实现 `fumadocs-docs`；Next.js 与 Docusaurus 在 MVP 中保留模板接口和产品模型，不作为第一阶段验收项。

---

## 8. Workspace 文件约定

每个 sandbox 内建议采用稳定目录结构，让 agent、构建工具和平台都能理解同一份工作区。

```text
/workspace
  /project                 # 生成后的站点源码
  /inputs
    prompt.md
    brief.md
    design.md
    content-sources.json
    attachments/
  /outputs
    build/
    export/
    screenshots/
    reports/
  /state
    context.md
    project.json
    run-log.jsonl
    tasks.json
    preview.json
    checkpoints/
```

文件原则：

- `brief.md` 是一次生成的契约。
- `design.md` 是风格和设计规则上下文。
- `context.md` 记录 agent 对该作品的长期理解和历史决策。
- `project.json` 记录 runtime 锁定的 `appRoot`、模板、框架、包管理器、模板版本和 lockfile 策略；由 harness 写入，agent 只通过受控工具读取或更新。
- `tasks.json` 记录本次 run 的任务拆分和进度，由 Build/Edit/Repair agent 在 run 开始时写入并持续更新。
- `preview.json` 记录 preview server 状态、实际 cwd、端口、URL、candidate version 和截图路径，供 harness 和 Review agent 读取。
- `run-log.jsonl` 记录机器可读任务事件。
- `project/` 是用户最终可导出的源码。
- Build/Edit 阶段的源码写入应以 `project.json.appRoot` 为准，禁止静默创建 nested package root。

**SandboxTemplate 初始化要求：** 模板启动脚本必须预创建 `/workspace/inputs`、`/workspace/outputs`（含子目录）、`/workspace/state` 目录，并写入空的 `tasks.json`（`[]`）、`preview.json`（`{}`）和可由 runtime 覆盖的 `project.json`（`{}`），避免 agent 首次写入时遇到目录不存在错误。

---

## 9. Agent 工具边界

### 9.1 推荐工具分层

**基础工具**

- 读写文件
- 列目录
- 运行 shell 命令
- 安装依赖
- 启动/停止 preview server
- 读取构建日志
- 截图和页面检查
- 完成任务信号

**平台工具**

- 读取 Brief
- 读取 design.md
- 读取附件索引
- 上报进度
- 写入预览元数据
- 生成导出包
- 请求用户补充信息

**未来工具**

- 读取 Figma MCP 上下文
- 查询内部设计资产库
- 查询组件库文档
- 发布到内部站点托管平台

### 9.2 Agent-native 原则

- 工具应是原子能力，不要把“生成完整网站”做成一个黑盒工具。
- Agent 负责判断如何组织内容、修改文件、运行构建和修复错误。
- UI 能做的动作，agent 也应通过工具做到：修改内容、切换模板、查看预览、导出、回滚。
- 任务必须有明确完成信号，不依赖日志静默或进程退出猜测完成。

---

## 10. 安全架构

### 10.1 隔离层级

```text
用户 / 项目权限
  -> Product API 权限校验
  -> Project-scoped SandboxBinding
  -> Kubernetes namespace / labels
  -> Sandbox pod security context
  -> gVisor 或 Kata runtime
  -> NetworkPolicy 限制
  -> 最小权限 Secret / 凭证代理
```

### 10.2 网络策略

默认策略：

- sandbox 不能访问控制面数据库。
- sandbox 不能访问 Kubernetes API，除非通过受限 service account。
- sandbox 只能访问内部 LLM 网关、包缓存、素材存储、必要的内部文档源。
- preview 只允许通过平台 preview router 暴露给有权限用户。
- 默认禁止访问公网；如需要下载 npm 依赖，应优先走内部 registry/proxy。

### 10.3 凭证策略

- 不把长期密钥直接写入 sandbox 文件系统。
- sandbox 访问内部服务应通过短期 token 或凭证代理。
- 用户上传附件与生成代码分级存储，避免 agent 意外读取跨项目内容。
- 所有工具调用、外部访问、导出行为都应可审计。

### 10.4 运行时隔离

MVP 建议优先支持 gVisor；如果内部资料安全等级更高或需要更强隔离，再评估 Kata。

- gVisor：启动成本相对低，适合大多数代码生成和构建场景。
- Kata：隔离更强，但资源成本和运维复杂度更高，适合高敏场景。

---

## 11. 状态机

### 11.1 Project 状态

```text
draft
  -> briefing
  -> brief_ready
  -> brief_confirmed
  -> sandbox_provisioning
  -> generating
  -> preview_ready
  -> editing
  -> exporting
  -> completed
```

异常状态：

```text
needs_user_input
blocked
failed_recoverable
failed_terminal
paused
archived
```

### 11.2 Sandbox 生命周期

```text
unclaimed
  -> claimed
  -> starting
  -> ready
  -> busy
  -> idle
  -> paused
  -> resumed
  -> deleting
  -> deleted
```

Product Project 与 Sandbox 不应完全同生命周期：

- Project 可以长期存在。
- Sandbox 可以暂停、恢复、重建。
- 源码和关键状态必须通过持久卷、对象存储或导出包保存。

---

## 12. 存储与制品

### 12.1 推荐存储分层

- App DB：项目元数据、Brief、对话、运行状态、权限。
- Object Storage：原始附件、导出包、截图、构建产物。
- Sandbox PVC：源码、依赖缓存、运行中间状态。
- Git Repo 或内部代码存储：可选，用于版本化导出和长期维护。

### 12.2 Checkpoint 策略

每次重要动作生成 checkpoint：

- Brief 确认
- 首次生成成功
- 每次对话修改成功
- 导出前
- 自动修复后

checkpoint 至少包含：

- 当前 Brief 版本
- 当前 design.md 版本
- 源码快照或 commit id
- 对话消息范围
- 构建结果
- 预览地址或截图

---

## 13. 预览与导出

### 13.1 Preview Router

建议控制面提供统一 Preview Router，对外暴露两类 URL：

```text
# 稳定的"当前版本"预览 URL（跟随 promoted 版本变化，设计师收藏和分享用）
/preview/{projectId}/current
  -> AuthZ
  -> 查找 project.current_version_id 对应的 promoted preview
  -> SandboxBinding lookup
  -> Route to sandbox preview server

# 精确版本预览 URL（绑定具体 versionId，用于调试、审计和回溯）
/preview/{projectId}/{versionId}
  -> AuthZ
  -> SandboxBinding lookup
  -> Route to sandbox preview server
```

URL 稳定性约定：

- `/current` URL 在 run 期间指向上一个 promoted 版本（保持稳定），`preview.updated` 到达后切换到新版本。
- `/{versionId}` URL 永久有效，但如果对应 sandbox 已暂停，Router 需负责唤醒或返回 screenshot 降级。
- **不使用 `runId` 作为 URL 组成部分**，避免每次 edit run 导致设计师的预览链接失效。

Preview Router 负责：

- 权限校验。
- 连接恢复或唤醒 sandbox。
- 隐藏 pod/service 细节。
- 统一注入安全 header。
- 记录访问审计。

Service 创建策略：agent-sandbox 对 Service 创建采用 opt-in 模式。MVP 阶段 Preview Router 优先通过 pod IP/内部路由能力访问 sandbox preview server，不为每个 sandbox 默认创建 Service。只有在内部路由不可行时，才为对应 sandbox 显式启用 Service。此策略需要在 U5 sandbox adapter 实现前与基础设施团队确认。

### 13.2 导出

MVP 导出建议支持：

- 源码 zip。
- 构建产物。
- 生成报告，包括模板、Brief、design.md、主要修改记录。

部署上线可以后置，但架构上应保留发布到内部托管平台的出口。

---

## 14. 可观测性

### 14.1 用户可见事件

- 正在整理内容
- Brief 已生成
- 正在准备工作区
- 正在生成源码
- 正在安装依赖
- 正在构建预览
- 正在自动修复
- 预览已就绪
- 需要用户补充信息
- 生成失败，可重试或查看摘要

### 14.2 平台可观测指标

- Sandbox 申请耗时
- WarmPool 命中率
- 生成任务耗时
- 构建成功率
- 自动修复成功率
- token 使用量
- 模板维度成功率
- sandbox CPU / memory / storage 使用
- preview 访问量
- 失败原因分类

### 14.3 日志分层

- 用户日志：友好的步骤和摘要。
- Agent 日志：工具调用、模型响应摘要、完成信号。
- Sandbox 日志：进程、构建、preview server。
- 平台日志：API、权限、K8s 资源变更。
- 审计日志：附件访问、导出、敏感操作、外部网络请求。

---

## 15. MVP 技术分期

### Phase 0：基础验证

- Next.js 控制台骨架。
- Project / ContentSource / Brief / Conversation / GenerationRun 数据模型。
- 手动或最小 orchestrator 创建 agent-sandbox `SandboxClaim`（此阶段 sandbox 接入可以是 mock 或本地进程替代，但 SandboxBinding 数据结构必须按真实 API 定义，不允许用简化字段，否则 Phase 1 集成时需要重写数据层）。
- 一个模板优先跑通，建议 `astro-website`。
- 支持 Markdown -> Brief -> 确认 -> 生成 -> 预览。

**Phase 0 与 Phase 1 边界约定：** Phase 0 中所有对外部系统（K8s、sandbox channel、内部模型网关、package registry）的调用都可以用 mock/fake 替代，但接口契约（函数签名、API schema、事件结构）必须与真实接口一致。Phase 1 的工作量是"替换实现"而不是"重写接口"。

### Phase 1：MVP 闭环

- Website / Docs 两类作品。
- Astro + Fumadocs 两个高优先级模板。
- 每作品独立 sandbox。
- 对话式修改。
- 自动构建与可恢复错误修复。
- Preview Router。
- 附件上传和内部对象存储。
- 基础审计和资源清理。

### Phase 2：模板扩展与治理

- Next.js 与 Docusaurus 模板。
- `SandboxWarmPool` 按模板预热。
- design.md 管理与校验。
- checkpoint / rollback。
- 更完整的 NetworkPolicy、ResourceQuota、运行时隔离策略。
- 管理员模板配置页。

### Phase 3：高级能力

- Figma MCP 作为视觉上下文来源。
- 内部设计资产库。
- 发布到内部托管平台。
- 多人协作、评论和审批。
- 更精细的权限、审计报表和成本治理。

---

## 16. 风险与建议

### 16.1 主要风险

- 四个模板同时做深会拖慢 MVP，建议先 Astro + Fumadocs。
- 直接让 agent 生成而不经过 Brief，会导致生成质量不可控。
- 如果 sandbox 可以访问过多内部网络，会削弱“安全生成”的核心价值。
- 如果 preview 路由直接暴露 pod/service，会造成权限和运维复杂度。
- 如果没有明确完成信号和 checkpoint，长任务恢复会很脆。

### 16.2 架构建议

- 控制面保持产品语义，执行面保持 Kubernetes 语义，不要混在一个服务里。
- 业务数据库先承载产品对象，K8s CRD 先承载执行对象。
- 每个作品一个长期 sandbox，每次生成/修改一个 `GenerationRun`。
- Agent 工具保持原子，复杂行为靠系统提示词和上下文组织。
- 默认网络收紧，按模板和任务逐步放行。
- 优先建设 Preview Router 和事件流，因为它们直接决定用户是否觉得“自然顺畅”。

---

## 17. 推荐 MVP 架构落点

第一版最小可落地组合：

```text
Next.js App
  + Product API
  + App DB
  + Object Storage
  + Brief Orchestrator
  + Agent Orchestrator
  + Kubernetes cluster
  + agent-sandbox controller/extensions
  + Astro SandboxTemplate
  + Fumadocs SandboxTemplate
  + Preview Router
```

第一条端到端链路：

```text
Markdown + prompt
  -> Brief
  -> 用户确认
  -> SandboxClaim from astro-website WarmPool
  -> Agent 生成 Astro website
  -> preview
  -> 用户对话修改
  -> rebuild preview
  -> export zip
```

这条链路能验证产品最关键的三个假设：

- 设计师是否接受 Brief-first 的生成方式。
- agent sandbox 是否能稳定支撑对话式作品修改。
- Markdown-first 是否足以生成高质量 Website / Docs。

开工计划以 `docs/product/2026-07-04-anydesign-mvp/2026-07-04-mvp-implementation-plan.md` 为准。第一阶段开发不要同时展开四个模板、Figma MCP、发布流或完整治理台。

---

## 18. 参考资料

- [kubernetes-sigs/agent-sandbox README](https://github.com/kubernetes-sigs/agent-sandbox)
- [Agent Sandbox Documentation](https://agent-sandbox.sigs.k8s.io/docs/)
- [Running Agents on Kubernetes with Agent Sandbox](https://kubernetes.io/blog/2026/03/20/running-agents-on-kubernetes-with-agent-sandbox/)
- [Agent Sandbox Configuration](https://github.com/kubernetes-sigs/agent-sandbox/blob/main/docs/configuration.md)
- [Agent Sandbox Releases](https://github.com/kubernetes-sigs/agent-sandbox/releases)
