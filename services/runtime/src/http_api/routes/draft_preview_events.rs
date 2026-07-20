use super::super::{
    authorize_project_operation, not_found, AppState, ErrorResponse, PROJECT_READ_OPERATION,
};
use crate::{
    authorization::ApplicationAuthorizationPolicy,
    visual_contracts::{DraftPreviewEvent, DraftPreviewSessionStatus},
};
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
    Router::new().route(
        "/draft-preview-sessions/{session_id}/events",
        get(stream_draft_preview_events),
    )
}

async fn stream_draft_preview_events(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Result<
    Sse<impl futures::Stream<Item = Result<Event, Infallible>>>,
    (StatusCode, Json<ErrorResponse>),
> {
    let store = state.store.draft_preview_store();
    let session = store
        .get(&session_id)
        .ok_or_else(|| not_found(format!("DraftPreviewSession not found: {session_id}")))?;
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &session.project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    let start_after = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.rsplit(':').next())
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let history = store.events(&session_id);
    let history_len = history.len();
    let replay = history
        .into_iter()
        .enumerate()
        .filter_map(|(index, event)| ((index + 1) > start_after).then_some((index + 1, event)))
        .collect::<VecDeque<_>>();
    let stream = stream::unfold(
        DraftPreviewSseState {
            session_id,
            replay,
            live: store.subscribe(&session.session_id),
            sequence: history_len.max(start_after),
            terminal: matches!(
                session.status,
                DraftPreviewSessionStatus::Failed | DraftPreviewSessionStatus::Stopped
            ),
        },
        next_event,
    );
    Ok(Sse::new(stream).keep_alive(KeepAlive::default().text("heartbeat")))
}

struct DraftPreviewSseState {
    session_id: String,
    replay: VecDeque<(usize, DraftPreviewEvent)>,
    live: broadcast::Receiver<DraftPreviewEvent>,
    sequence: usize,
    terminal: bool,
}

async fn next_event(
    mut state: DraftPreviewSseState,
) -> Option<(Result<Event, Infallible>, DraftPreviewSseState)> {
    if let Some((sequence, event)) = state.replay.pop_front() {
        return Some((Ok(encode(&state.session_id, sequence, &event)), state));
    }
    if state.terminal {
        return None;
    }
    let event = match state.live.recv().await {
        Ok(event) => event,
        Err(broadcast::error::RecvError::Lagged(_)) | Err(broadcast::error::RecvError::Closed) => {
            return None
        }
    };
    state.sequence += 1;
    state.terminal = matches!(
        event,
        DraftPreviewEvent::DevFailed { .. } | DraftPreviewEvent::DevStopped { .. }
    );
    Some((Ok(encode(&state.session_id, state.sequence, &event)), state))
}

fn encode(session_id: &str, sequence: usize, event: &DraftPreviewEvent) -> Event {
    Event::default()
        .id(format!("{session_id}:{sequence}"))
        .event(event_name(event))
        .json_data(event)
        .unwrap_or_else(|_| Event::default().event("preview.serialization_error"))
}

fn event_name(event: &DraftPreviewEvent) -> &'static str {
    match event {
        DraftPreviewEvent::DevStarting { .. } => "preview.dev_starting",
        DraftPreviewEvent::DevReady { .. } => "preview.dev_ready",
        DraftPreviewEvent::DevUpdating { .. } => "preview.dev_updating",
        DraftPreviewEvent::DevCompileError { .. } => "preview.dev_compile_error",
        DraftPreviewEvent::DevRestarting { .. } => "preview.dev_restarting",
        DraftPreviewEvent::DevFailed { .. } => "preview.dev_failed",
        DraftPreviewEvent::DevStopped { .. } => "preview.dev_stopped",
        DraftPreviewEvent::SourceRevisionCommitted { .. } => "source.revision_committed",
        DraftPreviewEvent::SourceRevisionDurable { .. } => "source.revision_durable",
        DraftPreviewEvent::SourceSnapshotCreated { .. } => "source.snapshot_created",
    }
}
