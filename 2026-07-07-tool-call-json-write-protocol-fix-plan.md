---
date: 2026-07-07
status: draft
type: fix-plan
topic: tool-call-json-and-large-write-protocol
evidence:
  - run-167: build run repeatedly placed a full long page into fs.write arguments; tool-call JSON was truncated and surfaced as "fs.write requires path"
related:
  - ./2026-07-07-runtime-harness-fix-plan.md
  - ./2026-07-04-rust-runtime-spec.md
  - /Users/carlos/Downloads/claude-code-main/src/tools/FileWriteTool/prompt.ts
  - /Users/carlos/Downloads/claude-code-main/src/tools/FileEditTool/prompt.ts
  - /Users/carlos/Downloads/claude-code-main/src/services/api/claude.ts
  - /Users/carlos/Downloads/claude-code-main/src/utils/messages.ts
  - /Users/carlos/Downloads/claude-code-main/src/utils/toolResultStorage.ts
---

# Tool-call JSON 与大文件写入协议修复方案

## 1. 结论

`run-167` 的核心问题不是某个模型"不会写页面"，而是当前 harness 允许模型把完整 HTML/CSS 页面作为单个 `fs.write.arguments` JSON 字段发送。页面一长，OpenAI-compatible function call 的 `arguments` 就会变成最脆弱的传输层：一旦模型输出或网关响应在 JSON 中间截断，runtime 只能得到不可解析的字符串，最后被包装成 `{ "arguments": "<raw>" }`，再进入 `fs.write` 校验并报 `fs.write requires path`。

这个错误信息误导性很强。真正失败不是模型忘了传 `path`，而是 `arguments` JSON 已经坏了。继续把同一个错误作为普通工具校验失败反馈给模型，会诱导模型重试更大的 `fs.write`，形成循环。

参考 Claude Code 后，修复方向不是尝试"修复半截 JSON"，而是调整工具协议：

- 大内容不要长期依赖单个 tool-call JSON 参数承载。
- 创建文件和修改文件分流：新建可用 `fs.write`，已有文件必须优先 `fs.patch` / 多 edit。
- 对 `fs.write.text` 设置硬上限，超限时返回明确 recoverable guidance。
- 对 tool-call JSON parse failure 做专门分类，不能退化成 `{ "arguments": raw }` 后继续普通校验。
- P0 就引入最小可用的大文件写入替代路径：`fs.write_chunk` / `fs.commit_chunks`。
- stream 和 SSE 必须暴露真实失败类型：`tool.input_json_parse_failed`、`tool.input_too_large`、`tool.recovery_suggested`。

## 2. Claude Code 的可借鉴机制

Claude Code 对类似问题的处理重点在协议设计，而不是靠模型自觉：

1. `Write` 和 `Edit` 分工具。
   - `Write` 用于新建文件或完整重写。
   - `Edit` 用于既有文件修改，只发送 `old_string/new_string`。
   - `Write` prompt 明确写着：修改已有文件应优先用 `Edit`，因为它只发送 diff。

2. `Edit` prompt 强制小片段。
   - 它要求使用最小唯一 `old_string`，通常 2-4 行。
   - 明确避免 10 行以上上下文。
   - 这能从行为层面抑制"把整页塞进工具参数"。

3. raw stream 自己累积 tool input。
   - Claude Code 使用 raw streaming 累积 `input_json_delta.partial_json`。
   - 注释说明这样避免 SDK 对 partial JSON 做 O(n^2) 解析。
   - 直到 `content_block_stop` 才把完整工具输入交给规范化和 schema 校验。

4. JSON parse failure 单独记录。
   - `normalizeContentFromAPI` 如果解析失败会记录 `tengu_tool_input_json_parse_fail`。
   - 它不会尝试猜测半截 JSON 的语义。
   - 当前项目也不应修补半截页面，而应把它分类成 input transport failure，并要求换写入方式。

5. 大 tool result 落盘，不回灌全文。
   - Claude Code 对 tool result 有 per-tool 和 per-message budget，超过后落盘并给模型 path + preview。
   - 我们已有 tool result truncation，但 run-167 是 tool input 过大，所以还需要补 input 侧协议。

## 3. 当前实现问题

### 3.1 非 streaming OpenAI-compatible 请求

当前 `openai_chat_request` 固定：

```json
{
  "stream": false
}
```

这意味着 runtime 不能观察 `arguments` 是逐步变大的，也无法在中途检测超限或截断，只能等完整 response 返回后解析。

### 3.2 parse fail 被包装成普通 arguments

当前逻辑：

```rust
serde_json::from_str(&call.function.arguments)
    .unwrap_or_else(|_| json!({ "arguments": call.function.arguments }))
```

这会把不可解析 JSON 继续当作一个合法对象传下去。后续 `normalize_tool_input` 无法解开，就进入 `fs.write.validate_input`，最终报缺 `path/text`。这丢失了最重要的诊断：原始 `arguments` 不是合法 JSON。

### 3.3 `fs.write` 没有输入体积限制

`fs.write` schema 是：

```json
{
  "path": "Workspace path",
  "text": "File contents"
}
```

没有 `text` 最大字符数，也没有对页面级内容建议使用 chunk/edit/project writer 的 guidance。模型自然会把整页作为 `text` 一次性输出。

### 3.4 `fs.patch` 已存在但没有被强约束为默认修改路径

`fs.patch` 已经提供 `oldStr/newStr`，并且有歧义检测。但 prompt 和 tool failure recovery 没有把模型从大 `fs.write` 强制引导到 `fs.patch`。

### 3.5 当前 result truncation 只覆盖输出，不覆盖输入

`truncate_large_result_if_needed` 能把超大的 tool result 写到 `outputs/tool-results`，但它发生在工具执行之后。run-167 失败发生在工具执行之前：tool input JSON 已经无法稳定解析。

## 4. 目标状态

P0 完成后，`run-167` 这一类失败应变成以下行为：

1. 模型尝试 `fs.write` 写超长页面。
2. runtime 在执行前检测 `text` 超过阈值，返回 recoverable error：

```text
fs.write input too large for direct tool-call JSON.
Use fs.patch for existing files or fs.write_chunk/fs.commit_chunks for new large files.
Do not retry the same full file in fs.write.
```

3. 如果响应中的 `arguments` JSON 已经截断，runtime 不再报 `fs.write requires path`，而是返回：

```text
tool input JSON parse failed for fs.write; likely truncated while writing a large file.
Retry with chunked write or smaller fs.patch edits.
```

4. build run 不无限循环同类错误。同一 `tool + errorKind + path` 连续 3 次后进入 `partial`，并在 stream 中给出可操作摘要。

## 5. 修复方案

### P0. 防止误诊和死循环

#### P0.1 增加 ToolInputParseError

修改 `services/runtime/src/model_gateway.rs`：

- `OpenAiChatCompletionResponse::into_model_response` 不再把 parse failure 包装成 `{ "arguments": raw }`。
- 新增内部结构，注意这里不保存 raw arguments 原文：

```rust
pub struct ToolInputParseFailure {
    pub tool_call_id: String,
    pub tool_name: String,
    pub raw_len: usize,
    pub raw_sha256: String,
    pub ends_with_json_close: bool,
    pub bracket_balance: i32,
    pub quote_closed: bool,
    pub likely_truncated: bool,
}
```

新增唯一的 runtime 返回路径：

```rust
pub enum ModelResponse {
    ToolCalls(Vec<ToolCall>),
    ToolInputParseFailed {
        parsed_calls: Vec<ToolCall>,
        failures: Vec<ToolInputParseFailure>,
    },
    ...
}
```

对 OpenAI-compatible tool call：

- parse 成功：正常进入 `parsed_calls`。
- parse 失败：进入 `failures`，不生成可执行 `ToolCall`。
- agent loop 必须为每个 failure 追加一条 assistant tool-call transcript 记录，input 只包含 runtime diagnostic stub，不能包含 raw arguments。
- agent loop 必须为每个 failure 追加一条 matching tool result，`recoverable = true`，`errorKind = "tool.input_json_parse_failed"`。
- 不复用当前 `ToolCallsThenError -> emit_missing_tool_results` 路径，因为那条路径语义是"模型流/响应中断导致 tool call 未完成"，当前实现还固定 `recoverable: false`。

疑似截断判断：

- raw 不以 `}` 或 `]` 结尾。
- quote/bracket balance 明显未闭合。
- raw len 接近 provider output cap 或本地 configured cap。
- raw 中包含 `"text"`、`"<html"`、`"---"`、`"<style"`、`"class="` 等大文件内容信号。

安全规则：

- `raw_sha256` 可进入 audit / run health。
- `raw_len`、`ends_with_json_close`、`bracket_balance`、`quote_closed`、`likely_truncated` 可进入 SSE metadata。
- raw prefix 只能进入本地 debug log，且默认关闭；不得写入 transcript、SSE、audit 或模型上下文。

#### P0.2 错误文案改为 transport/schema 分层

当前 `fs.write requires path` 只能说明 schema 缺字段。新增分类：

- `tool.input_json_parse_failed`: tool-call arguments 不是合法 JSON。
- `tool.input_schema_invalid`: JSON 合法，但字段缺失或类型错误。
- `tool.input_too_large`: JSON 合法，但某字段超过工具输入预算。

这些 error kind 要写入：

- tool result metadata
- audit record summary：`decision = deny`，`reason` 必须包含 error kind；`input_summary` 只允许包含 `toolUseId`、长度、hash、budget、truncation flags 等安全诊断字段
- SSE event metadata
- run health report

audit / transcript / SSE 禁止写入以下内容：

- 原始 `arguments` 字符串。
- raw HTML / Astro / Markdown 页面正文。
- 截断前缀、截断后缀或任何可复原用户内容片段。

#### P0.3 `fs.write.text` 输入预算

给 `fs.write` 增加 P0 硬上限：

```text
MAX_DIRECT_WRITE_ARGUMENT_BYTES = 96_000
MAX_DIRECT_WRITE_TEXT_CHARS = 48_000
```

判断方式：

- 对已解析 input 执行 `serde_json::to_vec(&input).len()`，超过 `MAX_DIRECT_WRITE_ARGUMENT_BYTES` 即拒绝。
- 同时检查 `text.chars().count()`，超过 `MAX_DIRECT_WRITE_TEXT_CHARS` 即拒绝。
- 两个条件任意命中都返回 `tool.input_too_large`。

建议 48k / 96k 而不是沿用 tool result 的 200k：

- OpenAI-compatible function call 的 `arguments` 本身要转义，HTML/CSS 中大量引号和换行会放大 serialized bytes。
- 一次 tool call 不应吃掉整个模型输出窗口。
- 页面级内容更适合 chunk 或模板/patch。

超过上限时：

- 不执行写入。
- 返回 recoverable error。
- 明确要求不要重试同一个 `fs.write`。
- 如果目标文件已存在，引导 `fs.patch`。
- 如果目标文件不存在，引导 chunk writer。

#### P0.4 新增最小 chunk writer

P0 必须提供可用替代路径，否则 `tool.input_too_large` 的 guidance 会指向不存在的工具。

新增两个工具：

```json
fs.write_chunk({
  "path": "project/src/pages/index.astro",
  "sessionId": "index-page-v1",
  "index": 0,
  "total": 4,
  "text": "..."
})
```

```json
fs.commit_chunks({
  "path": "project/src/pages/index.astro",
  "sessionId": "index-page-v1",
  "total": 4,
  "sha256": "optional expected final hash"
})
```

P0 只实现最小能力：

- 每个 chunk 最多 `MAX_CHUNK_TEXT_CHARS = 24_000`。
- 每个 chunk serialized input 最多 `MAX_CHUNK_ARGUMENT_BYTES = 48_000`。
- chunk 写入 `outputs/staged-writes/<safe-session-id>/chunk-N.txt`。
- `commit_chunks` 按 `0..total-1` 顺序拼接，写临时文件后原子 rename 到目标 path。
- commit 复用 `checked_write_path`、`ensure_not_nested_package_root` 和 appRoot-aware 检查。
- commit 成功后删除 staged chunks。
- 缺 chunk、重复 chunk、`total` 不一致、hash 不匹配均返回 recoverable error。

#### P0.5 大写入连续失败熔断

在 agent loop 增加 recoverable error fingerprint：

```text
fingerprint = phase + tool + errorKind + normalizedPath
```

规则：

- 同一 fingerprint 连续 2 次：向模型注入 stronger guidance。
- 同一 fingerprint 连续 3 次：停止自动重试，run status = `partial`，保留最近成功 preview。

这与需求文档 R13/R14 的"同一错误最多 3 次自动修复"一致。

### P1. 建立大文件写入协议

#### P1.1 强化 chunk writer

在 P0 最小 chunk writer 基础上增强：

- chunk progress 写入 SSE：`chunk.received`、`chunk.committed`。
- `commit_chunks` 支持 `mode = create | overwrite | append`，默认 `overwrite`。
- sessionId 增加 TTL 清理，防止 staged writes 长期堆积。
- run cancelled / failed 时清理当前 run 的 staged writes。
- run health report 记录 chunk count、final bytes、sha256。

#### P1.2 强化 `fs.patch`

沿用 Claude Code 的 `Edit` 思路，把 `fs.patch` 变成已有文件修改的默认路径：

- prompt 中明确：已有文件修改必须优先 `fs.patch`。
- `fs.patch` 必须建立 read-before-patch 契约：模型需要先 `fs.read` 目标文件或目标 range。
- runtime 可在 P1 记录最近 read state；如果没有读过目标文件，`fs.patch` 返回 recoverable guidance。
- `oldStr` 建议 2-6 行唯一片段。
- 禁止模型用 `fs.write` 完整覆盖已有大文件，除非文件小于预算且用户明确要求重写。
- `fs.patch` 失败时返回：
  - `oldStr not found`: read targeted range and retry.
  - `oldStr found multiple times`: add 2-4 lines context, do not paste whole file.

#### P1.3 新增 `project.write_page` 专用工具

对 website/docs 生成，可进一步提供专用语义工具，减少模型直接操作大源码文件：

```json
project.write_page({
  "route": "/",
  "title": "...",
  "sections": [
    {
      "kind": "hero",
      "heading": "...",
      "body": "...",
      "visual": "..."
    }
  ],
  "styleProfile": "saas"
})
```

runtime 根据模板把结构化内容渲染成 Astro 页面。这个工具不是 P0 必须，但它是长期最稳的产品路径：模型负责内容和结构，harness 负责框架文件和样式骨架。

### P2. 真 streaming 和输入观测

#### P2.1 OpenAI-compatible streaming support

把 `stream: false` 改成可配置：

```text
MODEL_STREAMING=true
```

实现 OpenAI SSE parser：

- 接收 `choices[].delta.tool_calls[].function.arguments`。
- 按 tool call id 累积 arguments。
- 每个 tool call 维护 `input_chars`。
- 超过 `MAX_TOOL_ARGUMENT_CHARS` 时，停止继续累积该 tool call，并返回 recoverable error。

注意：P2 streaming 是增强项，不应阻塞 P0。P0 即使继续非 streaming，也必须能正确分类 parse failure 和 input too large。

#### P2.2 工具输入预算与 schema 一起发给模型

工具 schema 的 description 要包含预算：

```text
text: File contents. Max 48000 chars. For larger files use fs.write_chunk/fs.commit_chunks.
```

如果 provider 支持 strict function calling：

- 通过 `MODEL_STRICT_TOOLS=true` 对 OpenAI-compatible tools 设置 strict schema。
- `additionalProperties: false` 保持开启。
- 但 strict schema 不能替代输入预算，因为合法 JSON 仍可能过大。
- 默认不强开 strict，避免 DeepSeek / Kimi 等 OpenAI-compatible provider 在 schema 支持不完整时拒绝请求。

#### P2.3 大输入遥测

新增指标：

- `tool_input_json_parse_failed`
- `tool_input_too_large`
- `tool_input_retry_same_large_write`
- `tool_chunk_write_started`
- `tool_chunk_write_committed`
- `tool_chunk_write_failed`

SSE 示例：

```text
event: tool.failed
data: {
  "tool": "fs.write",
  "errorKind": "tool.input_too_large",
  "recoverable": true,
  "guidance": "Use fs.write_chunk or fs.patch; do not retry full fs.write."
}
```

## 6. 代码改动清单

### `services/runtime/src/model_gateway.rs`

- 替换 parse failure fallback。
- 新增 `tool_input_json_parse_failed` 分类。
- 给 OpenAI-compatible response 增加 raw arguments size diagnostics。
- 增加测试：
  - invalid arguments JSON 不生成 `{ "arguments": raw }`。
  - invalid arguments JSON 返回 recoverable parse failure。
  - wrapped `{ "arguments": { ... } }` 仍正常解包。

### `services/runtime/src/tools/runtime.rs`

- 扩展 `ValidationError` 或新增 `ToolInputErrorKind`。
- `ToolExecution` 在 validation failure 时保留 kind/metadata。
- audit summary 中加入 error kind，而不是只有 `"object keys=[arguments]"`。
- recoverable `ToolError` 转 tool result 时写 `metadata.recoverable = true`。
- 新增 helper：

```rust
ToolResult::typed_error(message, error_kind, recoverable, metadata)
```

避免每个工具手写不一致的 metadata。

### `services/runtime/src/tools/sandbox.rs`

- `FsWriteTool` 增加 `MAX_DIRECT_WRITE_TEXT_CHARS`。
- `FsWriteTool` 增加 `MAX_DIRECT_WRITE_ARGUMENT_BYTES`，以 serialized input bytes 为主阈值。
- `validate_input` 检测 `text` 长度和 serialized bytes。
- 超限返回 typed validation/recoverable error。
- 注册 `fs.write_chunk` 和 `fs.commit_chunks`。
- chunk commit 复用 `checked_write_path` 和 `ensure_not_nested_package_root`。
- prompt/schema description 更新：已有文件优先 `fs.patch`，大文件使用 chunk。

### `services/runtime/src/agent_loop.rs`

- 记录 recoverable error fingerprint。
- 同类错误 2 次注入强 guidance。
- 同类错误 3 次 partial/block，不再无限循环。
- synthetic `tool.input_json_parse_failed` / `tool.input_too_large` 必须写入 audit record，便于 run-167 类问题复盘；audit 中只保留 `toolUseId`、长度、hash、budget 和 truncation flags，不保留 raw arguments。
- Build/Edit prompt 中加入写入协议：

```text
Never send a full long page through fs.write if it may exceed 48000 chars.
For existing files, use fs.patch with a small unique oldStr.
For new large files, use fs.write_chunk then fs.commit_chunks.
If a tool reports tool.input_json_parse_failed or tool.input_too_large, switch strategy; do not retry the same full fs.write.
```

### `services/runtime/src/types.rs` / event types

- 给 `AgentEvent::ToolFailed` 增加向后兼容的 optional metadata：

```rust
metadata: Option<Value>
```

`metadata` 至少支持：

  - `errorKind`
  - `recoverable`
  - `inputChars`
  - `serializedBytes`
  - `guidance`

如果短期不想改 public event shape，允许先把 metadata 写入 conversation item metadata，但 P0 验收必须保证 `/runs/{runId}/events` 能看到 `errorKind`。

## 7. 测试矩阵

| 层级 | 用例 | 预期 |
|---|---|---|
| Unit | OpenAI tool arguments 为半截 JSON | 返回 `tool_input_json_parse_failed`，不包装成 `{arguments: raw}` |
| Unit | OpenAI wrapped object arguments | 继续解包成功 |
| Unit | `fs.write.text` 低于 48k | 正常写入 |
| Unit | `fs.write.text` 高于 48k | recoverable `tool.input_too_large` |
| Unit | `fs.write` serialized input 高于 96k 但 text 低于 48k | recoverable `tool.input_too_large` |
| Unit | `fs.write_chunk` 缺 chunk | `commit_chunks` recoverable fail |
| Unit | `fs.write_chunk` chunk 超 24k | recoverable fail |
| Unit | `fs.commit_chunks` 成功 | 目标文件内容等于 chunks 拼接 |
| Unit | chunk commit 写 nested package root | 被 `ensure_not_nested_package_root` 拦截 |
| Unit | parse failure event | `tool.failed.metadata.errorKind = tool.input_json_parse_failed` 且不含 raw arguments |
| Unit | parse / too-large synthetic failure audit | audit `reason` 包含 error kind，`input_summary` 只有长度/hash/budget 等安全摘要，不含 raw arguments 或页面正文 |
| Unit | `fs.patch` 未 read-before-patch | recoverable guidance |
| Integration | 模型连续 3 次大 `fs.write` | run 进入 `partial`，不继续循环 |
| Integration | 大页面通过 chunks 写入 | build 成功，preview candidate 可生成 |
| E2E | run-167 design case with real DeepSeek | 不再出现 `fs.write requires path` 循环 |
| E2E | model outputs malformed arguments | stream 显示 parse failure guidance |

## 8. 验收标准

P0 验收：

- `run-167` 类错误不再显示为 `fs.write requires path`。
- SSE 能显示真实原因：`tool.input_json_parse_failed` 或 `tool.input_too_large`。
- audit 能按 `runId/tool/errorKind` 复盘 synthetic tool-input failure，且不泄露 raw arguments 或页面正文。
- parse failure 进入 `ModelResponse::ToolInputParseFailed` 路径，`recoverable = true`，不会复用 missing tool result 的 unrecoverable 路径。
- P0 已存在可调用的 `fs.write_chunk/fs.commit_chunks`，`tool.input_too_large` guidance 不指向不存在的工具。
- 大于 48k 的页面可以通过最小 `fs.write_chunk/fs.commit_chunks` 写入并参与 build。
- 模型不会连续 3 次用同一大 `fs.write` 无限重试。
- 真实 API website 生成即使失败，也能以 `partial` 和中文可操作摘要结束，保留最近成功预览。

P1 验收：

- chunk writer 有进度事件、TTL 清理、cancel/fail 清理和 run health 记录。
- 已有页面修改优先走 `fs.patch`，不再全文件覆盖。
- `fs.patch` 有 read-before-patch 契约，降低旧片段不匹配导致的重试率。
- `cargo test --manifest-path services/runtime/Cargo.toml` 覆盖新增工具和失败分类。

P2 验收：

- OpenAI-compatible streaming 模式可打开。
- runtime 能在 tool arguments 累积阶段检测超限。
- stream 中能看到 chunk 写入进度。

## 9. 推荐实施顺序

1. 改 `model_gateway.rs`：parse failure 不再包装成 `{arguments: raw}`。
2. 新增 `ModelResponse::ToolInputParseFailed`，agent loop 生成 recoverable matching tool result。
3. 给 `AgentEvent::ToolFailed` / tool result 增加 `errorKind` metadata。
4. 给 `fs.write.text` 和 serialized input 增加上限与 clear guidance。
5. 新增 P0 最小 `fs.write_chunk/fs.commit_chunks`。
6. 在 agent loop 增加同类 recoverable error fingerprint 熔断。
7. 更新 Build/Edit prompt，要求 parse failure 或 input too large 后切换策略。
8. 强化 `fs.patch` prompt 和错误 guidance。
9. 跑 mock integration：半截 JSON、大输入、chunk 写入、连续重试。
10. 跑真实 DeepSeek design.md website 用例，对比 run-167 stream。
11. P2 再做 OpenAI-compatible streaming parser。

## 10. 最终判断

这套方案的核心是把"大页面生成"从单次 JSON function call 中解耦出来。Claude Code 的经验说明，成熟 harness 不应把所有文件变更都压到一个通用 `write` 参数里，而应提供更小、更可恢复、更可观察的编辑协议。

完成 P0 后，run-167 的错误会被正确归因，不会再误报 `fs.write requires path`。完成 P1 后，即使模型生成很长页面，也有稳定的大文件写入通道。完成 P2 后，runtime 可以在流式输入阶段提前发现风险，而不是等 provider 返回一个已经坏掉的 JSON。
