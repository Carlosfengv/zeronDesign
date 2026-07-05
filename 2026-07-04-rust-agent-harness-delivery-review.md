---
date: 2026-07-04
topic: rust-agent-harness-delivery-review
status: draft
source_harness: ./2026-07-04-agent-harness-design.md
source_architecture: ./2026-07-04-internal-ai-website-docs-generator-architecture.md
---

# Rust Agent Harness 交付评审

## 1. 评审结论

从资深 harness 工程交付角度看，当前方案 **可行**，但必须把它当成一个后端 agent runtime 项目交付，而不是一个 Next.js 应用里附带的 LLM 调用功能。

推荐路线是：

- 前端只负责创建、Brief 确认、左侧 chat、右侧 preview、审批和导出入口。
- 后端控制面负责 AgentSession、AgentRun、事件流、权限、版本和 sandbox 调度。
- Rust runtime 负责 agent loop、model streaming、tool execution、permission enforcement、MCP adapter、checkpoint 和 run completion。
- Kubernetes / agent-sandbox 负责每个作品的长期隔离执行环境。

参考成熟开源 agent runtime（如 OpenCode、Claude Code 等）的工程形态，理解 Tool 抽象、agent loop、权限引擎、StreamingToolExecutor、subagent 的组织方式，但不建议直接复制或绑定任何外部实现作为运行时。本产品应自建 Rust harness，以满足内部安全要求和长生命周期作品的特殊需求。

最终判断：

> 这个产品可以自建 Rust harness。MVP 不需要复刻完整 Claude Code，但必须先做出最小可信 agent loop、工具抽象、权限引擎、sandbox adapter 和 preview promotion。

开发开工口径：

- 可以立即开工 `docs/product/2026-07-04-anydesign-mvp/2026-07-04-mvp-implementation-plan.md` 的 U1-U3。
- U4-U6 开工前必须确认 agent-sandbox release、开发 K8s 集群、内部模型网关、对象存储、内部 package registry/proxy 和 preview routing。
- Phase A 第一条端到端链路只验收 `astro-website`。
- `fumadocs-docs` 是 Phase A.5 第二条模板闭环，不阻塞第一轮 runtime API freeze。
- Next.js、Docusaurus、Figma MCP、外部发布和完整治理台不进入第一阶段开发。

---

## 2. 真实代码参考带来的关键修正

### 2.1 不是“LLM + 几个后端 API”

`claude-code-main` 的核心不是几个工具函数，而是一套完整 runtime：

- `Tool` 类型包含 schema、call、permission、并发安全、只读/破坏性判断、MCP 信息、延迟加载等字段。
- `QueryEngine` 是每个会话的生命周期对象。
- `query()` 是 agent loop，负责模型流、tool use、compact、budget、stop hook 和 tool result。
- `StreamingToolExecutor` 负责边流式接收 tool call，边执行并发安全工具，同时保持结果顺序。
- `AgentTool` / `runAgent` 负责子 agent、独立权限、独立 MCP、独立 transcript。

这说明我们的 harness 不能只写成：

```text
POST /generate
  -> call LLM
  -> run shell
  -> return preview
```

否则后续很快会卡在权限、恢复、并发工具、长任务、子 agent、预览版本对齐、审计这些问题上。

### 2.2 Tool trait 是 Rust runtime 的第一优先级

真实代码里的 `Tool` 抽象证明，工具不是普通函数，而是可被 agent loop 调度、可被权限系统判断、可被 UI 展示、可被事件系统记录的一等对象。

Rust 中建议抽象为：

```rust
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn input_schema(&self) -> serde_json::Value;
    fn output_schema(&self) -> Option<serde_json::Value>;

    fn is_read_only(&self, input: &serde_json::Value) -> bool;
    fn is_concurrency_safe(&self, input: &serde_json::Value) -> bool;
    fn is_destructive(&self, input: &serde_json::Value) -> bool;

    async fn validate_input(&self, input: serde_json::Value, ctx: &ToolContext)
        -> anyhow::Result<serde_json::Value>;

    async fn check_permission(&self, input: &serde_json::Value, ctx: &ToolContext)
        -> PermissionDecision;

    async fn call(&self, input: serde_json::Value, ctx: ToolContext, progress: ProgressSink)
        -> anyhow::Result<ToolResult>;
}
```

MVP 里可以先不做所有扩展字段，但 `name/schema/call/permission/read_only/concurrency_safe/destructive` 这几个不能省。

### 2.3 权限引擎必须独立于 prompt

真实代码中的权限判断不是 system prompt 文案，而是工具执行前的强制决策链。我们的内部安全目标更强，所以必须更保守：

```text
organization deny
  -> project deny
  -> agent profile deny
  -> run scoped deny
  -> tool-specific permission
  -> run scoped allow/ask
  -> agent profile allow/ask
  -> platform default
```

决策结果只允许三类：

```text
allow
ask
deny
```

默认策略：

- 未声明的写文件、shell、网络、secret、外部目录访问一律 deny。
- `ask` 不一定弹给设计师，可以转成平台策略审批、管理员审批或运行前配置。
- `deny` 永远优先。
- 权限决策必须写入 audit log。

用户体验策略：

- 设计师只处理业务确认：Brief、作品类型、模板、方向性大改、导出或 handoff。
- 平台处理安全确认：未知依赖、网络例外、长时间 sandbox 占用、敏感导出。
- 管理员处理高风险确认：公网访问、外部发布、跨项目资产、高敏凭证、destructive command。
- 常规生成路径必须通过模板白名单自动放行，不应出现频繁底层权限弹窗。

### 2.4 子 agent 是运行图，不是普通函数调用

Build、Review、Repair、Edit 不应该只是后端函数，而应该是有父子关系的 `AgentRun`：

```text
build run
  -> visual review child run
  -> repair child run
  -> final review child run
  -> complete
```

每个 child run 应有：

- `parentRunId`
- `triggeredByEventId`
- `baseVersionId`
- `outputVersionId`
- `agentProfile`
- 独立工具集
- 独立权限模式
- 独立 transcript / event stream

这样作品详情页才能解释清楚“为什么自动修了、修了什么、现在右侧预览对应哪个版本”。

---

## 3. 交付风险评审

### P0 风险：把生成流程写死成后端 workflow

风险表现：

- 后端写 `generateWebsite()`、`fixBuildErrors()`、`applyDesignMd()`。
- agent 只是填参数和调用固定步骤。
- 每加一个生成场景都要改代码。

交付建议：

- 后端只提供原子工具和 run 状态机。
- “生成 website/docs” 是 agent profile + prompt + tool composition 的结果。
- 只允许少量平台编排逻辑：claim sandbox、启动 run、接收完成信号、preview promotion。

### P0 风险：没有显式完成信号

风险表现：

- 靠进程退出、没有新日志、预览端口启动来判断完成。
- 前端左侧显示完成，右侧其实还是旧版本。

交付建议：

- 必须提供 `run.complete` 工具。
- 只有 agent 调用 `run.complete(status, summary, outputVersionId)` 才能进入终态。
- `preview.updated` 必须绑定 `GenerationRun` 或 checkpoint。

### P0 风险：工具执行无权限边界

风险表现：

- shell 直接透传。
- npm 可访问公网。
- sandbox 可读控制面 secret 或跨项目附件。

交付建议：

- tool executor 前必须调用 permission engine。
- shell command 做 prefix/subcommand policy。
- package install 默认走内部 registry/proxy。
- public internet 默认 deny。
- `/workspace` 外读写默认 deny。

### P0 风险：preview 直接等于正式结果

风险表现：

- build server 一启动就刷新右侧正式预览。
- 自动修复期间用户看到半成品。
- review 失败但 UI 显示成功。

交付建议：

```text
preview.rebuilding
  -> preview.candidate
  -> review / repair
  -> preview.updated
```

右侧正式预览只消费 `preview.updated`，不能直接消费候选预览。

### P1 风险：一开始支持四个模板

风险表现：

- Next.js、Astro、Fumadocs、Docusaurus 都只做半成。
- 每个模板的 install/build/preview/repair 差异都拉低稳定性。

交付建议：

- MVP 只深做两个模板：`astro-website` 和 `fumadocs-docs`。
- Next.js 和 Docusaurus 先保留模板接口，不作为首个验收闭环。

### P1 风险：只做事件流，不做持久化消息模型

风险表现：

- 刷新页面后左侧 chat 丢失。
- 工具日志和用户可见消息混在一起。
- 无法回放某个版本是怎么生成的。

交付建议：

- `AgentEvent` 用于实时。
- `ConversationItem` 用于长期页面展示。
- 原始工具日志进入 `run-log.jsonl` 或对象存储。
- 左侧只展示用户可理解的摘要。

---

## 4. 推荐 Rust Runtime 架构

```text
anydesign-runtime
├── runtime-core
│   ├── AgentSession
│   ├── AgentRun
│   ├── AgentLoop
│   ├── TurnState
│   └── Checkpoint
├── model-gateway
│   ├── ModelClient
│   ├── ModelStream
│   └── ToolUseBlock parser
├── tool-core
│   ├── Tool trait
│   ├── ToolRegistry
│   ├── ToolContext
│   └── ToolResult
├── tool-executor
│   ├── StreamingToolExecutor
│   ├── serial/concurrent batching
│   ├── cancellation
│   └── progress events
├── permission-engine
│   ├── policy resolver
│   ├── path policy
│   ├── command policy
│   ├── network policy
│   └── audit writer
├── sandbox-adapter
│   ├── Kubernetes client
│   ├── SandboxClaim binding
│   ├── workspace channel
│   └── preview channel
├── mcp-adapter
│   ├── MCP client
│   ├── MCP tool wrapper
│   └── Figma MCP profile
├── agent-profiles
│   ├── brief
│   ├── build
│   ├── review
│   ├── repair
│   ├── edit
│   └── export
└── event-store
    ├── AgentEvent
    ├── ConversationItem
    ├── RunLog
    └── Snapshot metadata
```

### 模块职责边界

| 模块 | 必须负责 | 不应负责 |
|---|---|---|
| `runtime-core` | run 生命周期、agent loop、完成信号 | 具体文件/命令实现 |
| `tool-core` | 工具接口、schema、上下文 | 权限策略细节 |
| `tool-executor` | 工具调度、并发、取消、进度 | 判断业务是否允许 |
| `permission-engine` | allow/ask/deny、审计、策略合并 | 执行工具 |
| `sandbox-adapter` | K8s/agent-sandbox 绑定、通道、状态 | agent 判断 |
| `mcp-adapter` | MCP 工具发现和包装 | 产品流程 |
| `event-store` | 实时事件和持久化消息 | UI 展示细节 |

---

## 5. MVP 交付切片

### Slice 1：最小 Agent Loop

目标：能跑一个没有 sandbox 的 Brief Agent。

交付内容：

- `AgentSession`
- `AgentRun`
- `ModelClient::stream`
- `ToolRegistry`
- `run.complete`
- `AgentEvent` SSE
- `ConversationItem` 持久化

验收：

- 用户输入 prompt / md 文本。
- Brief Agent 生成结构化 Brief。
- 用户可以要求修改 Brief。
- Agent 必须显式调用 `run.complete`。

### Slice 2：Sandbox Build 闭环

目标：Brief 确认后，在 sandbox 内生成一个 Astro website。

交付内容：

- `sandbox.claim`
- `/workspace` 协议
- `fs.read/list/search/write/patch`
- `shell.run`
- `package.install`
- `preview.start/status`
- `preview.candidate`
- `preview.updated`

验收：

- 生成 `/workspace/project`。
- 成功 install/build/start preview。
- 右侧显示正式预览。
- 左侧有用户可读进度，不展示原始日志刷屏。

### Slice 3：Permission Engine

目标：所有工具调用都经过强制权限。

交付内容：

- agent profile policy
- path policy
- command policy
- network policy
- audit log
- `permission.requested` 事件

验收：

- 访问 `/workspace` 外路径被拒绝。
- 读取 `.env` / token / kubeconfig 被拒绝。
- public internet 被拒绝。
- shell 高风险命令被拒绝或进入 ask。
- audit 能按 projectId/runId/tool 查询。

### Slice 4：Review / Repair 子运行图

目标：生成后自动检查并修复可恢复问题。

交付内容：

- `ReviewFinding`
- child `AgentRun`
- `browser.screenshot`
- `browser.inspect`
- `diagnostics.build_log`
- `repair` profile
- candidate preview promotion gate

验收：

- review 发现 blocking issue 时不 promotion。
- repair 成功后重新 build/preview/review。
- 左侧能解释自动修复摘要。
- 右侧正式预览只显示通过 gate 的版本。

### Slice 5：Fumadocs Docs 模板

目标：跑通 Markdown-first docs 场景。

交付内容：

- `fumadocs-docs` SandboxTemplate
- docs content mapping prompt
- sidebar/nav generation rules
- docs preview check

验收：

- Markdown 内容可生成可导航 docs。
- 文档结构先来自 Brief，而不是简单拼接原文。
- preview 中能检查至少首页、一个文档页、导航链接。

---

## 6. Tool Catalog MVP

### Control-plane tools

| Tool | 用途 | MVP |
|---|---|---:|
| `content.list_sources` | 列出内容源 | 必须 |
| `content.read_source` | 读取 prompt/md/附件解析文本 | 必须 |
| `brief.write_draft` | 写 Brief 草稿 | 必须 |
| `brief.read` | 读取 Brief | 必须 |
| `brief.update` | 修改 Brief | 必须 |
| `brief.request_confirmation` | 请求确认 | 必须 |
| `run.report_progress` | 用户可见进度 | 必须 |
| `run.complete` | 显式完成 | 必须 |
| `user.ask` | 请求补充信息 | 可后置 |

### Sandbox tools

| Tool | 用途 | MVP |
|---|---|---:|
| `fs.read` | 读文件 | 必须 |
| `fs.list` | 列目录 | 必须 |
| `fs.search` | 搜索 | 必须 |
| `fs.write` | 写文件 | 必须 |
| `fs.patch` | 精确修改 | 必须 |
| `shell.run` | 运行命令 | 必须 |
| `package.install` | 内部 registry 安装 | 必须 |
| `preview.start` | 启动预览 | 必须 |
| `preview.status` | 查询预览 | 必须 |
| `browser.screenshot` | 截图 | Slice 4 |
| `browser.inspect` | 控制台/DOM 检查 | Slice 4 |
| `diagnostics.build_log` | 构建日志摘要 | 必须 |
| `artifact.create_zip` | 导出源码包 | 可后置 |

### 暂缓工具

- Figma MCP：架构预留，MVP Markdown/docs-first 不进入首批验收。
- LSP 深度诊断：先用 build log 和 TypeScript CLI，后续接 LSP。
- ToolSearch / deferred tools：首版工具数量有限，不急。
- 多 agent team：先实现 parent-child run graph，不做复杂协作。

---

## 7. 数据模型交付要求

### AgentSession

```ts
type AgentSession = {
  id: string
  projectId: string
  sandboxId?: string
  status: "active" | "paused" | "archived"
  currentVersionId?: string
  createdAt: string
  updatedAt: string
}
```

### AgentRun

> 以下定义与 `agent-harness-design.md` Section 6.1 保持一致，为规范定义。`packages/shared/src/schemas.ts` 以此为准。

```ts
type AgentRun = {
  id: string
  projectId: string
  parentRunId?: string
  triggeredByEventId?: string     // 触发本 run 的事件 ID（如 review finding 触发 repair run）
  phase: "brief" | "build" | "repair" | "review" | "edit" | "export"
  agentProfile: string
  status:
    | "queued"
    | "running"
    | "needs_user_input"
    | "completed"
    | "partial"               // 超出修复上限或 maxIterations，保存 checkpoint 后返回
    | "blocked"               // 有明确阻塞原因，需要用户或管理员介入
    | "failed"
    | "cancelled"
  model: string
  sandboxId?: string
  briefVersion?: string
  designVersion?: string
  baseVersionId?: string
  outputVersionId?: string
  findingIds?: string[]           // review 发现的 finding ID 列表（repair run 使用）
  inputMessageIds: string[]
  checkpointId?: string
  startedAt: string
  updatedAt: string
  completedAt?: string
}
```

### AgentEvent

> 以下定义与 `agent-harness-design.md` Section 6.2 保持一致。

```ts
type AgentEvent =
  | { type: "run.started"; runId: string; label: string }
  | { type: "agent.message"; runId: string; text: string }
  | { type: "tool.started"; runId: string; tool: string; summary: string }
  | { type: "tool.completed"; runId: string; tool: string; summary: string; metadata?: unknown }
  | { type: "tool.failed"; runId: string; tool: string; error: string; recoverable: boolean }
  | { type: "permission.requested"; runId: string; tool: string; reason: string }
  | { type: "state.changed"; runId: string; state: string }
  | { type: "preview.rebuilding"; runId: string; previousVersionId?: string }
  | { type: "preview.candidate"; runId: string; url: string; versionId: string; screenshotId?: string }
  | { type: "preview.updated"; runId: string; url: string; versionId: string; screenshotId?: string }
  | { type: "artifact.created"; runId: string; artifactId: string; kind: string }
  | { type: "review.finding"; runId: string; findingId: string; severity: "info" | "warning" | "blocking"; summary: string }
  | { type: "run.completed"; runId: string; status: string; summary: string }
```

### ConversationItem

> 以下定义与 `agent-harness-design.md` Section 6.3 保持一致。

```ts
type ConversationItem = {
  id: string
  projectId: string
  runId?: string
  versionId?: string
  checkpointId?: string
  kind:
    | "user_message"
    | "assistant_message"
    | "tool_summary"
    | "progress"
    | "approval_request"
    | "preview_update"
    | "review_finding"
    | "error_summary"
  role?: "user" | "assistant" | "system"
  text: string
  metadata?: unknown
  createdAt: string
}
```

---

## 8. 关键接口建议

### Product API

```text
POST   /api/projects
POST   /api/projects/{projectId}/content-sources
POST   /api/projects/{projectId}/brief-runs
POST   /api/projects/{projectId}/brief/confirm
POST   /api/projects/{projectId}/generation-runs
POST   /api/projects/{projectId}/messages
GET    /api/projects/{projectId}/events
GET    /api/projects/{projectId}/conversation
GET    /api/projects/{projectId}/preview
POST   /api/runs/{runId}/cancel
POST   /api/runs/{runId}/permissions/{permissionId}/decision
```

### Runtime internal API

```text
StartRun(project_id, phase, agent_profile, context)
ContinueRun(session_id, user_message)
CancelRun(run_id)
StreamRunEvents(run_id)
ResolvePermission(permission_id, decision)
OpenSandboxChannel(project_id)
PromotePreview(project_id, run_id, candidate_version_id)  # in-process orchestrator; HTTP only behind test/admin feature flag
```

---

## 9. 验收标准

### 功能验收

- Prompt / Markdown 可以生成 Brief。
- Brief 可以被用户反复修改并确认。
- 确认后能在独立 sandbox 中生成 Astro website。
- 生成后左侧是 chat / progress / tool summary，右侧是 preview。
- 用户可以继续通过 chat 修改作品。
- 每次成功修改都有新 checkpoint。
- preview 版本和左侧完成消息一致。

### 安全验收

- sandbox 无法访问控制面数据库。
- sandbox 无法访问 Kubernetes API。
- sandbox 无法读取跨项目附件。
- sandbox 默认不能访问公网。
- shell 命令经过 command policy。
- secret-like 文件和内容被拒绝。
- 每次工具调用有 audit record。

### 工程验收

- AgentRun 有明确终态。
- 所有工具调用有 started/completed/failed event。
- tool failure 能区分 recoverable / terminal。
- runtime 重启后能恢复 run 或标记为 recoverable failed。
- 长输出不会直接塞满模型上下文，应写入文件并给摘要。
- preview candidate 不会直接覆盖正式 preview。

### 体验验收

- 设计师不需要理解 shell、npm、K8s。
- 左侧消息是产品语言，不是原始 terminal 日志。
- 失败时给可操作摘要：重试、修改 Brief、联系管理员、查看日志。
- 右侧预览在 rebuild 时保留旧版本并显示状态。

---

## 10. 不建议现在做的事

- 不要一开始做完整业务 CRD，先用 App DB 管 Project/Run/Conversation。
- 不要一开始支持四个模板同等深度。
- 不要把 Figma MCP 放进第一条闭环。
- 不要把 public internet 当作默认可用能力。
- 不要让前端直接驱动工具调用。
- 不要把 agent loop 写在 Next.js route handler 内。
- 不要用 “LLM 返回 JSON 后后端执行固定流程” 替代 harness。

---

## 11. 推荐里程碑

### M0：Runtime skeleton

**前置条件：** 内部模型网关接口可用（或有 mock 契约）。

- Rust service 启动。
- Model stream 打通。
- Tool trait + ToolRegistry。
- AgentRun / AgentEvent / run.complete。
- Brief Agent 跑通。

### M1：Sandbox generation

**前置条件：** 以下所有项必须在 M1 开工前确认，否则 sandbox 集成工作无法进行：
- agent-sandbox release 版本已锁定，CRD 清单已获取。
- 开发 Kubernetes 集群可用。
- 内部 package registry/proxy 可用（sandbox 内 pnpm install 的出口）。
- Preview routing 策略已确认（pod IP 直连或内部 DNS）。
- 对象存储 bucket/prefix 已分配（截图、构建产物）。

交付内容：
- Kubernetes sandbox claim + wait_ready 机制。
- Astro template。
- fs/shell/package/preview tools。
- 生成、构建、预览闭环。

### M2：Permission integration and audit

**前置条件：** Permission core 已完成单元测试，M1 sandbox 工具调用路径已跑通。

- path/command/network policy（含 exec array、realpath 检查）。
- audit log。
- permission requested/denied 事件。
- 真实 `fs.*`、`shell.run`、`package.install`、`preview.*`、`browser.*`、`diagnostics.*` 工具路径上的安全验收用例。

### M3：Review / Repair / Edit

- visual-review / repair child runs。
- candidate preview gate（`run.complete` 前置检查 promotion 状态）。
- chat edit。
- checkpoint。

### M4：Docs template（Phase A.5）

**前置条件：** Phase A Astro runtime loop 和安全验收通过，runtime API candidate 已通过 mock BFF contract tests。

- Fumadocs template。
- Markdown content structuring。
- docs preview check。

---

## 12. 最终建议

当前方案可以进入技术预研和 MVP 实施，但要锁定三个原则：

1. **Agent runtime 后端化**：LLM loop、tool execution、permission 和 sandbox 都在后端，不进前端。
2. **工具原子化**：不要做 workflow-shaped tools，保留 agent 的判断空间。
3. **版本可解释**：左侧消息、右侧 preview、run、checkpoint 必须绑定同一版本链。

只要这三个原则守住，Rust runtime + Kubernetes agent-sandbox 的路线是成立的，并且会比直接套用通用开源 agent runtime 更贴合内部安全、长生命周期作品、设计师可持续修改这三个核心目标。

当前文档状态已经达到开发启动标准：需求、架构、harness、交付评审和 implementation plan 已经闭环。下一步不应继续扩写大文档，而应进入 U1-U3 的工程实现，同时把 U4-U6 的外部依赖确认作为并行 spike。
