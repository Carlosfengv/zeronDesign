---
date: 2026-07-09
status: proposed
type: implementation-guide
topic: runtime-run-events-live-sse
related:
  - ./2026-07-09-run-events-live-sse-plan.md
  - ./2026-07-08-runtime-api-freeze.md
  - ./2026-07-08-project-lifecycle-generation-edit-build-plan.md
---

# Run Events Live SSE 落地方案

## 1. 背景

当前 `GET /runs/{runId}/events` 已经返回 SSE 格式，但语义仍是历史回放：

```rust
let events = state.store.events(&run_id).await;
let stream = stream::iter(events.into_iter().enumerate().filter_map(...));
```

这会导致连接在回放完已有事件后立即关闭。产品层如果要展示实时生成、编辑、构建、预览更新，只能反复轮询该接口，无法用标准 `EventSource` 获得自然的实时体验。

本方案将该接口升级为真正的长连接 SSE：

```text
历史回放 -> 继续等待新事件 -> 空闲 heartbeat -> run.completed 后关闭
```

接口路径、事件 id、事件 payload 均保持兼容。

## 2. 目标

- `GET /runs/{runId}/events` 支持 `Last-Event-ID` 历史补偿。
- 活跃 run 的连接在历史回放后保持打开，并实时推送新事件。
- 空闲期间发送 heartbeat comment，避免代理或浏览器误判连接死亡。
- 收到 terminal `run.completed` 后发送该事件并关闭连接。
- 已完成 run 的历史请求只回放历史事件，然后在 `run.completed` 后正常结束。
- 流关闭以 terminal `run.completed` 事件为事实来源，不能只依赖 run status。
- 不引入 WebSocket，不新增产品侧必须适配的新 endpoint。
- 不改变现有 `AgentEvent` JSON payload 和 `{runId}/{sequence}` id 格式。

## 3. 非目标

- 不实现多副本共享事件总线。
- 不改变 Phase B Runtime API contract 中的事件类型。
- 不新增 snapshot-only 查询参数，除非后续真实产品客户端证明必须保留快照关闭语义。
- 不把事件持久化从现有 run log 迁移到数据库。
- 不在本阶段实现 Product BFF 或 UI。

## 4. 当前实现缺口

### 4.1 HTTP 层

文件：`services/runtime/src/http_api.rs`

当前 `stream_run_events`：

- 验证 run 存在；
- 读取当前内存事件；
- 根据 `Last-Event-ID` 过滤；
- 用 `stream::iter` 输出；
- 输出完立即关闭。

缺少：

- 活跃 run 订阅；
- 历史回放和 live fanout 的衔接；
- heartbeat；
- terminal close；
- broadcast lag 处理。

### 4.2 Store 层

文件：`services/runtime/src/conversation.rs`

当前 `RuntimeStoreInner` 有：

```rust
events: HashMap<String, Vec<AgentEvent>>,
```

`append_event` 负责写 JSONL run log 和内存事件列表，但没有通知正在监听的 SSE 连接。

缺少：

- per-run broadcast channel；
- 事件 sequence 和 in-memory vector push 的一致性；
- terminal 事件后的 broadcaster 清理；
- live 订阅 API。

## 5. 目标架构

```text
Agent loop / tools
  -> RuntimeStore::append_event(event) -> Result<()>
    -> append JSONL run log
    -> hydrate persisted sequence when needed
    -> push in-memory events[runId]
    -> derive sequence under same write lock
    -> broadcast SequencedAgentEvent

GET /runs/{runId}/events
  -> validate run
  -> parse Last-Event-ID
  -> if active: subscribe before reading history
  -> replay stored events after cursor
  -> fan out live broadcast events
  -> heartbeat while idle
  -> close after run.completed
```

关键原则：**先订阅，再读历史**。这样可以避免事件刚好发生在“读完历史”和“开始订阅”之间时丢失。

另一个关键原则：**以持久化事件流为 source of truth**。run status 可以辅助判断是否需要 live fanout，但不能单独决定 SSE stream 是否可以关闭。当前部分路径会先更新 terminal status，再 append terminal `run.completed` 事件；实现必须覆盖这段窗口，避免客户端在 terminal status 已写入但 terminal event 尚未持久化时连接后漏掉最终事件。

## 6. 数据结构变更

### 6.1 新增 sequenced event

文件：`services/runtime/src/conversation.rs`

```rust
#[derive(Debug, Clone)]
pub struct SequencedAgentEvent {
    pub sequence: usize,
    pub event: AgentEvent,
}
```

`sequence` 从 1 开始，与 SSE id 的后半段一致：

```text
id: run-123/1
id: run-123/2
```

### 6.2 RuntimeStoreInner 增加 broadcaster

```rust
const RUN_EVENT_BROADCAST_CAPACITY: usize = 512;

#[derive(Debug, Default)]
struct RuntimeStoreInner {
    events: HashMap<String, Vec<AgentEvent>>,
    event_broadcasters:
        HashMap<String, tokio::sync::broadcast::Sender<SequencedAgentEvent>>,
    ...
}
```

容量 512 的含义：

- 常规 UI 连接不会积压这么多事件；
- 如果消费者落后超过容量，关闭连接，让浏览器用 `Last-Event-ID` 重连并从 run log 补偿；
- 避免无限内存增长。

### 6.3 Sequence 恢复约束

sequence 不能只依赖当前进程内的 `events.len()`。`RuntimeStore::events` 目前在内存为空时会从 run log 回放历史事件；进程重启后，如果某个 run 仍可能继续 append，新进程内存中的 `events` 可能为空，但 run log 已有事件。

因此实现必须满足以下任一策略：

- 在 `append_event` 获取 write lock 后，如果 `inner.events` 尚未包含该 run，先从 run log hydrate 历史事件到内存，再 push 新事件并计算 sequence；
- 或维护一个从 run log 初始化的 per-run `next_event_sequence` / `last_event_sequence`，append 时以该计数器为准。

验收要求：进程恢复后继续 append 的 live event id 不得从 `/1` 重新开始，也不得与 run log 中已有 id 重复。

## 7. Store API 设计

### 7.1 subscribe_events

新增：

```rust
pub async fn subscribe_events(
    &self,
    run_id: &str,
) -> Option<tokio::sync::broadcast::Receiver<SequencedAgentEvent>>
```

行为：

- run 不存在：返回 `None`；
- run 已 terminal 且 terminal `run.completed` 已在持久化事件中：返回 `None`，HTTP 层只做历史回放；
- run 已 terminal 但 terminal `run.completed` 尚未出现在持久化事件中：仍应允许订阅，或由 HTTP 层等待/重读到 terminal event 后再关闭；
- run 活跃：懒创建 broadcaster 并返回 receiver。

是否 terminal 应复用现有 `AgentRunStatus::is_terminal()`，不要新增第二套状态判断：

```rust
status.is_terminal()
```

注意：`status.is_terminal()` 只能说明 run 进入终态，不能说明 `run.completed` 事件已经对 SSE 客户端可见。

### 7.2 append_event 顺序

`RuntimeStore::append_event` 必须保持以下顺序：

1. 从 event 中提取 `run_id`。
2. 写入 JSONL run log，并在失败时返回错误。
3. 获取 store write lock。
4. 如果该 run 的内存事件尚未 hydrate，则从 run log 恢复历史事件或恢复 sequence 计数器。
5. `events.entry(run_id).or_default().push(event.clone())`。
6. 在同一把锁下计算 `sequence = events.len()`。
7. clone 当前 run 的 broadcaster。
8. 如事件是 `RunCompleted`，记录 terminal 后需要清理 broadcaster。
9. 释放锁。
10. broadcast `SequencedAgentEvent { sequence, event }`。
11. terminal broadcast 后移除 broadcaster，或在第 8 步先 `remove` 后仍用 cloned sender 发送 terminal。

持久化必须早于 broadcast。否则客户端收到 live event 后断线，重连时 run log 可能还没有该事件，造成不可恢复的缺口。

持久化失败时不得 broadcast，也不得只写 `eprintln!` 后继续推内存事件。推荐把 `append_event` 改为返回 `Result<()>`，调用方根据错误决定是否 fail run、返回 5xx、或进入可观测的降级路径。测试必须覆盖 run log 写入失败不会向 SSE 客户端广播不可重放事件。

sequence 必须和内存 vector push 在同一把锁内完成。不要在释放锁后重新读长度，否则并发 append 时 SSE id 可能和持久化顺序不一致。

### 7.3 Terminal event 原子性

当前代码存在先 `update_run_status(Completed | Cancelled | ...)`，再 `append_event(AgentEvent::RunCompleted)` 的路径。live SSE 实现时必须消除或覆盖这段竞态窗口。

可选方案：

- 新增 store 方法，例如 `complete_run_with_event(run_id, status, event)`，在同一个 store 语义操作中更新 terminal status 并 append terminal event；
- 或保持 status/event 两步写入，但 `stream_run_events` 对 terminal status 且缺少 terminal event 的 run 不得立即关闭，必须短暂订阅或重读直到看到 `run.completed`；
- 或让 `update_run_status` 不先进入 terminal，统一由 append terminal event 的封装方法完成 terminal transition。

推荐第一种：封装 terminal transition，减少调用方遗漏事件的概率。

## 8. HTTP SSE 设计

### 8.1 stream_run_events 流程

文件：`services/runtime/src/http_api.rs`

目标流程：

1. `get_run(&run_id)` 验证 run 存在。
2. 用现有 `last_event_sequence` 解析 `Last-Event-ID`。
3. 判断 run 当前 status，并检查历史事件中是否已有 terminal `run.completed`。
4. 如果 run 未 terminal，或 run 已 terminal 但历史里尚无 terminal event，先调用 `store.subscribe_events(&run_id)`。
5. 读取 `store.events(&run_id)` 作为历史窗口。
6. 回放 `sequence > start_after` 的历史事件。
7. 如果回放窗口包含 `AgentEvent::RunCompleted`，回放后关闭。
8. 如果 run 已 terminal 且完整历史已包含 terminal event，回放后关闭。
9. 如果 run 未 terminal，或 terminal event 尚未回放，进入 live fanout。
10. live fanout 只发送 `sequence > replayed_max_sequence` 的事件，避免重复。
11. 空闲时发送 heartbeat comment。
12. live 收到 `RunCompleted` 后发送并关闭。
13. 如果 receiver lagged，关闭连接，交给客户端重连补偿。

如果 `subscribe_events` 之后读取历史时发现 terminal `run.completed` 已经出现，必须回放 terminal 后关闭，并丢弃 live receiver。不要继续等待 heartbeat。

### 8.2 Event 编码

业务事件：

```rust
fn encode_sse_event(
    run_id: &str,
    sequence: usize,
    event: &AgentEvent,
) -> Event {
    Event::default()
        .id(format!("{run_id}/{sequence}"))
        .data(serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string()))
}
```

heartbeat：

```rust
Event::default().comment("heartbeat")
```

heartbeat 不带 id，不递增 sequence，客户端不应将它当业务事件解析。

### 8.3 heartbeat 选择

优先使用 Axum SSE keep-alive：

```rust
Sse::new(stream).keep_alive(KeepAlive::default().text("heartbeat"))
```

如果当前 Axum 版本或测试能力不适配，再使用手写 `tokio::time::interval`。两种方式只能选一种，避免重复 heartbeat。

建议参数：

```text
interval: 15s
text: heartbeat
```

测试中不要真实等待 15s。实现需要提供可测试的 heartbeat 间隔配置，或把 stream 组合逻辑抽出来配合 paused Tokio time 推进。CI 中 heartbeat 测试的单次等待应保持在毫秒级 timeout。

### 8.4 Lagged 策略

当 `broadcast::Receiver::recv()` 返回 `Lagged(_)`：

- 立即结束当前 SSE stream；
- 不尝试在当前连接内补齐；
- 浏览器 `EventSource` 会自动携带 last event id 重连；
- 重连后由历史 replay 从 run log 补齐。

这样实现简单、内存有界，并且依赖已经存在的持久化 source of truth。

## 9. 测试方案

测试文件：

- `services/runtime/tests/http_api.rs`
- `services/runtime/tests/mock_bff_contract.rs`

### 9.1 新增 SSE 测试 helper

需要一个增量读取 body frame 的 helper，不能对活跃 run 使用完整 `to_bytes(response.into_body(), ...)`。

helper 能力：

- 在 timeout 内读取前 N 个业务事件；
- 识别 SSE `id:`、`data:`、comment heartbeat；
- 判断 stream 是否在 terminal 后结束；
- 对活跃 stream 验证短时间内没有提前 EOF。

现有测试工具需要拆成两类：

- `read_terminal_sse_body`：只用于已 terminal run 的 historical replay，可以完整读取 body；
- `read_live_sse_frames`：用于 active run，按 frame 增量读取，所有等待必须带 timeout。

`mock_bff_contract.rs` 里现有 `get_sse()` 也使用完整 `to_bytes` 读取 body。实现 live SSE 时必须同步迁移或限制它只接收 terminal run，否则 mock BFF 合约测试会在 active stream 场景挂起。

### 9.2 必测用例

#### stream_events_keeps_connection_open_for_running_run

流程：

- 创建 running run；
- append `run.started`；
- 请求 `/runs/{runId}/events`；
- 读到历史事件；
- 在短 timeout 内确认 stream 没有结束。

验收：

- 连接不再是快照式立即关闭。

#### stream_events_replays_then_fans_out_without_duplicates

流程：

- append 事件 1、2、3；
- 用 `Last-Event-ID: run/2` 连接；
- 期望先收到事件 3；
- 连接保持打开；
- append 事件 4；
- 期望收到事件 4 且只收到一次；
- append `RunCompleted`；
- 期望收到 terminal 后关闭。

验收：

- replay 和 live 衔接无丢失、无重复。

#### stream_events_closes_after_terminal_run_completed

流程：

- 对 running run 打开 SSE；
- append 普通事件；
- append `RunCompleted`；
- 验证两个事件都收到；
- 验证 stream 结束。

验收：

- terminal live event 是最后一个业务事件。

#### stream_events_replay_terminal_run_closes_without_live_subscription

流程：

- 创建 run，append 普通事件和 `RunCompleted`；
- run 已 terminal 后再连接；
- 验证历史回放包含 `RunCompleted`；
- 验证连接结束，不等待 heartbeat。

验收：

- 完成态历史回放兼容老客户端和测试。

#### stream_events_sends_heartbeat_comments

流程：

- 打开 active run SSE；
- 等待或推进 heartbeat interval；
- 验证出现 `: heartbeat`；
- 验证 heartbeat 没有 id。

验收：

- 空闲连接有保活帧；
- heartbeat 不污染业务事件序列。

#### stream_events_reconnect_replays_from_run_log

流程：

- 使用 `RuntimeStore::with_checkpoint_dir`；
- append 多个事件；
- 重新构造 store；
- 使用 `Last-Event-ID` 连接；
- 验证从持久化 run log 回放缺失事件。

验收：

- live 断线后的补偿依赖持久化，而不是内存 broadcaster。

#### stream_events_recovered_active_run_uses_next_persisted_sequence

流程：

- 使用 `RuntimeStore::with_checkpoint_dir`；
- append 事件 1、2、3；
- 重新构造 store；
- 对同一 run append 第 4 个事件；
- 连接 `/runs/{runId}/events`；
- 验证第 4 个事件 id 是 `{runId}/4`。

验收：

- 进程恢复后 sequence 不重置。

#### stream_events_terminal_status_without_terminal_event_does_not_close_early

流程：

- 创建 run；
- 将 run status 更新为 terminal，但暂不 append `RunCompleted`；
- 请求 `/runs/{runId}/events`；
- 验证连接不会因为 terminal status 立即空回放并关闭；
- append `RunCompleted`；
- 验证客户端收到 terminal event 后关闭。

验收：

- terminal close 以 `run.completed` 事件为准，不单独以 run status 为准。

#### append_event_does_not_broadcast_when_run_log_append_fails

流程：

- 使用不可写 run log 目录或可注入失败的 store fixture；
- 建立 live SSE 订阅；
- 调用 `append_event`；
- 验证返回错误；
- 验证 SSE 客户端未收到不可持久化事件。

验收：

- persist-before-broadcast 是强语义，不只是实现顺序注释。

## 10. 实施步骤

建议一个 PR 或一个 commit 完成，commit message：

```text
feat(runtime): stream run events with live SSE fanout
```

执行顺序：

1. 在 `conversation.rs` 增加 `SequencedAgentEvent` 和 `RUN_EVENT_BROADCAST_CAPACITY`。
2. 给 `RuntimeStoreInner` 增加 `event_broadcasters`。
3. 复用现有 `AgentRunStatus::is_terminal()`，不要新增重复 terminal 状态判断。
4. 增加 helper：判断历史事件窗口是否包含 `AgentEvent::RunCompleted`。
5. 实现 `RuntimeStore::subscribe_events`，覆盖 terminal status 但缺 terminal event 的窗口。
6. 修改 `RuntimeStore::append_event` 返回 `Result<()>`，保证 persist -> hydrate/sequence -> push -> broadcast 顺序。
7. 增加或封装 terminal transition API，避免 status 先 terminal、event 后 append 的竞态。
8. 修改 `http_api.rs::stream_run_events` 为 replay + live stream。
9. 加 heartbeat，优先 Axum keep-alive，并提供测试可控间隔。
10. 在 `http_api.rs` 和 `mock_bff_contract.rs` 测试中拆分 terminal/full-body 与 live/incremental SSE helper。
11. 添加第 9 节列出的测试。
12. 跑格式化和 focused tests。
13. 跑 runtime local gate。
14. 如有 provider key，跑 provider gate 并保存 evidence。

## 11. 验证命令

基础验证：

```bash
cargo fmt --manifest-path services/runtime/Cargo.toml -- --check
cargo test --manifest-path services/runtime/Cargo.toml --test http_api -- --nocapture
cargo test --manifest-path services/runtime/Cargo.toml --test mock_bff_contract -- --nocapture
npm test --prefix packages/shared
npm run typecheck --prefix packages/shared
```

完整本地 gate：

```bash
bash services/runtime/scripts/run-runtime-harness-local-gates.sh
```

provider gate：

```bash
DEEPSEEK_API_KEY=... \
RUNTIME_E2E_REQUIRE_COMPUTED_STYLE=1 \
bash services/runtime/scripts/run-runtime-harness-provider-gates.sh
```

## 12. 验收标准

代码层：

- `stream_run_events` 不再使用单纯的 `stream::iter(events)` 快照输出。
- active run 的 SSE 请求在历史回放后保持打开。
- append 新事件时，已连接客户端能收到 live event。
- `Last-Event-ID` 后的 replay 不重复、不跳号。
- 进程恢复后继续 append 的 event id 延续 run log sequence。
- terminal `run.completed` 会关闭 live 和 historical stream。
- terminal status 已写入但 `run.completed` 尚未持久化时，SSE 不会提前关闭并漏掉 terminal event。
- run log 持久化失败时不会 broadcast 不可重放事件。
- heartbeat comment 不带业务 id。
- broadcast lag 会关闭连接并允许客户端重连补偿。

测试层：

- 新增 live SSE 测试全部通过。
- 既有 `http_api`、`mock_bff_contract`、shared schema 测试通过。
- 所有 active stream 测试都使用增量 frame helper 和 timeout，不使用完整 `to_bytes` 读取。
- local gate 通过。

产品契约层：

- Phase B 可以直接使用浏览器 `EventSource`。
- BFF 可以透传 runtime SSE，不需要轮询模拟实时。
- 现有事件 schema 不变。
- 预览切换仍以 `preview.updated` 为准。

## 13. 兼容性与风险

### 13.1 快照客户端行为变化

旧客户端如果依赖“请求结束表示当前没有更多事件”，升级后 active run 请求会保持打开。

处理策略：

- 当前产品目标是 live semantics，不为假设客户端提前加 `?snapshot=1`。
- 如果后续发现真实客户端依赖旧语义，再新增显式 snapshot endpoint 或 query flag。

### 13.2 broadcaster 内存占用

每个 active run 最多一个 sender，容量固定 512。

处理策略：

- terminal 后清理 broadcaster；
- lagged receiver 关闭连接；
- 不为 terminal run 创建 broadcaster。

### 13.3 多副本部署

当前方案是单进程 in-memory fanout。多副本下，客户端可能连到没有产生事件的 runtime 实例。

处理策略：

- 本阶段标记为 out of scope；
- 多副本时引入 Redis pub/sub、Postgres `LISTEN/NOTIFY` 或 NATS；
- replay source 仍可保留 run log 或迁移到共享持久层。

### 13.4 测试挂起

活跃 SSE 是长连接，测试如果继续全量读 body 会挂住。

处理策略：

- active stream 测试只做增量读取；
- 所有等待都加 timeout；
- full-body read 只用于已经 terminal 的 historical replay。

### 13.5 Terminal status/event 竞态

如果 status 已经 terminal，但 `run.completed` 事件尚未 append，SSE 连接不能空回放后关闭。

处理策略：

- 优先用 store 封装方法同时完成 terminal transition 和 terminal event append；
- HTTP 层仍要防御历史缺 terminal event 的窗口；
- 测试必须人工构造该窗口。

### 13.6 持久化失败导致 live-only 事件

如果 run log 写入失败但 live broadcast 成功，客户端断线后无法 replay 该事件。

处理策略：

- `append_event` 返回 `Result<()>`；
- run log append 失败时不 push 内存、不 broadcast；
- 调用方必须暴露错误或进入可观测降级路径。

## 14. 回滚方案

如果 live SSE 上线后出现阻塞或测试不稳定：

1. 保留 `SequencedAgentEvent` 和 broadcaster 代码可以不回滚；
2. 将 `stream_run_events` 临时切回历史 replay；
3. 保留 `append_event -> Result<()>` 的 persist-before-broadcast 语义，不回退到 live-only 内存事件；
4. 保留新增测试但先标记 ignored，直到修复 stream helper；
5. 不改变 shared contract，避免连带影响 Phase B。

回滚不应删除 run log replay 能力，也不应改变事件 payload。

## 15. 后续工作

live SSE 通过后，下一步进入 Phase B：

- `apps/web/lib/runtime-client.ts` 使用 `packages/shared` 类型封装 runtime API；
- BFF events route 透传 runtime SSE；
- 前端使用 `EventSource`；
- timeline 根据 `AgentEvent` 更新；
- preview panel 只在 `preview.updated` 后切换 current preview。

DesignProfile 仍应后置。当前优先级是让产品工作台能稳定观察真实 runtime 生命周期。
