# Run Events Live SSE Upgrade Plan

## 1. Goal

Upgrade `GET /runs/{runId}/events` from a snapshot-style SSE response into a
long-lived SSE stream with:

- historical replay from `Last-Event-ID`;
- live event fanout after the replay window;
- periodic heartbeat comments;
- terminal-event close;
- compatibility with the current SSE URL and event id format.

The endpoint should remain a one-way event stream. Do not replace it with
WebSocket for this phase.

## 2. Current Behavior

Current implementation in `services/runtime/src/http_api.rs`:

```rust
let events = state.store.events(&run_id).await;
let stream = stream::iter(events.into_iter().enumerate().filter_map(...));
```

This means the endpoint:

- reads the current stored events;
- emits them as SSE messages;
- closes as soon as the iterator is exhausted.

This is technically SSE-formatted output, but product behavior is snapshot
replay, not a live stream. Consumers must poll the endpoint to simulate realtime
progress.

Current persistence in `RuntimeStore::append_event`:

- writes event JSONL to the run log;
- pushes the event into the in-memory `events` map;
- does not notify active `/events` connections.

## 3. Target Behavior

New behavior:

```text
GET /runs/{runId}/events
  -> validate run exists
  -> subscribe to live event broadcast if run is not already terminal
  -> read stored historical events
  -> replay events after Last-Event-ID
  -> close immediately if replay includes terminal run.completed
  -> stream newly appended events for active runs
  -> send heartbeat comments while idle
  -> close after terminal run.completed
```

The endpoint keeps the same URL and continues to use ids like:

```text
id: run-28/74
```

`Last-Event-ID: run-28/50` should replay event 51 and later.

## 4. Runtime Store Design

Add a sequenced event wrapper:

```rust
#[derive(Debug, Clone)]
pub struct SequencedAgentEvent {
    pub sequence: usize,
    pub event: AgentEvent,
}
```

Add per-run broadcast channels to `RuntimeStoreInner`:

```rust
event_broadcasters: HashMap<String, tokio::sync::broadcast::Sender<SequencedAgentEvent>>
```

Recommended broadcast capacity:

```rust
const RUN_EVENT_BROADCAST_CAPACITY: usize = 512;
```

### append_event Ordering

`RuntimeStore::append_event` should preserve this order:

1. derive `run_id` from the event;
2. append the event to JSONL run log;
3. acquire the store write lock;
4. push the event into the in-memory `events` vector;
5. compute `sequence = events.len()` while still holding that write lock;
6. clone the per-run broadcaster while still holding that write lock;
7. release the write lock;
8. broadcast `SequencedAgentEvent { sequence, event }`.

Persist-before-broadcast is important. If a client receives a live event and then
disconnects, reconnect replay must be able to recover the same event from
durable storage.

The sequence must be derived from the same lock-protected vector mutation that
stores the event. Do not compute the sequence from a separate read after
releasing the lock; concurrent appends could otherwise produce ids that do not
match persisted order.

### Store API

Add:

```rust
pub async fn subscribe_events(
    &self,
    run_id: &str,
) -> tokio::sync::broadcast::Receiver<SequencedAgentEvent>
```

If a broadcaster does not exist for an active run, create it lazily. This allows
running or recovered-but-still-active runs with no new events yet to be
subscribed.

Do not create a broadcaster just to serve a terminal run. Terminal runs should be
served from replay and closed after the terminal event. When `append_event`
broadcasts `RunCompleted`, the store may remove the run broadcaster after
sending the terminal event because no more live events should arrive for that
run.

## 5. SSE Endpoint Design

Implementation outline for `stream_run_events`:

1. validate the run exists;
2. parse `Last-Event-ID` with existing `last_event_sequence`;
3. read the current run status;
4. if the run is active, subscribe to `store.subscribe_events(&run_id)` before
   reading history;
5. read historical events with `store.events(&run_id)`;
6. replay historical events with `sequence > start_after`;
7. close immediately if the replayed window includes `AgentEvent::RunCompleted`;
8. close immediately after replay if the run status is already terminal;
9. remember `replayed_max_sequence`;
10. stream live broadcast events with `sequence > replayed_max_sequence`;
11. send heartbeat comments while idle;
12. close after terminal `AgentEvent::RunCompleted`.

Subscribing before reading history avoids the race where an event is appended
between "read historical events" and "subscribe live receiver".

Terminal replay is a first-class case, not an edge case. Existing tests and
older clients often request events after a run has already completed. Those
requests must still finish after replaying the terminal event instead of
entering an idle live stream that can never receive another terminal event.

### Event Encoding

Business events:

```rust
Event::default()
    .id(format!("{run_id}/{sequence}"))
    .data(serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string()))
```

Heartbeat:

```rust
Event::default().comment("heartbeat")
```

Heartbeat comments must not increment event sequence and must not be parsed by
clients as business events.

Prefer Axum's SSE keep-alive support for heartbeat comments if the current Axum
version exposes it cleanly:

```rust
Sse::new(stream).keep_alive(KeepAlive::default().text("heartbeat"))
```

If manual heartbeat is needed for tests or version compatibility, implement it
inside the stream with a `tokio::time::interval`. Do not use both mechanisms at
the same time, otherwise tests and browser traces will see duplicate heartbeat
frames.

### Terminal Close

Close the stream after sending:

```rust
AgentEvent::RunCompleted { status, .. }
```

Terminal statuses:

- `completed`
- `partial`
- `blocked`
- `failed`
- `cancelled`

The current runtime emits terminal run state through `run.completed`, so this is
the natural close signal.

For a historical request, the terminal close condition applies during replay as
well as during live fanout. A completed run should replay through
`run.completed` and then close, even when no live broadcaster exists.

### Lag Handling

If `broadcast::Receiver` returns `Lagged(_)`, close the SSE connection. The
browser/client can reconnect with `Last-Event-ID`, and replay will recover the
missed events from the run log.

This keeps memory bounded and avoids trying to rebuild missed events from the
broadcast queue.

## 6. Client Contract

Frontend should use `EventSource`:

```ts
const source = new EventSource(`/runs/${runId}/events`);

source.onmessage = (message) => {
  const event = JSON.parse(message.data);
  // update run timeline
};

source.onerror = () => {
  // Browser will retry automatically.
};
```

Browser-managed reconnect will send the last received SSE id as `Last-Event-ID`.
The UI can also store `message.lastEventId` if it wants an explicit cursor for
diagnostics or manual reconnect logic.

## 7. Test Plan

Add tests in `services/runtime/tests/http_api.rs`.

Because `/runs/{runId}/events` becomes a long-lived response for active runs,
new tests must not use `to_bytes(response.into_body(), ...)` for non-terminal
streams. Add a small SSE test helper that reads body frames incrementally and
uses `tokio::time::timeout` to collect either:

- the first N business events;
- one heartbeat comment;
- or the stream end after a terminal event.

Keep full-body reads only for completed historical runs where the replayed
`run.completed` event should close the stream.

### 7.1 Keeps Connection Open

Test name:

```rust
stream_events_keeps_connection_open_for_running_run
```

Flow:

- create a running run;
- request `/runs/{runId}/events`;
- read only the first replayed event or frame with a timeout;
- verify the stream does not terminate before a terminal event;
- avoid reading the full body.

### 7.2 Replay Then Live Fanout

Test name:

```rust
stream_events_replays_then_fans_out_without_duplicates
```

Flow:

- append events 1, 2, 3;
- connect with `Last-Event-ID: run/2`;
- expect replay of event 3;
- append event 4 while the connection is open;
- expect event 4 exactly once;
- append `RunCompleted`;
- verify the stream closes after `RunCompleted`.

### 7.3 Terminal Close

Test name:

```rust
stream_events_closes_after_terminal_run_completed
```

Flow:

- open SSE for a non-terminal run;
- append a normal event;
- append `RunCompleted`;
- verify both events are emitted;
- verify stream ends after `RunCompleted`.

Also add a historical variant:

```rust
stream_events_replay_terminal_run_closes_without_live_subscription
```

Flow:

- append normal events and `RunCompleted`;
- connect after the run is already terminal;
- verify replay includes `RunCompleted`;
- verify the response body ends without waiting for heartbeat or live fanout.

### 7.4 Heartbeat

Test name:

```rust
stream_events_sends_heartbeat_comments
```

Flow:

- use paused tokio time if practical;
- open SSE;
- advance heartbeat interval;
- verify a heartbeat comment is emitted;
- verify heartbeat does not create a business event id.

This test should assert the chosen heartbeat mechanism, either Axum keep-alive
or manual interval. It should not allow both mechanisms to produce duplicate
heartbeat comments.

### 7.5 Reconnect From Persistent Run Log

Test name:

```rust
stream_events_reconnect_replays_from_run_log
```

Flow:

- use `RuntimeStore::with_checkpoint_dir`;
- append events;
- recreate the store from the same checkpoint/storage dir;
- connect with `Last-Event-ID`;
- verify replay comes from persisted run log.

## 8. Implementation Steps

Recommended commit:

```text
feat(runtime): stream run events with live SSE fanout
```

Steps:

1. add `SequencedAgentEvent` and broadcast map to `conversation.rs`;
2. add `RuntimeStore::subscribe_events` for active runs;
3. update `RuntimeStore::append_event` to persist first, derive sequence under
   the store write lock, then broadcast after releasing the lock;
4. remove the per-run broadcaster after terminal `RunCompleted` fanout;
5. replace `stream::iter(...)` in `stream_run_events` with a replay + live stream;
6. close during replay when `run.completed` is encountered;
7. close after replay when the run is already terminal;
8. add exactly one heartbeat mechanism;
9. add HTTP tests with incremental body reads and timeouts;
10. update shared/API docs if a public contract doc exists for event streaming.

## 9. Rollout Notes

This change should be backward compatible for callers that already use
`EventSource` or parse SSE messages.

Polling clients will still work, but each request may stay open until terminal
completion. If any existing client expects the request to close immediately after
snapshot replay, update that client to either:

- use `EventSource` normally; or
- request a future explicit snapshot endpoint if one is needed.

Do not add query flags like `?live=1` unless a current product client truly
depends on snapshot-close behavior. The product target is live stream semantics.

## 10. Multi-Instance Caveat

The proposed fanout is in-memory. It works for the current local runtime and
single-process harness.

If runtime is deployed with multiple replicas, live fanout needs a shared event
bus, for example:

- Redis pub/sub;
- Postgres `LISTEN/NOTIFY`;
- NATS;
- another runtime event bus.

The durable replay source can remain the run log, but active live subscribers
must receive events from the same shared channel across replicas.

This multi-instance fanout is out of scope for the first implementation unless
the deployment topology already requires horizontal runtime replicas.
