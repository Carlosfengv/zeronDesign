---
date: 2026-07-04
topic: agent-harness-design
status: draft
source_requirements: ./2026-07-04-internal-ai-website-docs-generator-requirements.md
source_architecture: ./2026-07-04-internal-ai-website-docs-generator-architecture.md
---

# Agent Harness 方案：从 Prompt 到确认再到生成

## 1. 结论

这个产品的 agent harness 应该采用 **Brief-first, permissioned multi-agent harness**：

- **Brief Agent** 在控制面运行，只整理内容和生成 Brief，不创建项目源码。
- **Build Agent** 在每个作品独立 sandbox 内运行，负责生成源码、安装依赖、构建、预览和修复。
- **Review Agent** 以只读方式检查生成质量、页面结构、构建结果和预览截图。
- **Edit Agent** 基于同一个 sandbox 对作品进行对话式修改。
- **Export Agent** 负责导出源码包、构建产物和生成报告。

借鉴 OpenCode 的方向是：agent 分角色、prompt 可配置、工具是原子能力、权限在 harness 层强制执行，而不是只靠 prompt 约束。OpenCode 有 Build/Plan 这类 primary agent，并通过 permissions 限制工具访问；本产品应把这个模式转译成 “Brief/Build/Review/Edit/Export” 的作品生成 harness。

---

## 2. Harness 的职责边界

### 2.1 Harness 负责什么

- 管理一次 agent run 的生命周期。
- 选择 agent profile、模型、工具集和权限策略。
- 注入项目上下文：Brief、design.md、内容源、模板信息、历史修改。
- 执行工具调用并做权限拦截。
- 将工具结果转换为事件流给前端。
- 保存 checkpoint、日志、制品和完成信号。
- 在 sandbox 不可用时创建、恢复或重连 sandbox。
- 在 agent 卡住、重复调用、越权或失败时中止并给出可恢复状态。

### 2.2 Harness 不负责什么

- 不把 “生成一个网站” 写成后端固定 workflow。
- 不替 agent 决定具体页面结构、组件布局、内容取舍。
- 不把工具设计成 `generate_website`、`fix_all_errors` 这种黑盒业务工具。
- 不把安全边界只写进 system prompt。
- 不让控制面直接执行 LLM 生成代码。

---

## 3. 总体结构

```text
┌──────────────────────────────────────────────────────────────┐
│                         Next.js UI                            │
│  Create / Brief Confirm / Detail Workspace / Export           │
│  Detail Workspace = Left Chat Messages + Right Preview        │
└───────────────────────────────┬──────────────────────────────┘
                                │
                                ▼
┌──────────────────────────────────────────────────────────────┐
│                      Agent Harness API                        │
│  - startRun(projectId, phase)                                 │
│  - continueRun(runId, userMessage)                            │
│  - approveBrief(projectId, briefVersion)                      │
│  - streamEvents(runId)                                        │
│  - cancelRun(runId)                                           │
└───────────────────────────────┬──────────────────────────────┘
                                │
                                ▼
┌──────────────────────────────────────────────────────────────┐
│                    Harness Orchestrator                       │
│  Agent Profile + Permissions + Context + Tool Router          │
└───────────────┬───────────────────────────────┬──────────────┘
                │                               │
                ▼                               ▼
┌──────────────────────────────┐      ┌──────────────────────────────┐
│     Control-plane Agents      │      │      Sandbox Agents          │
│  - Brief Agent                │      │  - Build Agent               │
│  - Brief Review Agent         │      │  - Edit Agent                │
│                               │      │  - Repair Agent              │
└──────────────────────────────┘      │  - Review Agent              │
                                      │  - Export Agent              │
                                      └───────────────┬──────────────┘
                                                      │
                                                      ▼
┌──────────────────────────────────────────────────────────────┐
│                Per-project agent-sandbox                      │
│  /workspace/project /workspace/inputs /workspace/state         │
│  files + shell + package manager + preview + browser checks    │
└──────────────────────────────────────────────────────────────┘
```

---

## 4. 从 Prompt 到生成的完整状态机

```text
project.created
  -> content.ingested
  -> brief.generating
  -> brief.ready
  -> brief.revising
  -> brief.confirmed
  -> sandbox.claiming
  -> sandbox.ready
  -> generation.planning
  -> generation.writing
  -> generation.installing
  -> generation.building
  -> generation.previewing
  -> generation.reviewing
  -> generation.repairing
  -> preview.ready
  -> edit.ready
  -> export.ready
```

异常状态：

```text
needs_user_input
permission_denied
tool_failed_recoverable
tool_failed_terminal
agent_blocked
sandbox_unavailable
cancelled
```

设计原则：

- Brief 确认前，不启动长期 Build Agent。
- Brief 确认后，创建或领取 sandbox。
- 所有生成和修改都绑定到 `GenerationRun`。
- Agent 必须调用显式完成工具结束 run，不靠日志静默判断。
- 每个状态变化都发事件给 UI。

---

## 5. Agent Profile 设计

### 5.1 Profile 表

| Agent | 运行位置 | 主要职责 | 写权限 | Shell 权限 | 用户确认 |
|---|---|---|---|---|---|
| `brief` | 控制面 | 整理 prompt/附件/md 为 Brief | 只能写 Brief 草稿 | 无 | 可直接运行 |
| `brief-review` | 控制面 | 检查 Brief 是否可生成 | 只读或建议修改 | 无 | 可直接运行 |
| `build` | sandbox | 生成源码、构建、预览 | `/workspace/project` | 允许安全命令 | Brief 确认后 |
| `repair` | sandbox | 读取错误并修复 | `/workspace/project` | 允许构建/测试命令 | 自动触发 |
| `visual-review` | sandbox | 只读评审预览质量 | 禁止 | 允许截图/检查 | 自动触发 |
| `edit` | sandbox | 对话式修改作品 | `/workspace/project` | 允许构建/测试命令 | 用户发起 |
| `export` | sandbox + 控制面 | 打包源码和产物 | `/workspace/outputs` | 允许构建/压缩 | 用户发起 |

### 5.2 OpenCode 借鉴点

OpenCode 把 agent 分成 primary agents 和 subagents，Build 默认拥有完整开发工具，Plan 是受限 agent，用于分析和建议但不直接改代码。这个产品应继承这种思路，但用户不需要感知 agent 名称，只感知流程阶段。

权限也应像 OpenCode 一样以工具名和路径/命令规则为核心，但更严格：

- Brief 阶段禁止改源码和运行 shell。
- Build/Edit 阶段只能在 sandbox 内写入。
- Review 阶段默认只读。
- Export 阶段只能读取项目并写导出目录。
- 高风险命令和外部网络访问必须由 harness 拦截。

---

## 6. Harness Run 数据模型

### 6.1 AgentRun

```ts
type AgentRun = {
  id: string
  projectId: string
  parentRunId?: string
  triggeredByEventId?: string
  phase: "brief" | "build" | "repair" | "review" | "edit" | "export"
  agentProfile: string
  status:
    | "queued"
    | "running"
    | "needs_user_input"
    | "completed"
    | "partial"
    | "blocked"
    | "failed"
    | "cancelled"
  model: string
  sandboxId?: string
  briefVersion?: string
  designVersion?: string
  baseVersionId?: string
  outputVersionId?: string
  findingIds?: string[]
  inputMessageIds: string[]
  checkpointId?: string
  startedAt: string
  updatedAt: string
  completedAt?: string
}
```

### 6.2 AgentEvent

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

这些事件直接驱动作品详情页：

- 左侧 chat message 区渲染 `agent.message`、`tool.*`、`permission.requested`、`state.changed` 和 `run.completed`。
- 右侧 preview 区渲染 `preview.rebuilding`、`preview.candidate` 与 `preview.updated`。
- `preview.candidate` 只表示 sandbox 内已有候选预览，不能立即替换右侧正式预览。
- `preview.updated.versionId` 必须与当前完成的 `GenerationRun` 或 checkpoint 对齐，避免消息已经完成但预览仍停留在旧版本。

### 6.3 ConversationItem

事件流负责实时体验，但作品详情页的左侧消息必须有可持久化的数据模型，支持刷新、断线重连和历史版本回看。

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

持久化规则：

- 用户输入、assistant 可见回复、审批请求、错误摘要、review finding 和正式 preview update 必须持久化。
- 高频低价值事件可以合并为 `tool_summary` 或 `progress`，避免左侧消息被日志刷屏。
- 原始工具日志仍保存在 run log 中，不直接等同于 chat message。

### 6.4 Checkpoint

```ts
type AgentCheckpoint = {
  id: string
  runId: string
  projectId: string
  phase: string
  messageWindow: unknown[]
  taskList: AgentTask[]
  workspaceSnapshot?: string
  briefVersion?: string
  designVersion?: string
  lastKnownPreview?: string
  contextSummary: string
  createdAt: string
}
```

---

## 7. Workspace 协议

每个作品 sandbox 内固定文件结构：

```text
/workspace
  /inputs
    prompt.md
    brief.md
    design.md
    content-sources.json
    attachments/
  /project
    package.json
    src/
    app/
    content/
    docs/
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

各文件职责：

- `/workspace/inputs/brief.md`：已确认的生成契约，Build/Edit 必读。
- `/workspace/inputs/design.md`：风格约束，可选但长期重要。
- `/workspace/state/context.md`：agent 对该作品的长期记忆，生成后持续更新。
- `/workspace/state/project.json`：runtime 锁定的 appRoot、模板版本、框架、包管理器和 lockfile 策略。
- `/workspace/state/tasks.json`：本次 run 的任务拆分和进度。
- `/workspace/state/preview.json`：preview server、实际 cwd、端口、URL、candidate version、截图状态。
- `/workspace/project`：唯一可导出的作品源码。

---

## 8. 工具设计原则

工具要像 OpenCode 的 built-in tools 一样提供原子能力：读文件、写文件、搜索、shell、LSP、MCP、技能加载、提问。区别是本产品要把工具权限和用户体验包装到平台内。

### 8.1 不要做的工具

避免这些 workflow-shaped tools：

- `generate_website`
- `generate_docs`
- `fix_all_build_errors`
- `make_it_beautiful`
- `apply_design_md`

这些工具会把 agent 降级成 workflow 调用器，质量和灵活性都会差。

### 8.2 应该做的工具

提供原子能力，让 agent 组合：

- 文件读写
- 目录搜索
- shell 命令
- 包管理
- preview server
- browser/screenshot
- LSP/TypeScript diagnostics
- artifact export
- progress reporting
- ask user
- complete task

---

## 9. Tool Catalog

### 9.1 Control-plane tools

| Tool | Agent | 权限 | 说明 |
|---|---|---|---|
| `content.list_sources` | brief | read | 列出本项目内容源 |
| `content.read_source` | brief | read | 读取 prompt、Markdown、附件解析文本 |
| `brief.write_draft` | brief | write-brief | 写入 Brief 草稿 |
| `brief.read` | all | read | 读取当前 Brief |
| `brief.update` | brief | write-brief | 根据用户反馈更新 Brief |
| `brief.request_confirmation` | brief | ask | 请求用户确认 Brief |
| `conversation.append_summary` | all | write-metadata | 写入对话摘要 |
| `run.report_progress` | all | write-event | 上报用户可见进度 |
| `run.complete` | all | complete | 显式结束 run |
| `user.ask` | brief/build/edit | ask | 请求用户补充信息 |

### 9.2 Sandbox filesystem tools

| Tool | Agent | 权限 | 说明 |
|---|---|---|---|
| `fs.read` | build/edit/review/repair/export | read | 读取 workspace 文件，支持范围读取 |
| `fs.list` | build/edit/review/repair/export | read | 列目录 |
| `fs.search` | build/edit/review/repair/export | read | 搜索内容 |
| `fs.write` | build/edit/repair/export | write | 创建或覆盖文件 |
| `fs.patch` | build/edit/repair | write | 精确修改文件 |
| `fs.delete` | build/edit/repair | write | 删除文件，默认限制在 project 内 |

### 9.3 Sandbox execution tools

| Tool | Agent | 权限 | 说明 |
|---|---|---|---|
| `shell.run` | build/edit/repair/export | shell | 执行命令，受 command policy 限制 |
| `package.install` | build/repair | package | 安装依赖，默认走内部 registry |
| `project.init` | build | template | 创建 deterministic 模板骨架并锁定 appRoot，不生成业务页面内容 |
| `project.detect_root` | build/edit/repair | template | 读取或发现当前 appRoot，避免 `project/project` 路径分裂 |
| `project.build` | build/edit/repair | template | 执行框架 build 并写入结构化 latest build status |
| `preview.start` | build/edit/repair | preview | 从 appRoot 启动或重启 preview server，并写入 preview 状态 |
| `preview.status` | all sandbox agents | read-preview | 读取 preview 状态 |
| `preview.stop` | build/edit/repair/export | preview | 停止 preview server |
| `diagnostics.typescript` | build/edit/review/repair | diagnostics | 获取 TS/LSP 诊断 |
| `diagnostics.build_log` | build/edit/review/repair | diagnostics | 读取构建日志摘要 |
| `browser.open` | build/edit/review/repair | browser | 打开预览页面 |
| `browser.screenshot` | build/edit/review/repair | browser | 截图并保存 |
| `browser.inspect` | build/edit/review/repair | browser | 检查 DOM、控制台错误、可访问性基础问题 |

### 9.4 Artifact tools

| Tool | Agent | 权限 | 说明 |
|---|---|---|---|
| `artifact.create_zip` | export | artifact | 打包源码 |
| `artifact.upload` | export | artifact | 上传导出物到内部对象存储 |
| `artifact.create_report` | export/review | artifact | 生成报告 |

### 9.5 Kubernetes / sandbox tools

这类工具不直接暴露给 Build Agent，通常只给 Harness Orchestrator。

| Tool | 调用方 | 说明 |
|---|---|---|
| `sandbox.claim` | orchestrator | 通过 `SandboxClaim` 获取或创建 sandbox |
| `sandbox.get_status` | orchestrator | 查询 sandbox 状态 |
| `sandbox.resume` | orchestrator | 恢复暂停的 sandbox |
| `sandbox.pause` | orchestrator | 空闲暂停 |
| `sandbox.delete` | orchestrator | 清理废弃 sandbox |
| `sandbox.open_channel` | orchestrator | 建立工具调用通道 |

---

## 10. 权限模型

### 10.1 权限矩阵

| Capability | brief | build | repair | visual-review | edit | export |
|---|---:|---:|---:|---:|---:|---:|
| read content sources | allow | allow | deny | deny | allow | deny |
| write brief | allow | deny | deny | deny | deny | deny |
| read workspace | deny | allow | allow | allow | allow | allow |
| write workspace | deny | allow | allow | deny | allow | outputs only |
| shell | deny | allow | allow | deny | allow | limited |
| package install | deny | allow | ask/allow | deny | ask | deny |
| browser inspect | deny | allow | allow | allow | allow | deny |
| network public internet | deny | deny by default | deny by default | deny | deny by default | deny |
| ask user | allow | allow | allow | deny | allow | allow |
| complete run | allow | allow | allow | allow | allow | allow |

> **注：** `visual-review` 的 shell 权限为 `deny`。该 agent 通过 `browser.*`、`preview.status` 和 `diagnostics.*` 工具完成只读评审，无需执行 shell 命令。工具目录 9.3 节中 `visual-review` 不列 `shell.run` 与此一致。

### 10.2 Command policy

命令权限采用参数列表检查，而不是字符串前缀匹配。`shell.run` 工具必须以 exec array 方式执行（不通过 shell 解释器），并对第一个参数和完整参数列表分别做策略检查，防止 `sh -c “kubectl get pods”` 等绕过方式。

```text
allow（第一个参数匹配以下之一）:
  pwd
  ls
  find
  rg
  cat
  sed
  node
  pnpm（子命令为 install / build / dev / lint / test / run）
  npm（子命令为 run）

ask（转为平台策略审批，不弹给设计师）:
  pnpm add
  npm install
  npx
  git

deny（第一个参数匹配以下之一，或参数列表中出现以下 pattern）:
  sh（含 sh -c）
  bash（含 bash -c）
  rm（参数包含 -rf 且目标不在 /workspace 内）
  curl / wget（目标 URL 非内部白名单）
  kubectl
  docker
  ssh
  scp
  sudo
  chmod / chown（目标不在 /workspace 内）
```

**安全实现要求：**
- `shell.run` 接收 `{ argv: string[] }` 而不是 `{ cmd: string }`，不允许接收一个拼接字符串再交给 shell 解释器执行。
- permission engine 在检查命令时对 `argv[0]` 做白名单匹配，同时扫描完整 `argv` 是否包含 deny pattern。
- 任何无法归类到 allow/ask/deny 的命令，默认 deny 并写入 audit log。

内部产品里 `ask` 转为平台策略审批，不弹给设计师。常规生成路径的命令（pnpm install/build/dev）通过模板白名单自动放行，不产生审批弹窗。

### 10.3 Permission resolution

权限必须由 harness 在工具执行前强制拦截，不能只写进 prompt。参考 OpenCode 的 permission 模型，本产品采用分层合并，但内部平台默认更保守：

```text
1. Organization managed policy deny
2. Project/security policy deny
3. Agent profile deny
4. Run-time scoped deny
5. Run-time scoped allow/ask
6. Agent profile allow/ask
7. Platform default
```

决议规则：

- `deny` 永远优先于 `allow` 和 `ask`。
- 没有显式声明的写入、shell、网络、外部目录和 secret 读取默认 `deny`。
- `read` 只在 workspace 和当前 project 内容源内默认允许。
- 所有工具调用必须同时通过 tool permission、path permission、command/network policy。
- `ask` 在设计师产品里不一定弹给设计师；可以转成平台策略审批、管理员审批或安全提示。
- 权限决议结果必须写入 run log，左侧 chat 只展示用户可理解的摘要。

### 10.4 Path, secret, and network policy

- `external_directory`：任何 `/workspace` 之外的读写默认 deny。`fs.read`、`fs.write`、`fs.patch`、`fs.delete` 在执行前必须对传入路径做 `realpath` 解析，将 symlink 展开为绝对路径后再做边界检查，防止 agent 在 `/workspace` 内创建指向外部路径的 symlink 绕过限制。
- `secrets`：`.env`、`.env.*`、token、private key、kubeconfig、cloud credential 等路径和内容模式默认 deny。路径匹配在 realpath 解析后执行。
- `content_sources`：sandbox 只能读取当前 project 绑定的内容源和附件索引。
- `network`：默认 deny public internet，只允许内部 LLM 网关、内部 package registry、对象存储和 preview router。
- `package_install`：默认走内部 registry/proxy；禁止或隔离 npm lifecycle scripts，除非模板策略明确允许。
- `runtime_policy_profile`：默认 `production`；只有测试 harness 或管理员路径可以显式启用 `local-e2e`，用于 public npm 等本地 E2E 例外，且必须写 audit。
- `app_root`：Build/Edit 阶段源码写入必须落在 runtime 锁定的 appRoot 下；创建 nested package root（如 `/workspace/project/project/package.json`）必须返回 recoverable guidance。
- `egress_audit`：所有 sandbox 外联请求必须记录目标、runId、projectId 和 agentProfile。

---

## 11. Prompt Harness

### 11.1 System Prompt 组成

每个 agent 的 system prompt 由 harness 动态拼装：

```text
base identity
  + product rules
  + agent role
  + current phase
  + available tools
  + permission summary
  + project context
  + brief/design context
  + template guidance
  + completion rules
  + safety boundaries
```

### 11.2 Brief Agent prompt 骨架

```md
# Identity

You are the Brief Agent for an internal AI Website / Docs generator.
Your job is to turn prompt, Markdown, and attachments into a clear generation Brief.

## Core Behavior

- Do not generate source code.
- Do not create or modify project files.
- Extract intent, audience, content structure, missing information, and visual direction.
- Prefer a Brief that a designer can inspect and correct.
- If the input is ambiguous, make the best useful draft and mark assumptions.

## Output

Create or update the Brief with:
- project type: Website or Docs
- audience
- content hierarchy
- page or docs structure
- visual direction
- recommended technical template
- assumptions
- missing information

## Completion

- When the Brief is ready for user review, call run.complete with status success.
- If a blocking content source is unreadable and cannot be processed, call run.complete with status blocked and explain which source caused the block.
- If the user input cannot be interpreted as any meaningful content (e.g., empty prompt, corrupted file with no extractable text), call user.ask or brief.request_confirmation so the runtime pauses the run as needs_user_input. Do not call run.complete for needs_user_input, and do not attempt to generate a Brief from nothing.
```

### 11.3 Build Agent prompt 骨架

```md
# Identity

You are the Build Agent for one internal design project.
You run inside an isolated sandbox and create a high-quality Website or Docs project.

## Core Behavior

- Read /workspace/inputs/brief.md before writing files.
- Read /workspace/inputs/design.md if present.
- Generate a real runnable project in /workspace/project.
- Use the selected technical template.
- Keep the output polished, coherent, and suitable for designer review.
- Prefer stable, simple implementation over clever complexity.

## Workflow

1. Inspect the workspace.
2. Create or update a short task list.
3. Use project.init or project.detect_root to establish the app root.
4. Generate project files and content under the app root.
5. Install dependencies if needed.
6. Build with project.build.
7. Start preview.
8. Inspect preview with browser tools.
9. Fix recoverable issues.
10. Update context.md with decisions.
11. Call run.complete.

## Boundaries

- Do not access files outside /workspace.
- Do not call public internet unless a tool result says it is allowed.
- Do not expose internal content in external links.
- Do not stop after writing files; verify by building and previewing.
- project.* tools manage template lifecycle only; source, content, layout, and style remain agent-generated through fs.*.
```

### 11.4 Edit Agent prompt 骨架

```md
# Identity

You are the Edit Agent for an existing generated Website or Docs project.

## Core Behavior

- Preserve the existing project unless the user asks for a major direction change.
- Read current context.md, brief.md, design.md, and relevant files before editing.
- Make focused changes.
- Rebuild and refresh preview.
- If the requested change conflicts with the confirmed Brief, explain and ask whether to update the Brief.
```

---

## 12. Tool 调用顺序

### 12.1 Prompt / 附件到 Brief

```text
1. content.list_sources
2. content.read_source(prompt)
3. content.read_source(markdown files)
4. content.read_source(attachment extracted text)
5. brief.write_draft
6. run.report_progress("Brief 已整理")
7. run.complete(status=success)
8. UI 展示 Brief
```

用户要求修改 Brief：

```text
1. brief.read
2. content.read_source(relevant source if needed)
3. brief.update
4. run.complete(status=success)
```

### 12.2 Brief 确认到首次生成

```text
1. sandbox.claim(template)
2. sandbox.wait_ready(timeout=120s)   # 等待 sandbox 从 starting 进入 ready；冷启动需轮询或 watch SandboxClaim.status
3. sandbox.open_channel
4. fs.write(/workspace/inputs/brief.md)
5. fs.write(/workspace/inputs/design.md if present)
6. fs.write(/workspace/inputs/content-sources.json)
7. fs.list(/workspace)
8. fs.write(/workspace/state/tasks.json)
9. project.init(template=astro-website, path=/workspace/project) or project.detect_root
10. fs.write project files under appRoot
11. package.install if additional dependencies are needed
12. project.build
13. emit preview.rebuilding
14. preview.start(cwd=appRoot)
15. browser.open(preview_url)
16. browser.screenshot
17. diagnostics.build_log / browser.inspect
18. emit preview.candidate(versionId)
19. repair loop if needed
20. visual review if policy requires
21. promote candidate with emit preview.updated(versionId)
22. fs.write(/workspace/state/context.md)
23. run.complete(status=success)
```

**sandbox.wait_ready 说明：** `sandbox.claim` 提交后 sandbox 进入 `starting` 状态，必须等待 ready 信号才能调用 `open_channel`。实现上应使用 K8s watch 监听 `SandboxClaim.status.phase == Ready`，并设置超时上限（建议 120 秒）。超时后 orchestrator 应重试 claim 或向用户报告 `sandbox_unavailable`，不得直接进入 `open_channel`。WarmPool 命中时通常在几秒内就绪；冷启动路径可能需要更长等待，前端应在 `sandbox.claiming` 状态下展示有意义的等待提示（如"正在准备生成环境，首次启动需要较长时间"），不得显示超时错误直到超限。

### 12.3 自动修复 loop

```text
1. diagnostics.build_log
2. fs.search / fs.read relevant files
3. fs.patch or fs.write
4. shell.run build/check command
5. preview.start if build passes
6. browser.inspect
7. emit preview.candidate(versionId) when a candidate becomes reachable
8. repeat up to bounded max attempts
9. run.complete(success | partial | blocked)
```

修复 loop 必须有限制：

- 同一错误最多修 3 次。
- 同一工具相同参数重复 3 次触发 `doom_loop`。
- 超出预算后返回 `partial` 或 `blocked`，不要无限循环。

### 12.4 对话式修改

```text
1. fs.read(/workspace/state/context.md)
2. brief.read or fs.read(/workspace/inputs/brief.md)
3. fs.search relevant files
4. fs.patch focused changes
5. project.build; shell.run("pnpm build") may be used only for diagnostics/repair and does not write formal gate evidence
6. emit preview.rebuilding(previousVersionId)
7. preview.start(cwd=appRoot)
8. browser.screenshot
9. emit preview.candidate(versionId)
10. repair/review if needed
11. promote candidate with emit preview.updated(versionId)
12. fs.write(/workspace/state/context.md)
13. run.complete(status=success)
```

对话式修改期间，左侧 chat message 不能只展示最终回答，还要展示用户可理解的中间状态。推荐消息形态：

```text
User: 首页视觉再更强一点，减少大段文字。
Assistant: 我会保留当前信息结构，强化首屏视觉层级，并压缩长段正文。
Tool summary: 正在更新首页结构和文案密度。
Tool summary: 正在重新构建预览。
Preview: 新版本已就绪。
Assistant: 已完成，右侧预览已更新。
```

---

## 13. Harness Loop 伪代码

```ts
async function runAgent(runId: string) {
  const run = await loadRun(runId)
  const profile = await loadAgentProfile(run.agentProfile)
  const context = await buildRuntimeContext(run)
  const tools = createToolRouter({
    run,
    profile,
    permissions: profile.permissions,
    sandbox: run.sandboxId ? await connectSandbox(run.sandboxId) : undefined,
  })

  let messages = buildInitialMessages(profile.systemPrompt, context)
  let iterations = 0

  while (iterations < profile.maxIterations) {
    iterations++
    await saveCheckpoint(run, messages)

    const response = await model.call({
      model: profile.model,
      messages,
      tools: tools.describeAvailableTools(),
    })

    await appendAssistantMessage(run, response)

    if (!response.toolCalls.length) {
      emptyTurns++
      if (emptyTurns >= 3) {
        await finalizeRun(run, {
          status: "partial",
          summary: "Agent stopped responding with tool calls after 3 consecutive empty turns.",
        })
        return
      }
      messages.push({
        role: "system",
        content: "Continue working or call run.complete if the task is done.",
      })
      continue
    }
    emptyTurns = 0

    for (const call of response.toolCalls) {
      const decision = await authorizeToolCall(run, profile, call)
      if (decision.type === "deny") {
        const result = toolDeniedResult(call, decision.reason)
        messages.push(resultToMessage(result))
        continue
      }

      if (decision.type === "ask") {
        await emitPermissionRequest(run, call, decision.reason)
        await pauseRun(run, "needs_user_input")
        return
      }

      await emitToolStarted(run, call)
      const result = await tools.execute(call)
      await emitToolResult(run, call, result)
      messages.push(resultToMessage(result))

      if (result.shouldContinue === false) {
        await finalizeRun(run, result)
        return
      }
    }

    if (shouldCompact(messages)) {
      messages = await compactMessages(run, messages)
    }
  }

  await finalizeRun(run, {
    status: "partial",
    summary: "Reached max iterations before explicit completion.",
  })
}
```

---

## 14. 用户确认机制

确认点分三类：

### 14.1 产品确认

必须由设计师确认：

- Brief 是否可作为生成依据。
- 作品类型：Website / Docs。
- 技术模板选择。
- 方向性大改是否更新 Brief。

### 14.2 平台策略确认

通常不让设计师处理，由平台策略决定：

- 是否允许安装新依赖。
- 是否允许访问内部 registry 之外的地址。
- 是否允许导出包含敏感附件的源码包。
- 是否允许长时间占用 sandbox。

### 14.3 高风险确认

需要管理员或高级权限：

- 公网网络访问。
- 将结果发布到外部地址。
- 读取跨项目资产。
- 使用高敏凭证。
- 执行 destructive command。

---

## 15. 与 Kubernetes / agent-sandbox 的交互

Harness 不直接管理 pod，而是管理作品和 run。底层通过 agent-sandbox 资源完成执行环境分配。

### 15.1 首次生成

```text
GenerationRun.created
  -> Orchestrator chooses SandboxTemplate
  -> Create SandboxClaim
  -> Wait for Sandbox ready
  -> Bind Project to Sandbox
  -> Open tool channel
  -> Start Build Agent loop
```

### 15.2 后续修改

```text
User sends edit message
  -> Resolve Project SandboxBinding
  -> Resume sandbox if paused
  -> Open tool channel
  -> Start Edit Agent loop
```

### 15.3 空闲处理

```text
No active run for N minutes
  -> Stop preview
  -> Save checkpoint
  -> Pause sandbox
```

### 15.4 清理处理

```text
Project archived or TTL expired
  -> Export final artifacts if policy requires
  -> Delete SandboxClaim / Sandbox
  -> Retain DB metadata and object storage artifacts
```

---

## 16. Preview 与视觉检查

生成完成不等于任务完成。Build Agent 必须至少完成：

- 项目构建成功。
- preview server 可访问。
- 首屏截图成功。
- 浏览器控制台无关键错误。
- 页面没有明显空白或崩溃。

Review Agent 可以只读运行：

```text
preview.status
browser.open
browser.screenshot
browser.inspect
diagnostics.build_log
run.complete(summary)
```

Review Agent 的输出不直接改代码，而是：

- 如果问题清晰且可恢复，触发 Repair Agent。
- 如果问题涉及产品方向，回到用户或 Brief。
- 如果只是建议，展示给设计师。

### 16.1 Review / Repair run graph

Review Agent 和 Repair Agent 应像 OpenCode subagent child session 一样拥有独立 run，同时挂在主 Build/Edit run 下，便于追踪、折叠和恢复。

```text
BuildRun
  -> VisualReviewRun(parentRunId=BuildRun.id)
       -> review.finding[]
       -> RepairRun(parentRunId=VisualReviewRun.id, findingIds=[...])
            -> preview.candidate
            -> review/repair outcome
  -> preview.updated
  -> run.completed
```

建议数据结构：

```ts
type ReviewFinding = {
  id: string
  projectId: string
  runId: string
  versionId: string
  severity: "info" | "warning" | "blocking"
  category: "build" | "runtime" | "visual" | "content" | "safety"
  summary: string
  evidence?: {
    screenshotId?: string
    filePath?: string
    logExcerpt?: string
  }
  repairable: boolean
  status: "open" | "repairing" | "fixed" | "accepted" | "needs_user_input"
}
```

左侧 chat 展示建议：

```text
Review: 发现 3 个问题，其中 2 个可自动修复。
Repair: 已修复 2 个问题。
Needs input: 1 个内容方向问题需要你确认。
```

### 16.2 Detail Workspace 联动规则

作品详情页采用左侧 chat、右侧 preview 的双栏布局。Harness 必须保证：

- 左侧所有用户消息、agent 消息、工具摘要、错误恢复和确认请求按时间顺序追加。
- 右侧 preview 在新版本未 ready 前保留上一个可用版本，并显示 rebuilding 状态。
- `preview.candidate` 只更新后台候选版本和高级调试面板，不替换右侧正式预览。
- `preview.updated` 到达后，右侧切换到新版本并标记对应 `versionId`。
- `run.completed` 不应早于最终 `preview.updated`，除非 run 以 `partial` 或 `blocked` 结束。
- 用户在左侧发起新修改时，必须基于右侧当前版本对应的 checkpoint 开始。

---

## 17. 失败处理

### 17.1 失败类型

| 类型 | 处理 |
|---|---|
| 内容源读取失败 | Brief Agent 标记 blocked，请用户补充或删除附件 |
| Brief 不完整 | 生成可用草稿，标记 assumptions |
| Sandbox claim 失败 | Orchestrator 重试或切换模板池 |
| 依赖安装失败 | Repair Agent 尝试使用内部 registry/cache |
| 构建失败 | Repair Agent 读取日志并修复 |
| Preview 启动失败 | 检查端口、脚本、构建产物 |
| 视觉质量差 | Review Agent 生成问题报告，Edit/Repair Agent 修复 |
| 权限拒绝 | 转换为用户可理解的安全提示 |
| 迭代耗尽 | 返回 partial，并保存 checkpoint |

### 17.2 UI 反馈

不要向设计师直接展示长构建日志。应展示：

```text
正在修复预览构建问题
已发现依赖版本冲突，正在调整
预览已恢复
需要你确认：当前内容缺少产品受众描述
```

日志详情可以在高级面板中提供。

---

## 18. 质量门禁

### 18.1 Build Gate

- install 成功。
- build 成功。
- preview 可访问。
- 无明显 runtime error。
- 关键页面存在。
- 生成内容覆盖 Brief 中的主要结构。

### 18.2 Design Gate

- 首屏不是空白。
- 标题、正文、导航层级清楚。
- Website 有明确叙事和 CTA。
- Docs 有明确目录和章节结构。
- 视觉风格与 design.md / prompt 不冲突。

### 18.3 Safety Gate

- 没有外部未授权链接或资源上传。
- 没有把附件原文泄露到不该展示的位置。
- 没有跨项目文件访问。
- 没有读取环境密钥或暴露 token。
- 没有通过 package manager、postinstall script 或 shell 命令绕过网络策略。
- 没有访问 `.env`、kubeconfig、cloud credential、private key 或其它 secret pattern。

---

## 19. 配置形态

可以设计类似 OpenCode 的配置层，但面向平台内部：

```yaml
agents:
  brief:
    model: internal-balanced
    maxIterations: 8
    permissions:
      content.*: allow
      brief.*: allow
      run.*: allow
      user.ask: allow
      shell: deny
      workspace.write: deny
      network: deny

  build:
    model: internal-balanced
    maxIterations: 40
    permissions:
      fs.read:
        "/workspace/**": allow
      fs.write:
        "/workspace/project/**": allow
        "/workspace/state/**": allow
        "/workspace/outputs/**": allow
        "*": deny
      workspace.write:
        "/workspace/project/**": allow
        "/workspace/state/**": allow
        "*": deny
      appRoot:
        sourceWrites: require_app_root
        nestedPackageRoot: recoverable_error
      shell:
        "pnpm install": deny  # use package.install
        "pnpm build": allow   # diagnostics/repair only; formal gate evidence comes from project.build
        "pnpm dev": allow
        "rm -rf *": deny
      package.install:
        registry: internal-only
        lifecycleScripts: deny
      preview.*: allow
      browser.*: allow
      diagnostics.*: allow
      run.*: allow
      user.ask: allow
      network:
        public: deny
        internal_registry: allow
        internal_llm_gateway: allow
      external_directory: deny
      secrets: deny

  visual-review:
    model: internal-fast
    maxIterations: 10
    permissions:
      fs.read:
        "/workspace/project/**": allow
        "/workspace/state/preview.json": allow
        "*": deny
      preview.*: allow
      browser.*: allow
      diagnostics.*: allow
      run.*: allow
      workspace.write: deny
      shell: deny
      network: deny
```

配置原则：

- 工具分两层管理：catalog availability 决定工具是否存在/是否进入模型工具定义，run permission 决定当前 run/profile/input 是否允许执行。
- `is_enabled=false`、deferred tool、MCP stub、feature flag 属于 catalog availability；这些工具不可见或延迟可见。
- 可见但未授权的工具必须经过 permission engine 返回 Ask/Deny，并写入 audit；不能用隐藏工具替代应审计的拒绝。
- 平台 managed policy 优先级最高，项目和 agent 不能放宽它。
- Agent profile 只表达该角色需要的最小权限。
- Run-time scope 进一步收窄到当前 project、当前 sandbox 和当前 version。

---

## 20. MVP 推荐实现顺序

### Step 1：Harness Skeleton

- `AgentRun` / `AgentEvent` / `AgentCheckpoint`。
- `ConversationItem` 持久化模型。
- SSE 或 WebSocket 事件流。
- `run.complete` 显式完成工具。
- Tool Router、permission resolution 和权限拦截。

### Step 2：Brief Agent

- `content.*` 和 `brief.*` 工具。
- prompt/Markdown/附件解析文本 -> Brief。
- Brief 修改和确认流。

### Step 3：Sandbox Build Agent

- `sandbox.claim`。
- workspace 文件写入。
- `fs.*`、`shell.run`、`preview.*`。
- Astro 模板优先。

### Step 4：Preview + Repair

- preview router。
- `preview.rebuilding` / `preview.candidate` / `preview.updated` 三段事件。
- browser screenshot。
- build log diagnostics。
- bounded repair loop。
- Review / Repair parent-child run graph。

### Step 5：Edit Agent

- 基于已有 sandbox 修改。
- context.md 更新。
- rebuild preview。

### Step 6：Export Agent

- 源码 zip。
- 生成报告。
- artifact upload。

---

## 21. 最小端到端样例

```text
User:
  上传 product.md，并输入：
  “做成一个高级、克制、有技术感的产品官网。”

Brief Agent:
  content.list_sources
  content.read_source(product.md)
  brief.write_draft
  run.complete

User:
  修改 Brief：“受众改成企业 CTO，页面更偏安全可信。”

Brief Agent:
  brief.read
  brief.update
  run.complete

User:
  确认 Brief，选择 Astro Website。

Harness:
  sandbox.claim(astro-website)
  write inputs into /workspace/inputs
  start Build Agent

Build Agent:
  fs.read(brief.md)
  project.init(template=astro-website)
  fs.write(project files)
  package.install(if needed)
  project.build
  emit preview.rebuilding
  preview.start
  browser.screenshot
  emit preview.candidate(versionId)
  visual review / repair if needed
  emit preview.updated(versionId)
  run.complete

User:
  “首页视觉再更强一点，减少大段文字。”

Edit Agent:
  fs.read(context.md, brief.md, relevant files)
  fs.patch
  project.build
  emit preview.rebuilding(previousVersionId)
  preview.start
  browser.screenshot
  emit preview.candidate(versionId)
  repair/review if needed
  emit preview.updated(versionId)
  run.complete
```

---

## 22. 参考资料

- [OpenCode Agents](https://opencode.ai/docs/agents/)
- [OpenCode Tools](https://opencode.ai/docs/tools/)
- [OpenCode Permissions](https://opencode.ai/docs/permissions/)
- [OpenCode Skills](https://opencode.ai/docs/skills/)
- [OpenCode Config](https://opencode.ai/docs/config/)
