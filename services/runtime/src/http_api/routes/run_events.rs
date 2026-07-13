use super::super::{
    authorize_project_operation, last_event_sequence, not_found, AppState, ErrorResponse,
    PROJECT_READ_OPERATION,
};
use crate::authorization::ApplicationAuthorizationPolicy;
use crate::{conversation::SequencedAgentEvent, types::AgentEvent};
use axum::{
    extract::{Extension, Path, State},
    http::{HeaderMap, StatusCode},
    response::sse::{Event, KeepAlive, Sse},
    routing::get,
    Json, Router,
};
use futures::stream;
use std::{collections::VecDeque, convert::Infallible};
use tokio::sync::broadcast;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new().route("/runs/{run_id}/events", get(stream_run_events))
}

async fn stream_run_events(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(run_id): Path<String>,
    headers: HeaderMap,
) -> Result<
    Sse<impl futures::Stream<Item = Result<Event, Infallible>>>,
    (StatusCode, Json<ErrorResponse>),
> {
    let run = state
        .store
        .get_run(&run_id)
        .await
        .ok_or_else(|| not_found(format!("run not found: {run_id}")))?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &run.project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let start_after = last_event_sequence(
        headers
            .get("last-event-id")
            .and_then(|value| value.to_str().ok()),
        &run_id,
    );
    let live_events = state.store.subscribe_events(&run_id).await;
    let events = state.store.events(&run_id).await;
    let history_len = events.len();
    let replay_events = events
        .into_iter()
        .enumerate()
        .filter_map(move |(index, event)| {
            let sequence = index + 1;
            (sequence > start_after).then_some(SequencedAgentEvent { sequence, event })
        })
        .collect::<VecDeque<_>>();
    let stream = stream::unfold(
        RunEventsSseState {
            run_id,
            replay_events,
            live_events,
            min_live_sequence: history_len.max(start_after),
            finished: false,
        },
        next_run_event_sse,
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::default().text("heartbeat")))
}

struct RunEventsSseState {
    run_id: String,
    replay_events: VecDeque<SequencedAgentEvent>,
    live_events: Option<broadcast::Receiver<SequencedAgentEvent>>,
    min_live_sequence: usize,
    finished: bool,
}

async fn next_run_event_sse(
    mut state: RunEventsSseState,
) -> Option<(Result<Event, Infallible>, RunEventsSseState)> {
    loop {
        if state.finished {
            return None;
        }
        if let Some(sequenced) = state.replay_events.pop_front() {
            let is_terminal = sequenced.event.is_run_completed();
            let event = encode_run_event_sse(&state.run_id, sequenced.sequence, &sequenced.event);
            if is_terminal {
                state.finished = true;
                state.live_events = None;
            }
            return Some((Ok(event), state));
        }
        let receiver = state.live_events.as_mut()?;
        let sequenced = match receiver.recv().await {
            Ok(sequenced) => sequenced,
            Err(broadcast::error::RecvError::Lagged(_))
            | Err(broadcast::error::RecvError::Closed) => return None,
        };
        if sequenced.sequence <= state.min_live_sequence {
            continue;
        }
        state.min_live_sequence = sequenced.sequence;
        let is_terminal = sequenced.event.is_run_completed();
        let event = encode_run_event_sse(&state.run_id, sequenced.sequence, &sequenced.event);
        if is_terminal {
            state.finished = true;
            state.live_events = None;
        }
        return Some((Ok(event), state));
    }
}

fn encode_run_event_sse(run_id: &str, sequence: usize, event: &AgentEvent) -> Event {
    Event::default()
        .id(format!("{run_id}/{sequence}"))
        .data(serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string()))
}
