---
date: 2026-07-04
status: active
type: spec
phase: A
sources:
  - ./2026-07-04-mvp-implementation-plan.md
  - ./2026-07-04-agent-harness-design.md
  - ./2026-07-04-rust-agent-harness-delivery-review.md
  - ./claude-code-main/src/Tool.ts
  - ./claude-code-main/src/query.ts
  - ./claude-code-main/src/QueryEngine.ts
  - ./claude-code-main/src/services/tools/StreamingToolExecutor.ts
  - ./claude-code-main/src/utils/permissions/permissions.ts
---

# Rust Agent Runtime Spec — Phase A

本文档是 Phase A 的完整实施规范，面向开发者和评审者使用。每个模块包含：接口定义、行为要求、完成标准、测试要求。

Phase A 的冻结目标是 Astro Website runtime loop。Fumadocs Docs 是 Phase A.5，在 Phase A runtime API 冻结之后复用同一 runtime 能力补齐第二条模板闭环。

所有数据类型以 TypeScript 描述（便于与 `packages/shared` 对齐），Rust 实现使用 serde/schemars 映射相同结构。

---

## 目录

1. [数据模型规范](#1-数据模型规范)
2. [Tool Trait 规范](#2-tool-trait-规范)
3. [Permission Engine 规范](#3-permission-engine-规范)
4. [Agent Loop 规范](#4-agent-loop-规范)
5. [StreamingToolExecutor 规范](#5-streamingtoolexecutor-规范)
6. [Tool Catalog 规范](#6-tool-catalog-规范)
7. [Sandbox Adapter 规范](#7-sandbox-adapter-规范)
8. [Preview Promotion 规范](#8-preview-promotion-规范)
9. [Checkpoint 规范](#9-checkpoint-规范)
10. [HTTP + SSE API 规范](#10-http--sse-api-规范)
11. [Phase A 验收标准](#11-phase-a-验收标准)
12. [测试要求](#12-测试要求)

---

## 1. 数据模型规范

### 1.1 AgentRun

`packages/shared/src/schemas.ts` 中的规范定义，所有其他文档引用此处。

```ts
type AgentRun = {
  id: string
  projectId: string
  sessionId: string
  parentRunId?: string
  triggeredByEventId?: string     // 触发此 run 的 review.finding 事件 ID
  phase: "brief" | "build" | "repair" | "review" | "edit" | "export"
  agentProfile: string            // "brief" | "build" | "repair" | "visual-review" | "edit" | "export"
  status:
    | "queued"
    | "running"
    | "needs_user_input"
    | "completed"                 // 成功结束，有明确完成信号
    | "partial"                   // 超出修复上限或 maxIterations，保存 checkpoint 后返回
    | "blocked"                   // 有明确阻塞原因，需要用户或管理员介入
    | "failed"
    | "cancelled"
  model: string
  sandboxId?: string
  briefVersion?: string
  designVersion?: string
  baseVersionId?: string
  outputVersionId?: string        // build/edit run 完成时必须是 promoted 状态才能 run.complete
  findingIds?: string[]           // repair run 正在修复的 finding ID 列表
  inputMessageIds: string[]
  checkpointId?: string
  startedAt: string               // ISO 8601
  updatedAt: string
  completedAt?: string
}
```

**状态机约束：**

- 主路径：`queued` → `running`
- 可恢复暂停态：`running` → `needs_user_input` → `running`
- 终态：`completed | partial | blocked | failed | cancelled`
- 任何终态都不可逆；进入终态时设置 `completedAt`
- `needs_user_input` 不是终态，不设置 `completedAt`，等待 `ContinueRun` 或 `ResolvePermission` 恢复
- `partial` 表示已做了部分工作但未完成，checkpoint 必须存在
- `failed` 表示工具调用 terminal error，或 runtime 无法恢复

### 1.2 AgentEvent

SSE 流中的事件类型，前端消费，`packages/shared/src/events.ts` 规范定义。

```ts
type AgentEvent =
  | { type: "run.started";       runId: string; label: string; timestamp: string }
  | { type: "agent.message";     runId: string; text: string; timestamp: string }
  | { type: "tool.started";      runId: string; tool: string; summary: string; toolUseId: string; timestamp: string }
  | { type: "tool.completed";    runId: string; tool: string; summary: string; toolUseId: string; metadata?: unknown; timestamp: string }
  | { type: "tool.failed";       runId: string; tool: string; error: string; toolUseId: string; recoverable: boolean; timestamp: string }
  | { type: "permission.requested"; runId: string; permissionId: string; tool: string; reason: string; timestamp: string }
  | { type: "permission.denied"; runId: string; tool: string; reason: string; timestamp: string }
  | { type: "state.changed";     runId: string; state: string; timestamp: string }
  | { type: "preview.rebuilding"; runId: string; previousVersionId?: string; timestamp: string }
  | { type: "preview.candidate"; runId: string; url: string; versionId: string; screenshotId?: string; timestamp: string }
  | { type: "preview.updated";   runId: string; url: string; versionId: string; screenshotId?: string; timestamp: string }
  | { type: "review.finding";    runId: string; findingId: string; severity: "info" | "warning" | "blocking"; summary: string; timestamp: string }
  | { type: "run.completed";     runId: string; status: string; summary: string; timestamp: string }
```

**持久化规则：**

- 所有事件写入 `run-log.jsonl`（对象存储或本地文件）和 `agent_events`（用于 SSE 断线重连）
- `agent.message`、重要 `tool.started/completed/failed` 摘要、`permission.*`、`preview.updated`、`review.finding`、`run.completed` 写入前端可见的 `ConversationItem`
- 高频低价值工具日志（如 `fs.read` 小文件）只进 run-log / agent_events，不进 ConversationItem
- `preview.candidate` 写入 run-log / agent_events，但默认不写入前端可见的 ConversationItem；如需调试展示，必须标记为 `visibility = "debug"`

### 1.3 ConversationItem

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
  visibility?: "user" | "debug"   // 默认 user；debug 不进入设计师默认会话流
  createdAt: string
}
```

### 1.4 ReviewFinding

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

### 1.5 Brief JSON Schema

Brief 是生成契约，schema 在 `packages/shared/src/schemas.ts` 定义。

```ts
type Brief = {
  projectType: "website" | "docs"
  audience: string
  contentHierarchy: string[]
  pageStructure: BriefPage[] | BriefSection[]
  visualDirection: string
  recommendedTemplate: "astro-website" | "fumadocs-docs" | "nextjs-website" | "docusaurus-docs"
  assumptions: string[]
  missingInformation: string[]
}

type BriefPage = { title: string; purpose: string; keyContent: string[] }
type BriefSection = { title: string; level: number; content: string }
```

前端从 `Brief` JSON 类型渲染，不做字符串解析。

---

## 2. Tool Trait 规范

### 2.1 接口定义

基于对 claude-code `Tool.ts` 的完整 review，Rust Tool trait 必须包含以下字段：

```rust
#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    // --- 元信息 ---
    fn name(&self) -> &'static str;
    fn input_schema(&self) -> serde_json::Value;    // JSON Schema，用于 model tool definition
    fn input_json_schema(&self) -> Option<serde_json::Value> { None }
    fn output_schema(&self) -> Option<serde_json::Value> { None }
    async fn description(
        &self,
        input: Option<&Value>,
        ctx: &ToolDescriptionContext,
    ) -> String;                                    // 可按 input/profile 动态生成
    fn is_enabled(&self, ctx: &ToolContext) -> bool { true }
    fn aliases(&self) -> &'static [&'static str] { &[] }
    fn tool_loading(&self) -> ToolLoadingPolicy { ToolLoadingPolicy::Eager }
    fn mcp_info(&self) -> Option<McpToolInfo> { None }

    // --- 并发/安全性判断（per-input，不是静态值）---
    fn is_read_only(&self, input: &Value) -> bool;
    fn is_concurrency_safe(&self, input: &Value) -> bool;
    fn is_destructive(&self, input: &Value) -> bool;    // 影响 permission UI 展示
    fn interrupt_behavior(&self) -> InterruptBehavior { InterruptBehavior::Block }
    fn is_search_or_read(&self, input: &Value) -> SearchReadKind { SearchReadKind::None }
    fn requires_user_interaction(&self) -> bool { false }

    // --- 输入验证（在 permission check 之前）---
    async fn validate_input(
        &self,
        input: Value,
        ctx: &ToolContext,
    ) -> Result<Value, ValidationError>;            // 返回规范化后的 input 或验证错误
    fn normalize_input_for_model(&self, input: Value, ctx: &ToolContext) -> Value { input }
    fn backfill_observable_input(&self, input: &mut Value) {}
    fn inputs_equivalent(&self, a: &Value, b: &Value) -> bool { a == b }

    // --- 权限检查（tool-specific，在 global policy 之前）---
    async fn check_permission(
        &self,
        input: &Value,
        ctx: &ToolContext,
    ) -> PermissionResult;                          // 见 Section 3

    // --- 执行 ---
    async fn call(
        &self,
        input: Value,
        ctx: ToolContext,
        progress: ProgressSink,
    ) -> Result<ToolResult, ToolError>;

    // --- 结果大小限制 ---
    fn max_result_size_chars(&self) -> usize {
        DEFAULT_MAX_RESULT_SIZE_CHARS                // 默认 200_000
    }
}
```

```rust
pub enum ToolLoadingPolicy {
    Eager,            // 默认：随初始 tool definition 发送给模型
    Deferred,         // 需要 ToolSearch/显式选择后才加载
    AlwaysLoad,       // 即使开启 deferred tools，也必须首轮可见
}

pub enum InterruptBehavior {
    Block,            // 用户发新消息时等待工具完成
    Cancel,           // 用户发新消息时取消工具并丢弃结果
}

pub struct McpToolInfo {
    pub server_name: String,
    pub tool_name: String,
}

pub enum SearchReadKind {
    None,
    Search,
    Read,
    List,
}
```

**关键约束：**

- `is_concurrency_safe` 在 `addTool`（入队时）调用，而不是 `executeTool`（执行时）。计算结果缓存在 `TrackedTool.is_concurrency_safe`。
- `validate_input` 在 permission check 之前调用。输入不合法时返回 error，不进入 permission 流程。
- `input_json_schema` 用于 MCP 或外部工具已经提供 JSON Schema 的场景；没有时 runtime 从内部 schema 映射。
- `output_schema` 用于结构化输出校验、SDK/测试断言和 Brief JSON enforcement；不能只把工具结果视为字符串。
- `normalize_input_for_model` 负责 API-bound 输入规范化；`backfill_observable_input` 只作用于 transcript、hook、SDK stream、audit 中的可观察副本，不得修改已经进入 prompt cache 的原始 tool_use input。
- `interrupt_behavior=Cancel` 的工具在用户提交新消息时必须生成 synthetic cancelled tool_result；`Block` 的工具保持运行并阻塞新 turn。
- `tool_loading=Deferred` 和 `mcp_info` 是 runtime capability，不代表 Phase A 必须接入 Figma MCP；Phase A 可以只实现 contract 和测试 stub。
- `max_result_size_chars` 超过限制时，结果写入 `/workspace/outputs/tool-results/{uuid}.txt`，agent 收到文件路径 + 前 2000 字符预览。这防止大型 build log 爆炸 context。

### 2.2 Tool 可见性与权限边界

Runtime 必须明确区分两层：

1. **Catalog availability**：工具是否存在、是否注册、是否被当前 runtime capability 暴露。由 `ToolRegistry`、`is_enabled`、`tool_loading`、`mcp_info`、feature flag 和环境配置决定。
2. **Run permission**：工具已存在时，当前 run/profile/input 是否允许使用。由 Permission Engine 对 `tool + input + ctx` 决策。

规则：

- 不存在或 disabled 的工具不进入 model tool definition；如果模型仍调用，返回 unknown-tool synthetic error。
- Deferred tool 默认不 eager load，但 metadata 写入 run log/audit；Phase A 可以不实现 `tool.search`。
- 可见但未授权的工具应该保持可解释失败：permission result 为 Ask/Deny，并写入 audit。
- 禁止用 permission rule 伪装 catalog availability；也禁止用 `is_enabled=false` 绕过需要审计的 deny。

### 2.3 ToolResult 和 ToolError

```rust
pub struct ToolResult {
    pub content: String,             // 传回给模型的 tool_result content
    pub is_error: bool,
    pub metadata: Option<Value>,     // 内部使用，不传给模型
    pub context_modifier: Option<Box<dyn FnOnce(ToolContext) -> ToolContext>>,
    // context_modifier 只对非并发安全工具生效（与 claude-code 一致）
}

pub enum ToolError {
    Recoverable(String),             // 工具失败但 run 可以继续（如文件不存在）
    Terminal(String),                // 工具失败且 run 应停止（如 sandbox 断连）
    PermissionDenied(String),        // 权限拒绝（已在 permission engine 处理，这里作兜底）
    Aborted,                         // AbortController 触发
}
```

### 2.4 ProgressSink

```rust
pub struct ProgressSink {
    run_id: String,
    tool_use_id: String,
    tx: mpsc::Sender<AgentEvent>,
}

impl ProgressSink {
    pub fn emit(&self, summary: impl Into<String>) {
        // 发送 tool.started 摘要更新，前端实时展示
    }
}
```

### 2.5 buildTool 默认值

claude-code 的 `buildTool()` 会填充默认值；其中 `is_read_only=false`、`is_concurrency_safe=false` 与本 runtime 保持一致。权限默认值有意更保守：claude-code 默认 `checkPermissions` 返回 `allow`，本 runtime 默认返回 `Passthrough`，最终在 global policy 中转换为 `Ask`，避免新工具忘记声明权限时被静默放行。

```rust
// 安全默认值（fail-closed）
impl<T: Tool> Default for ToolDefaults {
    fn is_read_only(_: &Value) -> bool { false }        // 默认写操作
    fn is_concurrency_safe(_: &Value) -> bool { false } // 默认非并发安全
    fn is_destructive(_: &Value) -> bool { false }
    fn check_permission(_: &Value, _: &ToolContext) -> PermissionResult {
        PermissionResult::Passthrough { message: default_msg() }
    }
    fn max_result_size_chars() -> usize { 200_000 }
}
```

---

## 3. Permission Engine 规范

### 3.1 PermissionResult 类型

基于对 `permissions.ts` 的完整 review，`PermissionResult` 必须包含 4 种变体：

```rust
pub enum PermissionResult {
    Allow {
        updated_input: Value,
        reason: PermissionReason,
    },
    Ask {
        message: String,
        reason: PermissionReason,
        suggestions: Option<Vec<PermissionUpdate>>,
    },
    Deny {
        message: String,
        reason: PermissionReason,
    },
    Passthrough {
        message: String,
        // 工具自身没有意见，交给 global policy 判断
        // 在 pipeline 步骤 3 转换为 Ask
    },
}

pub enum PermissionReason {
    Rule { source: RuleSource, rule_content: String },
    SafetyCheck { classifier_approvable: bool },
    Mode { mode: String },
    Hook { hook_name: String, reason: Option<String> },
    AsyncAgent { reason: String },
    Other { reason: String },
}
```

### 3.2 Permission Pipeline（完整顺序）

实现必须严格遵循以下顺序，任何一步返回非 Passthrough 即停止：

```
has_permissions_to_use_tool(tool, input, ctx):

  1a. get_deny_rule_for_tool(ctx, tool)
      → Deny { reason: Rule }

  1b. get_ask_rule_for_tool(ctx, tool)
      → Ask { reason: Rule }
      （sandbox auto-allow 例外：特定 sandbox 环境下绕过此步）

  1c. tool.check_permission(input, ctx)
      → Allow | Ask | Deny | Passthrough（工具自定义检查）

  1d. 1c 返回 Deny → 返回 Deny

  1e. tool.requires_user_interaction() && 1c 返回 Ask
      → 返回 Ask（bypass mode 也不能跳过）

  1f. 1c 返回 Ask，reason 是 Rule（内容级 ask rule）
      → 返回 Ask（bypass mode 也不能跳过）

  1g. 1c 返回 Ask，reason 是 SafetyCheck
      → 返回 Ask（bypass mode 也不能跳过）

  2a. bypass_permissions mode → Allow

  2b. tool_always_allowed_rule(ctx, tool) 命中 → Allow

  3.  Passthrough → 转为 Ask
      Ask / Deny → 直接返回
```

**关键约束：**

- `Deny` 在步骤 1a，但 `SafetyCheck`（步骤 1g）在 bypass 之前，bypass mode 不能绕过 safety check。
- `Passthrough` 只在步骤 3 转换为 `Ask`，tool 自身不应以 `Ask` 代替 `Passthrough`。
- headless agent（sandbox 内 Build/Edit Agent）的 `Ask` 先走 platform policy hooks，无结果才转 `Deny`。

### 3.3 Hook Permission Resolution

Permission hook 是 permission pipeline 的一部分，不是绕过机制。实现必须区分三类 hook：

| Hook | 触发时机 | 作用 |
|---|---|---|
| `PreToolUse` | tool input 验证后、正式 permission check 前 | 可返回 allow/ask/deny、updated_input、additional_context、prevent_continuation |
| `PermissionRequest` | headless/background agent 遇到 Ask 且不能弹 UI 时 | 可自动 allow/deny/update input/update permissions |
| `PermissionDenied` | classifier 或 policy deny 后 | 可记录审计、提示 agent 可重试，但不能直接绕过 deny |

**PreToolUse 解析规则：**

```rust
resolve_pre_tool_use_hook(hook_result, tool, input, ctx):
  if hook_result == Allow:
    hook_input = hook_result.updated_input.unwrap_or(input)

    if tool.requires_user_interaction() && hook_result.updated_input.is_none():
      return can_use_tool(tool, hook_input, force_decision=None)

    rule_check = check_rule_based_permissions(tool, hook_input, ctx)
    if rule_check == Deny:
      return Deny
    if rule_check == Ask:
      return can_use_tool(tool, hook_input, force_decision=rule_check)
    return Allow(updated_input=hook_input)

  if hook_result == Deny:
    return Deny

  if hook_result == Ask:
    ask_input = hook_result.updated_input.unwrap_or(input)
    return can_use_tool(tool, ask_input, force_decision=hook_result)

  return can_use_tool(tool, input, force_decision=None)
```

**关键约束：**

- Hook `Allow` 不能绕过 deny/ask rule；deny 仍优先，ask 仍需要真实确认或 headless policy 处理。
- 对 `requires_user_interaction=true` 的工具，hook 只有在提供 `updated_input` 时才算已经满足交互；否则仍必须进入 `can_use_tool`。
- Hook 返回的 `updated_input` 替换后续 permission check 和 tool execution 的输入，并写入 audit。
- `prevent_continuation=true` 时，runtime 返回 error tool_result，并且本轮不继续让 agent 基于该工具结果推进。

**Headless PermissionRequest 规则：**

```rust
if permission_result == Ask && ctx.should_avoid_permission_prompts {
  hook_decision = run_permission_request_hooks(tool, input, suggestions)
  if hook_decision == Allow {
    persist_updated_permissions_if_any()
    return Allow(updated_input=hook_decision.updated_input.unwrap_or(input))
  }
  if hook_decision == Deny {
    if hook_decision.interrupt { ctx.abort("permission_denied_by_hook") }
    return Deny
  }
  return Deny(reason=AsyncAgent, message="Permission prompts are not available")
}
```

### 3.4 Command Policy（exec array，不是字符串匹配）

`shell.run` 接收 `{ argv: Vec<String> }`，不接收拼接字符串：

```rust
pub fn check_command_policy(argv: &[String], profile: &AgentProfile) -> PermissionResult {
    let cmd = argv.first().map(|s| s.as_str()).unwrap_or("");

    // 永远 deny（含 sh -c 绕过）
    const ALWAYS_DENY: &[&str] = &[
        "sh", "bash", "zsh", "fish",
        "kubectl", "docker", "ssh", "scp", "sudo",
    ];
    if ALWAYS_DENY.contains(&cmd) {
        return PermissionResult::Deny { message: format!("{} is not allowed", cmd), .. };
    }

    // 扫描完整 argv 是否含 deny pattern
    for arg in argv {
        if is_deny_pattern(arg) {
            return PermissionResult::Deny { .. };
        }
    }

    match (cmd, argv.get(1).map(|s| s.as_str())) {
        // Dependency installation must go through package.install so registry policy,
        // lockfile policy, and audit metadata cannot be bypassed through shell.run.
        ("pnpm", Some("install" | "add")) | ("npm", Some("install")) => {
            Deny { message: "Use package.install for dependency installation".into(), .. }
        },
        ("pnpm", Some("build" | "dev" | "lint" | "test" | "run")) => Allow,
        ("npm", Some("run")) => Allow,
        ("node" | "ls" | "pwd" | "find" | "rg" | "cat" | "sed") => Allow,
        ("npx" | "git", _) => {
            Ask { message: "platform policy approval required", .. }
        },
        _ => Deny { message: format!("{} not in allowlist", cmd), .. },
    }
}
```

### 3.5 Path Policy（realpath 优先）

所有 `fs.*` 工具在执行前必须做边界检查。读已有路径时 canonicalize 完整路径；创建新文件时 canonicalize 最近已存在的父目录，再验证最终路径组件，避免因为目标文件尚不存在而拒绝合法写入。

```rust
pub enum PathAccess {
    Existing,
    CreateFile,
    CreateDir,
}

pub fn check_existing_path(path: &Path) -> Result<PathBuf, PermissionError> {
    // 展开 symlink 为绝对路径（防止 symlink 绕过）
    let real = std::fs::canonicalize(path)
        .map_err(|_| PermissionError::CannotResolve(path.to_owned()))?;
    ensure_workspace_path(&real)?;
    ensure_not_secret_path(&real)?;
    Ok(real)
}

pub fn check_create_path(path: &Path) -> Result<PathBuf, PermissionError> {
    // 1. 找到并 canonicalize 最近已存在的父目录
    let parent = path.parent().ok_or_else(|| PermissionError::CannotResolve(path.to_owned()))?;
    let real_parent = std::fs::canonicalize(parent)
        .map_err(|_| PermissionError::CannotResolve(parent.to_owned()))?;
    ensure_workspace_path(&real_parent)?;

    // 2. 验证最终组件不是空、`.`、`..`，且不包含路径分隔符
    let file_name = path.file_name()
        .ok_or_else(|| PermissionError::CannotResolve(path.to_owned()))?;
    if file_name == "." || file_name == ".." {
        return Err(PermissionError::InvalidPathComponent(path.to_owned()));
    }

    // 3. 在 real parent 下重建目标路径，再检查 workspace 和 secret pattern
    let normalized = real_parent.join(file_name);
    ensure_workspace_path(&normalized)?;
    ensure_not_secret_path(&normalized)?;
    Ok(normalized)
}

fn ensure_workspace_path(real: &Path) -> Result<(), PermissionError> {
    if !real.starts_with("/workspace") {
        return Err(PermissionError::ExternalDirectory(real.to_owned()));
    }
    Ok(())
}

fn ensure_not_secret_path(real: &Path) -> Result<(), PermissionError> {
    if is_secret_path(real.to_str().unwrap_or("")) {
        return Err(PermissionError::SecretPath(real.to_owned()));
    }
    Ok(())
}

fn is_secret_path(path: &str) -> bool {
    const PATTERNS: &[&str] = &[
        ".env", "kubeconfig", ".ssh/", "id_rsa", "id_ed25519",
        ".token", "credentials", "private_key",
    ];
    PATTERNS.iter().any(|p| path.contains(p))
}
```

**工具使用规则：**

- `fs.read`、`fs.list`、`fs.search`、`fs.patch`、`fs.delete` 使用 `check_existing_path`。
- `fs.write` 创建新文件时使用 `check_create_path`；覆盖已有文件时使用 `check_existing_path`。
- `fs.delete` 默认额外限制在 `/workspace/project` 下，且不允许删除 `/workspace`、`/workspace/project`、`/workspace/inputs`、`/workspace/state`、`/workspace/outputs` 根目录。

### 3.6 Audit Log

每次 permission 决策必须写入 audit record，无论 allow/ask/deny：

```rust
pub struct AuditRecord {
    pub id: String,
    pub project_id: String,
    pub run_id: String,
    pub tool: String,
    pub input_summary: String,    // 路径/命令摘要，不记录敏感内容
    pub decision: String,         // "allow" | "ask" | "deny"
    pub reason: String,
    pub created_at: String,
}
```


---

## 4. Agent Loop 规范

### 4.1 核心状态机

Agent Loop 管理单次 AgentRun 的生命周期：

```rust
pub struct QuerySession {
    session_id: String,
    cwd: PathBuf,
    messages: Vec<Message>,
    tool_registry: ToolRegistry,
    agent_profiles: AgentProfileRegistry,
    system_prompt_builder: SystemPromptBuilder,
    model_config: ModelConfig,
    fallback_model: Option<String>,
    max_turns: Option<u32>,
    max_budget_usd: Option<Decimal>,
    task_budget: Option<TaskBudget>,
}

pub struct AgentLoop {
    run_id: String,
    profile: AgentProfile,
    messages: Vec<Message>,           // 完整 message history，每 turn 追加
    tool_registry: ToolRegistry,
    permission_engine: PermissionEngine,
    model_client: ModelClient,
    event_tx: mpsc::Sender<AgentEvent>,
    checkpoint_store: CheckpointStore,
}
```

`QuerySession` 对齐 claude-code `QueryEngine` 的职责：一个会话可包含多次 `submit_message` / `ContinueRun`；`AgentLoop` 是其中某个 `AgentRun` 的执行器。

**QuerySession 必须负责：**

- 组装 system prompt：runtime 默认 prompt + profile prompt + project/context prompt + append_system_prompt。
- 在每个 turn 开始前冻结 tool set：过滤 `is_enabled=false` 的工具，展开 eager/always-load 工具，保留 deferred tool metadata。
- 注册结构化输出 enforcement：Brief Agent 必须以 `output_schema`/JSON Schema 校验 Brief，不能只依赖 prompt 文字。
- 管理 model options：`model`、`fallback_model`、`thinking_config`、`max_output_tokens`、`task_budget`。
- 维护长期 conversation state：message history、permission denials、read-file cache、checkpoint cursor、pending tool summaries。
- 处理 orphaned permission / resume：runtime 重启或用户恢复时，旧 permission request 必须能被 resolved 或安全失效。

### 4.2 主循环逻辑

```
run(ctx):
  empty_turns = 0
  iterations = 0

  loop:
    iterations += 1
    if iterations > profile.max_iterations:
      finalize(run, status=partial, "Reached max iterations")
      return

    save_checkpoint(run, messages)

    response = model_client.stream(messages, profile.system_prompt, tools)

    // 处理流式响应
    try:
      for event in response:
        if event is AssistantMessage:
          messages.push(event)
          emit(agent.message, event.text)

          for tool_call in event.tool_calls:
            tool_executor.add_tool(tool_call, event)
    catch ModelFallbackTriggered:
      emit_missing_tool_results(messages, reason="model fallback triggered")
      tool_executor.discard()
      switch_to_fallback_model()
      continue
    catch ModelOrStreamError as err:
      emit_missing_tool_results(messages, reason=err.message)
      finalize(run, status=failed, err.message)
      return

    // 处理工具执行结果
    for result in tool_executor.get_remaining_results():
      emit(result.event)
      messages.push(result.message)

      if result.is_run_complete:
        finalize(run, result.status)
        return

    // 无工具调用保护
    if response.tool_calls.is_empty():
      empty_turns += 1
      if empty_turns >= 3:
        finalize(run, status=partial, "No tool calls for 3 consecutive turns")
        return
      messages.push(system_nudge("Continue working or call run.complete"))
      continue

    empty_turns = 0

    // context 压缩（token 接近限制时）
    if should_compact(messages):
      messages = compact_messages(run, messages)
```

### 4.3 Agent Loop 硬性不变量

Claude API conversation 对 tool block 有强约束：任何已经进入 transcript / SDK stream / run log 的 `tool_use`，最终都必须有且只有一个对应 `tool_result`。Runtime 必须在所有异常路径维护这个不变量。

**必须覆盖的路径：**

- model streaming 正常结束：所有 tool_use 通过 `StreamingToolExecutor` 产生 tool_result。
- model fallback：旧 attempt 的 assistant partial message tombstone/discard，已暴露的 tool_use 产生 synthetic error tool_result，然后用 fallback model 重试。
- model/runtime error：调用 `emit_missing_tool_results(...)`，再发 error message，run 进入 `failed` 或可恢复 `partial`。
- 用户 interrupt / CancelRun：消费 `get_remaining_results()`，让执行器为 queued/in-progress tools 产生 synthetic interrupted tool_result。
- unknown tool：不能 panic；立即为该 tool_use 生成 `is_error=true` 的 tool_result。
- executor discard：pending/in-flight 结果不得再写入当前 transcript，避免 orphan result 指向旧 attempt 的 tool_use_id。

```rust
fn emit_missing_tool_results(messages: &mut Vec<Message>, reason: &str) {
    let open_tool_uses = find_tool_uses_without_results(messages);
    for tool_use in open_tool_uses {
        messages.push(Message::tool_result(
            tool_use.id,
            format!("Tool call did not complete: {reason}"),
            is_error=true,
        ));
    }
}
```

### 4.4 空回复保护

与 claude-code `query.ts` 的行为一致，但增加计数器：

```rust
if response.tool_calls.is_empty() {
    empty_turns += 1;
    if empty_turns >= EMPTY_TURN_LIMIT {  // EMPTY_TURN_LIMIT = 3
        finalize_run(run, RunStatus::Partial, "Agent stopped calling tools after 3 consecutive empty turns");
        return;
    }
    messages.push(Message::system("Continue working or call run.complete if the task is done."));
    continue;
}
empty_turns = 0;
```

### 4.5 run.complete 工具（Agent 主动完成信号）

`run.complete` 是 agent 主动声明成功完成或阶段性收尾的唯一合法方式，不允许靠进程退出、没有新日志、预览端口启动来判断 `completed`。Runtime 仍可因 guard、取消、权限拒绝、terminal error 或恢复失败将 run 置为 `partial | blocked | failed | cancelled`。

需要用户输入时不调用 `run.complete(status=needs_user_input)`。`user.ask`、`brief.request_confirmation`、permission request 或 Brief 冲突由 runtime 将 run 暂停为 `needs_user_input`，发送 `state.changed` 和对应的可操作消息；用户通过 `ContinueRun` 或 `ResolvePermission` 恢复。

```rust
// run.complete 工具的 call() 实现
async fn call(&self, input: RunCompleteInput, ctx: ToolContext, _: ProgressSink) -> Result<ToolResult, ToolError> {
    // build/edit run 必须先 promote preview
    if matches!(ctx.run.phase, Phase::Build | Phase::Edit) {
        if let Some(vid) = &ctx.run.output_version_id {
            let version = ctx.db.get_project_version(vid).await?;
            if version.status != VersionStatus::Promoted {
                return Ok(ToolResult {
                    content: "Preview has not been promoted. Emit preview.updated before completing the run.".into(),
                    is_error: true,
                    ..Default::default()
                });
                // 不返回 ToolError::Terminal，让 agent 有机会修复
            }
        } else {
            return Ok(ToolResult {
                content: "No output_version_id set. Build must emit a promoted preview before completing.".into(),
                is_error: true,
                ..Default::default()
            });
        }
    }

    ctx.finalize_run(input.status, input.summary).await?;
    Ok(ToolResult { content: "Run completed.".into(), is_error: false, ..Default::default() })
}
```

### 4.6 Context 压缩

当 token 数接近模型上限时，将历史消息压缩写入 `/workspace/state/context.md`，保留最近几轮 + checkpoint 摘要。Phase A 必须先实现 deterministic compact；后续可补 Claude Code 风格的 snip/microcompact/reactive compact。无论采用哪种压缩，必须先保证 tool_use/tool_result 配对完整，再写 checkpoint。

```rust
fn should_compact(messages: &[Message]) -> bool {
    estimate_tokens(messages) > profile.compact_threshold
}

async fn compact_messages(run: &AgentRun, messages: Vec<Message>) -> Vec<Message> {
    // 1. 用 LLM 生成 context.md 摘要（通过 model_client 调用）
    // 2. 写入 /workspace/state/context.md
    // 3. 保留最近 N 轮 message + 系统提示 + 摘要消息
    // 4. 保存 checkpoint
}
```

### 4.7 Child Run / Subagent 语义

Review、Repair、Edit 的子运行不能只靠 `parentRunId` 表达。Runtime 必须在创建 child run 时冻结以下上下文：

```ts
type ChildRunContext = {
  parentRunId: string
  triggeredByEventId?: string
  agentProfile: "visual-review" | "repair" | "edit"
  allowedTools: string[]
  deniedTools: string[]
  permissionMode: "inherit" | "read_only" | "headless_deny_ask" | "bubble"
  transcriptMode: "sidechain" | "shared"
  inheritedMcpServers: string[]
  agentScopedMcpServers: string[]
  sourceCheckpointId: string
}
```

**规则：**

- Review child run 默认 `read_only`，只允许 `browser.inspect/screenshot`、`diagnostics.*`、`preview.status` 和必要只读文件工具。
- Repair child run 只允许修改与 finding 相关的 workspace path；不得继承 parent 的临时 allow rule，除非 rule source 是 org/project 级别。
- Async/headless child run 设置 `should_avoid_permission_prompts=true`；遇到 Ask 时走 `PermissionRequest` hook，无结果则 Deny。
- Child run transcript 使用 sidechain：子运行消息写入独立 run log，并通过 summary/finding/repair result 回填 parent run。
- Child run 结束、abort 或失败时必须清理 agent-scoped MCP server、background shell task、临时 hooks、read-file cache 和 sandbox locks。
- Parent run promotion gate 必须等待 blocking Review/Repair child run 完成或进入明确终态。


---

## 5. StreamingToolExecutor 规范

### 5.1 并发控制模型

基于对 `StreamingToolExecutor.ts` 的完整 review，并发规则如下：

```
canExecuteTool(is_concurrency_safe):
  executing = tools.filter(status == Executing)
  return executing.is_empty()
      || (is_concurrency_safe && executing.all(t => t.is_concurrency_safe))
```

**规则含义：**
- 执行中全是并发安全工具 + 新工具也是并发安全 → 可以并行执行
- 任何一个非并发安全工具在执行 → 新工具必须等待
- 非并发安全工具入队时遇到阻塞 → 停止继续扫描队列（维持顺序）

### 5.2 Bash 错误级联

只有 Bash 工具错误会取消 sibling tools（与 claude-code 一致）：

```rust
if is_error_result && tool.name == "shell.run" {
    self.has_errored = true;
    self.sibling_abort.abort("sibling_error");
    // 其他正在执行的工具收到 abort 信号，生成 synthetic error result
}
// fs.read、browser.screenshot 等工具失败不触发 sibling cancellation
```

### 5.3 is_concurrency_safe 在入队时计算

```rust
fn add_tool(&mut self, block: ToolUseBlock, assistant_msg: AssistantMessage) {
    let tool_def = self.registry.find(&block.name);
    // is_concurrency_safe 在 add_tool 时计算并缓存，不在 execute 时重新计算
    let is_safe = tool_def
        .map(|t| t.is_concurrency_safe(&block.input))
        .unwrap_or(false);

    self.tools.push(TrackedTool {
        id: block.id.clone(),
        block,
        assistant_msg,
        status: ToolStatus::Queued,
        is_concurrency_safe: is_safe,   // 缓存
        results: None,
        pending_progress: vec![],
    });

    self.process_queue();
}
```

### 5.4 Progress messages 立即 yield

progress 消息不等工具完成，立即通过 SSE 流发送：

```rust
// executeTool 内部
for update in generator {
    if update.is_progress {
        tool.pending_progress.push(update.event);
        self.notify_progress_available();  // 唤醒 getRemainingResults
    } else {
        tool.results.push(update.message);
    }
}
```

### 5.5 大结果截断

工具结果超过 `max_result_size_chars` 时：

```rust
async fn apply_result_size_limit(
    tool_name: &str,
    result: &mut ToolResult,
    run_id: &str,
) {
    let limit = tool_registry.get(tool_name).max_result_size_chars();
    if result.content.len() > limit {
        let preview = &result.content[..2000];
        let path = format!("/workspace/outputs/tool-results/{}.txt", uuid());
        fs::write(&path, &result.content).await?;
        result.content = format!(
            "[Output truncated: {} chars. Full output saved to {}]\n\n...",
            result.content.len(), path, preview
        );
    }
}
```

### 5.6 中断、未知工具和 discard

`StreamingToolExecutor` 必须能在 model stream 尚未结束时接收工具调用并执行，同时处理异常路径。

**未知工具：**

```rust
fn add_tool(block: ToolUseBlock, assistant_msg: AssistantMessage) {
    let Some(tool_def) = self.registry.find(&block.name) else {
        self.tools.push(TrackedTool::completed_with_error(
            block.id,
            format!("No such tool available: {}", block.name),
            assistant_msg.id,
        ));
        return;
    };
    // ...
}
```

**用户 interrupt 行为：**

| `interrupt_behavior` | 用户提交新消息时 |
|---|---|
| `Block` | 当前工具继续运行；新消息等待本轮工具结果落盘 |
| `Cancel` | 取消该工具，生成 `is_error=true` 的 synthetic interrupted tool_result，并丢弃真实晚到结果 |

**discard 行为：**

- model streaming fallback、runtime retry 或 attempt 被替换时调用 `executor.discard()`。
- discard 后 queued tools 不再启动，in-flight tools 即使完成也不能写入当前 transcript。
- 已经对外暴露的 tool_use 由 Agent Loop 的 `emit_missing_tool_results` 或 executor synthetic result 补齐。

**context modifier 规则：**

- 只有 `is_concurrency_safe=false` 的工具可以修改 `ToolContext`。
- 并发安全工具返回 context modifier 时必须忽略并记录 debug 日志，避免并发顺序导致 context 非确定。


---

## 6. Tool Catalog 规范

### 6.1 Control-plane Tools（Brief Agent 使用，无 sandbox 依赖）

| Tool | is_read_only | is_concurrency_safe | Permission |
|---|---|---|---|
| `content.list_sources` | true | true | allow |
| `content.read_source` | true | true | allow |
| `brief.write_draft` | false | false | allow（仅 brief profile）|
| `brief.read` | true | true | allow |
| `brief.update` | false | false | allow（仅 brief profile）|
| `brief.request_confirmation` | false | false | ask（需用户确认，进入 `needs_user_input` 暂停态）|
| `run.report_progress` | false | false | allow |
| `run.complete` | false | false | allow（含 promotion 前置检查）|
| `user.ask` | false | false | ask（进入 `needs_user_input` 暂停态） |

**`run.complete` 前置检查逻辑（见 Section 4.5）：**
- Build/Edit phase：`output_version_id` 对应 `ProjectVersion.status == promoted` 才允许完成
- 其他 phase：直接允许完成

### 6.2 Sandbox Filesystem Tools

| Tool | is_read_only | is_concurrency_safe | Path check |
|---|---|---|---|
| `fs.read` | true | true | realpath + /workspace boundary |
| `fs.list` | true | true | realpath + /workspace boundary |
| `fs.search` | true | true | realpath + /workspace boundary |
| `fs.write` | false | false | realpath + /workspace boundary |
| `fs.patch` | false | false | realpath + /workspace boundary |
| `fs.delete` | false | false | realpath + /workspace boundary，默认限制在 /workspace/project |

**`fs.patch` 原子性要求：**

```rust
// fs.patch 必须是原子操作：读取 → 验证 → 写入
// 不允许部分写入后失败（会导致文件损坏）
pub async fn patch(path: &Path, old_str: &str, new_str: &str) -> Result<(), ToolError> {
    let content = fs::read_to_string(path).await?;
    let count = content.matches(old_str).count();
    if count == 0 {
        return Err(ToolError::Recoverable("old_str not found in file".into()));
    }
    if count > 1 {
        return Err(ToolError::Recoverable("old_str found multiple times, provide more context".into()));
    }
    let new_content = content.replacen(old_str, new_str, 1);
    fs::write(path, new_content).await?;
    Ok(())
}
```

### 6.3 Sandbox Execution Tools

| Tool | is_read_only | is_concurrency_safe | Permission |
|---|---|---|---|
| `shell.run` | false | false | command policy（见 Section 3.4）|
| `package.install` | false | false | ask（platform policy）|
| `project.init` | false | false | allow（template policy）|
| `project.detect_root` | true | true | allow |
| `project.build` | false | false | allow（command policy + template policy）|
| `preview.start` | false | false | allow（默认 cwd = appRoot）|
| `preview.status` | true | true | allow |
| `preview.stop` | false | false | allow |
| `diagnostics.build_log` | true | true | allow |
| `diagnostics.typescript` | true | true | allow |
| `browser.open` | false | false | allow |
| `browser.screenshot` | false | false | allow |
| `browser.inspect` | true | true | allow |

**`shell.run` 接口定义：**

```rust
pub struct ShellRunInput {
    pub argv: Vec<String>,    // 不接受拼接字符串，必须是 argv 数组
    pub cwd: Option<String>,  // 默认 /workspace/project
    pub timeout_ms: Option<u64>,  // 默认 60_000
    pub env: Option<HashMap<String, String>>,
}
```

**Template lifecycle tools：**

`project.*` 工具是模板生命周期原子能力，不是 `generate_website` 这类业务黑盒：

- `project.init` 只负责创建 deterministic 模板骨架、写入 `/workspace/state/project.json` 并锁定 `appRoot`。
- `project.detect_root` 读取或发现当前 `appRoot`，用于修复 `project/project` 等路径分裂。
- `project.build` 按模板运行 build，并写入 `/workspace/outputs/build/latest.json` 作为 promotion gate 的 build 输入。
- 页面内容、布局、样式和信息架构仍由 agent 通过 `fs.*` 生成。
- `project.init` 必须由模板版本决定依赖版本、package manager、lockfile 策略和 registry 来源，保证相同模板版本下可复现。
- Build/Edit 阶段的 source 写入必须 appRoot-aware；创建第二个 package root（如 `/workspace/project/project/package.json`）返回 recoverable guidance。
- `preview.start` 默认读取 `/workspace/state/project.json.appRoot` 作为 cwd，并把实际 cwd 写入 `/workspace/state/preview.json`。

**`package.install` registry 限制：**

```rust
pub struct PackageInstallInput {
    pub packages: Vec<String>,
    pub cwd: Option<String>,       // 默认 /workspace/state/project.json.appRoot
    pub registry: Option<String>,  // 默认 RUNTIME_NPM_REGISTRY / 内部 registry/proxy
}

fn check_permission(input: &PackageInstallInput, ctx: &ToolContext) -> PermissionResult {
    if input.registry.is_none() {
        return PermissionResult::Passthrough { .. };
    }

    if is_internal_registry(input.registry.as_ref().unwrap(), ctx) {
        return PermissionResult::Passthrough { .. };
    }

    if ctx.policy_profile == RuntimePolicyProfile::LocalE2e && is_public_npm_registry(input.registry.as_ref().unwrap()) {
        return PermissionResult::Ask { message: "Public npm registry allowed only for local E2E/dev profile", .. };
    }

    PermissionResult::Deny { message: "Production-like sandboxes must use the internal package registry/proxy", .. }
}
```

**Runtime policy profile：**

```rust
pub enum RuntimePolicyProfile {
    Production,
    LocalE2e,
}
```

- 默认值为 `Production`，不能由模型输入覆盖。
- `LocalE2e` 只能由 runtime 配置或测试 harness 显式启用。
- public npm registry 只在 `LocalE2e` 下进入 ask/allow 路径；production-like profile 必须 deny。
- 任何 public registry ask/allow 都必须写 audit record，包含 `runId`、`projectId`、`registry`、`packages`、`toolUseId`。

### 6.4 MCP Adapter 与 Deferred Tools 边界

Phase A 不把 Figma MCP 作为产品输入源，也不要求接通真实 Figma-to-code 流程；但 runtime 必须预留 MCP adapter contract，避免后续接 MCP 时重写 ToolRegistry、permission 和 token budget。

**Phase A 必须实现：**

- `Tool` trait 支持 `mcp_info()`、`input_json_schema()`、`tool_loading=Deferred|AlwaysLoad`。
- ToolRegistry 能注册 `McpToolStub`，其行为是 passthrough schema、permission 走 normal pipeline、call 返回 `MCP adapter not configured` 的 recoverable error。
- Tool definition token 估算必须区分 MCP tools 与 non-MCP tools，优先使用 `input_json_schema`。
- Deferred tool metadata 能被 run log/audit 记录，但 Phase A 可以不暴露 `tool.search` 给 agent。

**Phase A 明确不做：**

- 不连接真实 Figma MCP server。
- 不实现 ToolSearch 排序、select 语义或 pending MCP server 动态加载。
- 不让 Product BFF 直接调用 MCP；MCP 仍是 runtime tool layer 的能力。

**Phase B 或 Figma 输入阶段再实现：**

```rust
pub trait McpAdapter {
    async fn list_tools(&self, server: &McpServerRef) -> Result<Vec<McpToolDefinition>, McpError>;
    async fn call_tool(&self, server: &McpServerRef, tool: &str, input: Value) -> Result<Value, McpError>;
    async fn close_agent_scoped_servers(&self, agent_id: &str);
}
```

MCP tool name 必须保留 server/tool 原名映射，例如 `mcp__figma__get_file` 对应 `McpToolInfo { server_name: "figma", tool_name: "get_file" }`，以便 audit、权限规则和 deferred search 使用。

### 6.5 Preview Promotion Tools（Harness 内部，不暴露给 agent）

`preview.promote` 由 runtime orchestrator 调用，不进入 tool registry，不暴露给 agent 或 Product BFF：

```rust
pub async fn promote_preview(
    project_id: &str,
    run_id: &str,
    candidate_version_id: &str,
) -> Result<ProjectVersion, Error> {
    // 1. 检查 candidate version 存在且属于此 run
    // 2. 原子更新 ProjectVersion.status = promoted
    // 3. 更新 Project.current_version_id
    // 4. 发送 preview.updated 事件
    // 5. 更新 AgentRun.output_version_id
}
```

**promotion 三段状态机：**

```
preview.rebuilding
  → preview.candidate（sandbox 内 Build 完成，URL 可访问）
  → [gate: Build/Review/Safety 检查]
  → preview.updated（promoted，右侧预览切换）
```

前端只消费 `preview.updated`，不消费 `preview.candidate`。


---

## 7. Sandbox Adapter 规范

### 7.1 SandboxClaim 生命周期

```
claim(template_key, project_id):
  1. 创建 SandboxClaim（v1beta1，spec.warmpoolRef）
  2. watch SandboxClaim.status.phase
  3. 等待 phase == Ready，超时 120s
  4. 超时 → status = sandbox_unavailable，向 orchestrator 报告
  5. Ready → 更新 SandboxBinding.status = ready

sandbox lifecycle states:
  unclaimed → claiming → starting → ready → busy → idle → paused → deleted
```

### 7.2 SandboxClaim Manifest

```yaml
apiVersion: extensions.agents.x-k8s.io/v1beta1
kind: SandboxClaim
metadata:
  name: project-{projectId}-{shortId}
  namespace: anydesign-sandboxes
spec:
  warmpoolRef:
    name: anydesign-{templateKey}-pool
  ttl: 4h
```

**前置确认（RA4 开工前必须确认）：**
- agent-sandbox release 版本和 API 是否使用 `warmpoolRef`
- sandbox channel 协议（gRPC / WebSocket）
- 记录在 `infra/agent-sandbox/base/controller-version.md`

### 7.3 Workspace 初始化

SandboxTemplate 启动脚本必须预创建以下目录和文件：

```bash
mkdir -p /workspace/inputs /workspace/project
mkdir -p /workspace/outputs/build /workspace/outputs/export
mkdir -p /workspace/outputs/screenshots /workspace/outputs/reports
mkdir -p /workspace/outputs/tool-results
mkdir -p /workspace/state/checkpoints

echo '[]' > /workspace/state/tasks.json
echo '{}' > /workspace/state/preview.json
```

**不预创建则 agent 首次写入时报 ENOENT，是 RA4 的已知必须处理项。**

### 7.4 SandboxBinding 数据模型

```ts
type SandboxBinding = {
  id: string
  projectId: string
  sandboxName: string
  sandboxClaimName: string
  workspacePvcName: string // 与 sandbox 共同界定该作品的可写 workspace PVC
  warmPoolName: string
  namespace: string
  status: "claiming" | "ready" | "busy" | "idle" | "failed" | "deleted"
  channelProtocol: "grpc" | "websocket"   // 根据 controller-version.md 确认
  lastSeenAt: string
}
```

`sandboxName + workspacePvcName` 是一个作品的 workspace 边界。Agent 的读写工具只能修改该 PVC 挂载到 sandbox 内的 `/workspace` 内容；不同作品不能复用同一个 workspace PVC。


---

## 8. Preview Promotion 规范

### 8.1 三段状态机

```
emit preview.rebuilding (previousVersionId?)
  ↓
Build/Edit Agent 构建成功
  ↓
emit preview.candidate (versionId, url, screenshotId?)
  ↓
gate: latest build success + preview 可访问 + browser 无 console error + screenshot 成功
  ↓ 通过
promote_preview(projectId, runId, candidateVersionId)
  ↓
emit preview.updated (versionId, url)
  ↓
ProjectVersion.status = promoted
Project.current_version_id = versionId
AgentRun.output_version_id = versionId
```

### 8.2 Promotion Gate 检查项

```rust
pub async fn check_promotion_gate(
    candidate_version_id: &str,
    ctx: &BuildContext,
) -> Result<(), GateError> {
    // 1. 最近一次 project.build 成功
    let latest_build = ctx.read_latest_build_status().await?;
    if latest_build.status != BuildStatus::Success {
        return Err(GateError::BuildFailed(latest_build.summary));
    }

    // 2. preview server 可访问
    let preview_status = ctx.preview_status().await?;
    if !preview_status.is_accessible {
        return Err(GateError::PreviewUnreachable);
    }

    // 3. 截图成功（非空白页面）
    let screenshot = ctx.browser_screenshot().await?;
    if screenshot.is_blank() {
        return Err(GateError::BlankPage);
    }

    // 4. 无 blocking review finding（若 review agent 已运行）
    let findings = ctx.get_findings(candidate_version_id).await?;
    let blocking = findings.iter().filter(|f| f.severity == Severity::Blocking).count();
    if blocking > 0 {
        return Err(GateError::BlockingFindings(blocking));
    }

    Ok(())
}
```

### 8.3 前端 Preview URL 约定

```
/preview/{projectId}/current          # 稳定 URL，跟随 promoted 版本，设计师收藏用
/preview/{projectId}/{versionId}      # 精确版本 URL，永久有效，调试/审计用
```

- **禁止在 URL 中使用 runId**，每次 edit 产生新 runId 会导致链接失效
- `/current` 在 rebuild 期间指向上一个 promoted 版本
- `preview.updated` 到达后，`/current` 切换到新版本


---

## 9. Checkpoint 规范

### 9.1 触发时机

以下时机必须保存 checkpoint：

1. Brief 确认后（开始 Build Agent 前）
2. 首次 generation 成功（preview.updated 后）
3. 每次 edit 成功（preview.updated 后）
4. Review/Repair 循环完成后
5. 导出前
6. 迭代上限耗尽时（partial 状态）

### 9.2 Checkpoint 数据结构

```rust
pub struct AgentCheckpoint {
    pub id: String,
    pub run_id: String,
    pub project_id: String,
    pub phase: String,
    pub message_window: Vec<Message>,       // 最近 N 轮 messages
    pub task_list: Vec<AgentTask>,
    pub workspace_snapshot_uri: Option<String>, // 对象存储路径
    pub brief_version: Option<String>,
    pub design_version: Option<String>,
    pub last_known_preview_url: Option<String>,
    pub context_summary: String,            // /workspace/state/context.md 内容
    pub created_at: String,
}
```

### 9.3 Runtime 重启恢复

```
runtime 重启时：
  1. 查找状态为 running 的 AgentRun
  2. 加载最近一个 checkpoint
  3. 如果 checkpoint 存在：
     a. 重建 message window
     b. 恢复 task list
     c. 重新连接 sandbox（如果 sandbox 仍然 ready）
     d. 继续 agent loop
  4. 如果无 checkpoint 或 sandbox 不可用：
     a. 将 run 标记为 failed（recoverable）
     b. 保留 DB 元数据和对象存储制品
     c. 用户可以手动重新触发
```

---

## 10. HTTP + SSE API 规范

### 10.1 端点列表

```
POST  /runs                               StartRun
POST  /runs/{runId}/continue              ContinueRun
POST  /runs/{runId}/cancel               CancelRun
GET   /runs/{runId}/events               StreamRunEvents (SSE)
POST  /permissions/{permissionId}/decision  ResolvePermission
POST  /internal/previews/promote         PromotePreview（test/admin feature flag only; production uses in-process orchestrator call）
GET   /health                            Health check
```

### 10.2 StartRun

```
POST /runs
Content-Type: application/json

{
  "projectId": "string",
  "phase": "brief" | "build" | "repair" | "review" | "edit" | "export",
    "agentProfile": "string",
    "policyProfile": "production" | "local-e2e", // optional；仅测试/管理员路径可设置，默认 production
    "inputContext": {
    "contentSources": [...],     // brief phase 用
    "briefId": "string",         // build/edit phase 用
    "sandboxBindingId": "string", // sandbox phase 用
    "parentRunId": "string",     // repair/review child run 用
    "findingIds": ["string"]     // repair run 用
  }
}

Response 200:
{
  "runId": "string",
  "status": "queued"
}
```

### 10.3 StreamRunEvents（SSE）

```
GET /runs/{runId}/events
Accept: text/event-stream

Response（SSE stream）:
data: {"type":"run.started","runId":"...","label":"Brief Agent","timestamp":"..."}

data: {"type":"agent.message","runId":"...","text":"正在整理内容源...","timestamp":"..."}

data: {"type":"tool.started","runId":"...","tool":"content.read_source","summary":"读取 product.md","toolUseId":"...","timestamp":"..."}

data: {"type":"tool.completed","runId":"...","tool":"content.read_source","summary":"读取完成","toolUseId":"...","timestamp":"..."}

data: {"type":"preview.rebuilding","runId":"...","timestamp":"..."}

data: {"type":"preview.candidate","runId":"...","url":"...","versionId":"...","timestamp":"..."}

data: {"type":"preview.updated","runId":"...","url":"...","versionId":"...","timestamp":"..."}

data: {"type":"run.completed","runId":"...","status":"completed","summary":"Astro website generated.","timestamp":"..."}
```

**断线重连：** 客户端携带 `Last-Event-ID`，服务端从该 ID 之后的事件重放。事件 ID 格式为 `{runId}/{sequence}`。

### 10.4 ResolvePermission

```
POST /permissions/{permissionId}/decision
Content-Type: application/json

{
  "decision": "allow" | "deny",
  "updatedInput": {...}   // optional，允许用户修改输入
}

Response 200:
{ "runId": "string", "status": "running" }
```

收到 `allow` 后，runtime 恢复暂停的 run。

### 10.5 PromotePreview

```
POST /internal/previews/promote
Content-Type: application/json

{
  "projectId": "string",
  "runId": "string",
  "candidateVersionId": "string"
}

Response 200:
{
  "versionId": "string",
  "previewUrl": "string",
  "status": "promoted"
}

Response 409:
{
  "error": "Gate check failed",
  "reason": "blocking_findings | build_failed | preview_unreachable | blank_page"
}
```

**边界约束：**

- `PromotePreview` 是 runtime 内部受控操作。Agent 只能产出 `preview.candidate`，不能直接 promote。
- Product BFF 和前端不调用此接口；生产路径由 runtime orchestrator 在 gate 检查通过后调用内部 `promote_preview(...)`。
- HTTP endpoint 默认禁用，仅用于集成测试或管理员 break-glass 操作。
- 启用 HTTP endpoint 必须同时满足：feature flag 打开、internal network、service auth、admin/test caller 标识、audit record 写入。
- 生产常规路径不得通过 HTTP 调用 promote，必须使用 in-process orchestrator call，避免绕过 gate 或被 BFF 误用。


---

## 11. Phase A 验收标准

Phase A 是 runtime 独立验证阶段。验收时不存在 `apps/web` 代码。所有测试通过 HTTP client 或集成测试直接调用 runtime。

### 11.1 功能验收（必须全部通过）

**Brief Agent 闭环**

- [ ] `POST /runs`（phase=brief）→ SSE stream 包含 `run.started`、至少一条 `agent.message`、`run.completed`
- [ ] Brief Agent 生成的 Brief JSON 包含所有必填字段：`projectType`、`audience`、`contentHierarchy`、`pageStructure`、`visualDirection`、`recommendedTemplate`、`assumptions`、`missingInformation`
- [ ] 空 prompt 输入 → run status=`needs_user_input`，SSE 包含 `state.changed` 和可操作的 `agent.message`
- [ ] 不可读内容源 → `run.completed` status=`blocked`
- [ ] 连续 3 次无工具调用 → `run.completed` status=`partial`
- [ ] 不调用 `run.complete` 工具 → run 不会自动进入 `completed`；只能由 runtime guard/error/cancel 进入 `partial | blocked | failed | cancelled`

**Sandbox Build Agent 闭环（Astro Website）**

- [ ] Brief 确认后，`POST /runs`（phase=build）→ sandbox claim → wait_ready → workspace 初始化 → `project.init` 锁定 appRoot → Astro 项目生成 → `project.build` 成功并写 latest build status → preview 可访问
- [ ] preview 从 `state/project.json.appRoot` 启动，并在 `state/preview.json` 记录实际 cwd
- [ ] SSE stream 依次包含：`preview.rebuilding` → `preview.candidate` → `preview.updated`
- [ ] `preview.updated` 早于 `run.completed`（或同时），不允许反序
- [ ] 在 `output_version_id` 未 promoted 时调用 `run.complete` → agent 收到 error result，不进入终态

**Edit Agent 闭环**

- [ ] `POST /runs/{runId}/continue` 携带 user message → Edit Agent 修改现有项目（不重建）→ 新 preview.updated
- [ ] Brief 冲突时 → run status=`needs_user_input`，左侧显示可操作摘要；用户 `ContinueRun` 后可恢复同一 run
- [ ] 用户在长工具执行期间 ContinueRun：`interrupt_behavior=Block` 的工具完成后再处理新消息，`Cancel` 的工具产生 synthetic interrupted tool_result

**Review/Repair 子运行图**

- [ ] Build run 触发 Review child run，`parentRunId` 正确设置
- [ ] Review child run 使用 sidechain transcript，工具池为 read-only，不继承 parent 的 session allow rule
- [ ] Blocking review finding → gate 拒绝 promotion
- [ ] Repair run 触发后，finding status 从 `open` → `repairing` → `fixed` 或 `needs_user_input`
- [ ] 同一错误 3 次修复失败 → run status=`partial` 或 `blocked`，不无限循环
- [ ] Doom-loop 检测：相同 tool + 相同参数重复 3 次 → 触发 doom_loop，run 进入 `blocked`

**Checkpoint 与恢复**

- [ ] Brief 确认后保存 checkpoint（含 brief_version、message_window）
- [ ] preview.updated 后保存 checkpoint（含 workspace_snapshot_uri、last_known_preview_url）
- [ ] runtime 重启后，`running` 状态的 run 从最近 checkpoint 恢复，继续执行
- [ ] 无 checkpoint 时，run 标记为 `failed`（recoverable），DB 元数据保留

### 11.2 安全验收（必须全部通过）

- [ ] `fs.read("/etc/passwd")` → permission denied，不返回文件内容
- [ ] `fs.read("/workspace/../etc/passwd")`（路径穿越）→ realpath 解析后 denied
- [ ] `/workspace` 内创建指向外部路径的 symlink，再 `fs.read` → realpath 解析后 denied
- [ ] `fs.read(".env")` → secret path check denied
- [ ] Build/Edit 阶段 `fs.write("/workspace/project/project/package.json")` → recoverable guidance，不能静默创建 nested package root
- [ ] `shell.run(["sh", "-c", "pnpm build"])` → argv[0] = "sh"，直接 denied
- [ ] `shell.run(["kubectl", "get", "pods"])` → denied
- [ ] `shell.run(["pnpm", "build"])` → allowed for diagnostics/repair, but does not write formal promotion evidence
- [ ] `shell.run(["pnpm", "install"])` / `shell.run(["npm", "install"])` → denied，提示使用 `package.install`
- [ ] `package.install` 指定 public registry URL → local E2E/dev profile ask/allow with audit；production-like profile deny
- [ ] 网络请求到 public internet → denied（NetworkPolicy 层）
- [ ] sandbox 无法访问控制面 DB
- [ ] 每次工具调用都有对应 audit record（按 projectId/runId/tool 可查）

### 11.3 工程验收

- [ ] AgentRun 有明确终态或可恢复暂停态，不存在永久 `running` 状态
- [ ] 所有工具调用有 `tool.started` + `tool.completed` 或 `tool.failed` 事件对
- [ ] `tool.failed` 包含正确的 `recoverable` 标志
- [ ] 工具结果超过 `max_result_size_chars` 时，agent 收到截断摘要 + 文件路径，不是完整内容
- [ ] SSE 断线重连后从 `Last-Event-ID` 位置重放，不重复发送已收到的事件
- [ ] `GET /health` 在 config 正确加载后返回 `{ "status": "ready" }`
- [ ] `POST /runs/{runId}/cancel` 中断正在执行的 run，已完成的工具结果不丢失
- [ ] 并发 `fs.read` 调用可以并行完成，不互相阻塞
- [ ] `fs.write` 和 `shell.run` 串行执行（不并发）
- [ ] 只有 `shell.run` 错误触发 sibling tools 的 synthetic error cancel，`fs.read` 失败不影响其他工具
- [ ] Mock BFF contract tests pass before API freeze: start run, stream/reconnect events, continue edit, cancel run, resolve permission, and read preview-current through shared API types.

### 11.4 不允许存在的状态

- `apps/web` 目录（Phase A 期间禁止创建）
- run 在没有 `run.complete` 调用的情况下进入 `completed` 状态
- `preview.updated` 出现在 `run.completed` 之后（成功路径）
- `preview.candidate` 直接触发前端切换预览（只有 `preview.updated` 触发）
- `shell.run` 接受字符串参数（必须是 `argv: Vec<String>`）
- path policy 未做 realpath 解析
- Product BFF or frontend calls `PromotePreview`
- Production runtime exposes `/internal/previews/promote` without feature flag, internal auth, and audit

### 11.5 Phase A.5 Docs Template 验收

Phase A.5 在 Phase A runtime API 冻结后运行，不阻塞 Astro runtime freeze。

- [ ] Fumadocs docs 模板：Markdown 内容 → docs 项目 → preview 可访问，包含首页、文档页、导航
- [ ] Docs Brief 包含 navigation、sections、sidebar、content page coverage
- [ ] Fumadocs promotion gate 复用 Phase A 的 build/review/safety gate
- [ ] Docs 缺少必需结构时产生 blocking finding，不 promoted


---

## 12. 测试要求

### 12.1 测试分层

```
层 1 — 单元测试（每个模块内部）
  覆盖：单个函数、单个工具、permission pipeline 各步骤
  依赖：无外部系统，mock model client

层 2 — 集成测试（跨模块，无真实 sandbox）
  覆盖：agent loop + tool executor + permission engine 联动
  依赖：mock model（预设响应序列）+ mock sandbox channel

层 3 — 端到端测试（Phase A 验收前）
  覆盖：完整链路，真实 model API + 真实 sandbox
  依赖：K8s 集群、内部模型网关、内部 package registry
```

### 12.2 Mock Model 实现要求

mock model 是集成测试的核心基础设施，必须在 RA2 完成前就绪：

```rust
pub struct MockModelClient {
    responses: VecDeque<MockResponse>,
}

pub enum MockResponse {
    // agent 发出若干 tool_call，然后结束本轮
    ToolCalls(Vec<MockToolCall>),
    // agent 发出纯文字消息（无工具调用）
    TextOnly(String),
    // 模拟 API 错误
    Error(String),
}

impl MockModelClient {
    pub fn new(responses: Vec<MockResponse>) -> Self { ... }
    pub fn assert_all_consumed(&self) { ... }  // 测试结束时验证所有预设响应都被消费
}
```

**集成测试示例：**

```rust
#[tokio::test]
async fn brief_agent_full_loop() {
    let runtime = TestRuntime::new()
        .with_mock_model(vec![
            MockResponse::ToolCalls(vec![
                tool_call("content.list_sources", json!({})),
                tool_call("content.read_source", json!({"id": "src1"})),
            ]),
            MockResponse::ToolCalls(vec![
                tool_call("brief.write_draft", json!({
                    "projectType": "website",
                    "audience": "enterprise designers",
                    ...
                })),
                tool_call("run.complete", json!({"status": "success", "summary": "Brief ready"})),
            ]),
        ])
        .build();

    let run = runtime.start_run(Phase::Brief, mock_context()).await;
    let events = run.collect_events().await;

    // 验证终态
    let completed = events.iter()
        .find(|e| e.event_type == "run.completed")
        .expect("run should complete");
    assert_eq!(completed.data["status"], "completed");

    // 验证 Brief 写入 DB
    let brief = runtime.db.get_brief(run.project_id).await.unwrap();
    assert_eq!(brief.project_type, "website");
    assert!(!brief.audience.is_empty());
}
```


### 12.3 必要单元测试清单

#### Permission Engine

| 测试名称 | 验证内容 |
|---|---|
| `deny_beats_allow` | 同一 tool 同时有 deny 和 allow rule，deny 胜出 |
| `workspace_read_allowed` | `/workspace/project/src/index.ts` → allow |
| `external_path_denied` | `/etc/passwd` → deny |
| `symlink_escape_denied` | symlink 指向 `/etc/passwd`，realpath 后 → deny |
| `create_new_file_allowed_under_workspace` | `/workspace/project/src/new.ts` 不存在但父目录存在 → allow |
| `create_new_file_parent_symlink_escape_denied` | 父目录 symlink 指向 `/etc`，创建新文件 → deny |
| `secret_path_denied` | `.env`、`kubeconfig`、`id_rsa` → deny |
| `sh_dash_c_denied` | `argv = ["sh", "-c", "pnpm build"]` → deny（argv[0] = sh）|
| `kubectl_denied` | `argv = ["kubectl", "get", "pods"]` → deny |
| `pnpm_build_allowed_for_diagnostics` | `argv = ["pnpm", "build"]` → allow，但不写 promotion build evidence |
| `pnpm_install_requires_package_tool` | `argv = ["pnpm", "install"]` → deny，提示使用 `package.install` |
| `passthrough_to_ask` | tool 返回 Passthrough，pipeline 步骤 3 转为 Ask |
| `safety_check_bypass_immune` | SafetyCheck 类型的 Ask 在 bypass mode 下仍返回 Ask |
| `pre_tool_allow_does_not_bypass_deny_rule` | PreToolUse hook allow 后仍命中 deny rule → deny |
| `pre_tool_updated_input_used_for_permission_and_execution` | hook 返回 updated_input，后续 permission 与 call 使用新输入 |
| `headless_permission_request_hook_allow` | headless Ask 先走 PermissionRequest hook，hook allow 后执行 |
| `headless_permission_request_hook_absent_auto_denies` | headless Ask 无 hook 决策 → deny，reason=AsyncAgent |
| `audit_on_every_decision` | allow/ask/deny 都写入 audit record |

#### StreamingToolExecutor

| 测试名称 | 验证内容 |
|---|---|
| `concurrent_reads_parallel` | 5 个 `fs.read` 同时入队，全部并行执行 |
| `write_serialized_after_read` | `fs.read` 执行中时，`fs.write` 等待 |
| `two_writes_serialized` | 两个 `fs.write` 按入队顺序串行执行 |
| `bash_error_cancels_siblings` | `shell.run` 失败 → 其他并发 `shell.run` 收到 synthetic error |
| `fs_error_no_sibling_cancel` | `fs.read` 失败 → 其他并发 `fs.read` 继续执行 |
| `concurrency_safe_at_enqueue` | `is_concurrency_safe` 在 `add_tool` 时计算，不在 `execute_tool` 时重算 |
| `progress_immediate_yield` | progress 消息不等工具完成，立即出现在 SSE 流 |
| `result_size_truncation` | 工具返回 300k 字符 → 结果截断，写文件，agent 收到路径+预览 |
| `unknown_tool_returns_synthetic_error` | 未注册工具调用 → 返回 is_error=true tool_result，不 panic |
| `interrupt_block_waits_for_tool` | Block 工具执行中用户 ContinueRun → 新消息等待工具完成 |
| `interrupt_cancel_emits_synthetic_result` | Cancel 工具执行中用户 ContinueRun → 工具取消并生成 synthetic interrupted tool_result |
| `discard_drops_late_results` | fallback/discard 后，晚到工具结果不写入当前 transcript |

#### Agent Loop

| 测试名称 | 验证内容 |
|---|---|
| `run_complete_required` | 不调用 run.complete → run 不进入终态 |
| `empty_turn_protection` | 连续 3 次无工具调用 → status=partial |
| `run_complete_promotion_check` | build phase，output_version_id 未 promoted → run.complete 返回 error |
| `run_complete_after_promotion` | promoted 后调用 run.complete → status=completed |
| `max_iterations_partial` | 达到 max_iterations → status=partial，checkpoint 存在 |
| `checkpoint_saved_each_turn` | 每次 turn 前保存 checkpoint |
| `runtime_restart_resumes` | 重启后从 checkpoint 恢复 message window 继续执行 |
| `tool_use_result_invariant_on_model_error` | model stream 在 tool_use 后报错 → runtime 补 missing tool_result |
| `tool_use_result_invariant_on_abort` | CancelRun 时 queued/in-flight tools 均有 tool_result |
| `fallback_discards_old_executor` | model fallback 后旧 executor 被 discard，旧 tool_result 不泄漏到新 attempt |
| `brief_structured_output_schema_enforced` | Brief Agent 输出必须通过 Brief JSON Schema 校验 |

#### Tool Registry / MCP Contract

| 测试名称 | 验证内容 |
|---|---|
| `tool_output_schema_available` | ToolRegistry 可读取 output_schema 并用于结构化输出校验 |
| `input_json_schema_preferred_for_mcp_stub` | MCP stub 提供 input_json_schema 时不从内部 schema 转换 |
| `disabled_tool_not_sent_to_model` | is_enabled=false 的工具不进入 model tool definition |
| `deferred_tool_metadata_recorded` | Deferred tool 不 eager load，但 metadata 写入 run log/audit |
| `mcp_stub_returns_recoverable_error` | 未配置 MCP adapter 时调用 MCP stub → recoverable tool error |

#### Mock BFF Contract

| 测试名称 | 验证内容 |
|---|---|
| `bff_start_run_stream_reconnect` | Mock BFF 使用 shared API types 启动 run，SSE 断线后用 Last-Event-ID 重连且不重复 |
| `bff_build_preview_current_contract` | Mock BFF 启动 build run，收到 preview.updated，并通过 preview-current contract 读取 promoted version |
| `bff_continue_edit_contract` | Mock BFF 通过 ContinueRun 发送编辑消息，收到新 promoted version，preview URL 仍为 project/current |
| `bff_resolve_permission_contract` | Mock BFF resolve permission 后同一 run 恢复 running 并继续发事件 |
| `bff_cancel_contract` | Mock BFF cancel run 后收到 terminal status，事件可重放 |
| `bff_no_product_promote_contract` | Mock BFF client 不包含 PromotePreview 方法；直接调用 HTTP promote 在默认配置下返回 disabled/auth error |

#### Preview Promotion

| 测试名称 | 验证内容 |
|---|---|
| `candidate_no_version_update` | promote 前，`Project.current_version_id` 不变 |
| `promote_updates_version` | promote 后，`Project.current_version_id = versionId` |
| `promote_emits_event` | promote 后，SSE 流收到 `preview.updated` |
| `gate_blocks_on_blocking_finding` | 有 blocking finding → promote 返回 409 |
| `gate_blocks_on_blank_page` | screenshot 全白 → promote 返回 409 |
| `candidate_not_forwarded_to_frontend` | `preview.candidate` 不写入前端可见 ConversationItem |
| `promote_http_disabled_by_default` | `/internal/previews/promote` 默认禁用，生产路径只能走 in-process orchestrator |
| `promote_http_requires_feature_auth_audit` | 测试/admin route 启用时必须具备 feature flag、internal auth 和 audit record |

#### Repair Loop

| 测试名称 | 验证内容 |
|---|---|
| `same_error_max_3_repairs` | 同类型错误 3 次修复失败 → status=partial/blocked |
| `error_dedup_by_type` | 错误类型匹配用 error code，不用含行号的原始字符串 |
| `doom_loop_detection` | 相同 tool + 相同 argv 重复 3 次 → status=blocked |
| `repair_run_parent_id` | repair run 的 `parentRunId` 正确指向 build/review run |
| `finding_status_update` | repair 成功后 finding.status = "fixed" |

### 12.4 端到端测试场景

以下场景在 Phase A 验收时运行，需要真实 K8s + 模型网关：

**E2E-1: Brief → Build → Preview**
```
1. POST /runs (phase=brief, prompt="做一个产品官网")
2. 等待 run.completed
3. POST /runs (phase=build, briefId=...)
4. 等待 preview.updated
5. GET preview URL → HTTP 200，页面内容非空
```

**E2E-2: 权限拦截**
```
1. 触发 Build Agent
2. 通过 mock prompt 注入 fs.read("/etc/passwd")
3. 验证 permission.denied 事件被发出
4. 验证 /etc/passwd 内容未出现在任何 ToolResult
5. 验证 audit record 存在
```

**E2E-3: 修复循环上限**
```
1. 配置 sandbox 使 project.build 始终返回同类型错误
2. 触发 Build Agent
3. 验证 run 在 3 次修复后进入 partial
4. 验证最后一条 ConversationItem 包含中文可操作摘要
5. 验证 Project.current_version_id 保留上一次 promoted 版本
```

**E2E-4: Runtime 重启恢复**
```
1. 触发 Build Agent，在 preview.candidate 阶段强制 kill runtime
2. 重启 runtime
3. 验证 run 从 checkpoint 恢复继续执行
4. 验证最终 preview.updated 正常发出
```

**Phase A.5 E2E-5: Fumadocs Docs 模板**
```
1. POST /runs (phase=brief, content=Markdown 文档集)
2. 确认 Brief recommendedTemplate = fumadocs-docs
3. POST /runs (phase=build, template=fumadocs-docs)
4. 等待 preview.updated
5. GET preview URL → 包含首页、文档页、侧边栏导航
```
