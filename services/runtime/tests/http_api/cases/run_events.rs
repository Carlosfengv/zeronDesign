use super::*;

#[tokio::test]
async fn stream_events_reconnect_uses_last_event_id_without_duplicates() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .append_event(anydesign_runtime::types::AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "first".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    store
        .append_event(anydesign_runtime::types::AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "second".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    store
        .append_event(anydesign_runtime::types::AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "third".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    store
        .append_event(anydesign_runtime::types::AgentEvent::RunCompleted {
            run_id: run.id.clone(),
            status: "completed".to_string(),
            summary: "done".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: RuntimeConfig::from_env(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}/events", run.id))
                .header("last-event-id", format!("{}/2", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body =
        String::from_utf8(to_bytes(response.into_body(), 8192).await.unwrap().to_vec()).unwrap();
    assert!(!body.contains("first"));
    assert!(!body.contains("second"));
    assert!(body.contains("third"));
    assert!(body.contains(&format!("id: {}/3", run.id)));
}

#[tokio::test]
async fn stream_events_replays_then_fans_out_without_duplicates() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    for text in ["first", "second", "third"] {
        store
            .append_event(AgentEvent::AgentMessage {
                run_id: run.id.clone(),
                text: text.to_string(),
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();
    }
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}/events", run.id))
                .header("last-event-id", format!("{}/2", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    let replay = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected replay event")
        .expect("expected replay frame")
        .expect("replay frame should be valid");
    let replay = String::from_utf8(replay.to_vec()).unwrap();
    assert!(replay.contains("third"));
    assert!(replay.contains(&format!("id: {}/3", run.id)));

    store
        .append_event(AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "fourth".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let live = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected live event")
        .expect("expected live frame")
        .expect("live frame should be valid");
    let live = String::from_utf8(live.to_vec()).unwrap();
    assert!(live.contains("fourth"));
    assert!(live.contains(&format!("id: {}/4", run.id)));

    store
        .append_event(AgentEvent::RunCompleted {
            run_id: run.id.clone(),
            status: "completed".to_string(),
            summary: "done".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let terminal = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected terminal event")
        .expect("expected terminal frame")
        .expect("terminal frame should be valid");
    let terminal = String::from_utf8(terminal.to_vec()).unwrap();
    assert!(terminal.contains("run.completed"));
    assert!(terminal.contains(&format!("id: {}/5", run.id)));

    let end = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("terminal stream should close");
    assert!(end.is_none());
}

#[tokio::test]
async fn stream_events_recovered_active_run_uses_next_persisted_sequence() {
    let checkpoint_dir = unique_temp_dir("http-sse-recovered-sequence");
    let store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    for text in ["first", "second", "third"] {
        store
            .append_event(AgentEvent::AgentMessage {
                run_id: run.id.clone(),
                text: text.to_string(),
                timestamp: chrono::Utc::now(),
            })
            .await
            .unwrap();
    }

    let recovered_store = RuntimeStore::with_checkpoint_dir(&checkpoint_dir);
    recovered_store
        .append_event(AgentEvent::AgentMessage {
            run_id: run.id.clone(),
            text: "fourth".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: RuntimeConfig::from_env(),
        store: recovered_store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}/events", run.id))
                .header("last-event-id", format!("{}/3", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let frame = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected recovered replay frame")
        .expect("expected body frame")
        .expect("frame should be valid");
    let frame = String::from_utf8(frame.to_vec()).unwrap();
    assert!(frame.contains("fourth"));
    assert!(frame.contains(&format!("id: {}/4", run.id)));
}

#[tokio::test]
async fn stream_events_terminal_status_without_terminal_event_waits_for_terminal_event() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .update_run_status(&run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}/events", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    assert!(
        timeout(Duration::from_millis(100), body.next())
            .await
            .is_err(),
        "terminal status without run.completed should not close the stream before terminal event"
    );

    store
        .append_event(AgentEvent::RunCompleted {
            run_id: run.id.clone(),
            status: "completed".to_string(),
            summary: "done".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await
        .unwrap();
    let terminal = timeout(Duration::from_secs(1), body.next())
        .await
        .expect("expected terminal event")
        .expect("expected body frame")
        .expect("frame should be valid");
    let terminal = String::from_utf8(terminal.to_vec()).unwrap();
    assert!(terminal.contains("run.completed"));
    assert!(terminal.contains(&format!("id: {}/1", run.id)));
}

#[tokio::test]
async fn append_event_does_not_broadcast_when_run_log_append_fails() {
    let checkpoint_dir = unique_temp_dir("http-sse-bad-checkpoints");
    let storage_parent = unique_temp_dir("http-sse-bad-run-log-parent");
    let run_log_file = storage_parent.join("not-a-directory");
    fs::write(&run_log_file, "occupied").unwrap();
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_file);
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Export,
            "export".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: RuntimeConfig::from_env(),
        store: store.clone(),
        model: Arc::new(MockModelClient::new(vec![])),
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}/events", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    let result = store
        .append_event(AgentEvent::AgentMessage {
            run_id: run.id,
            text: "should not broadcast".to_string(),
            timestamp: chrono::Utc::now(),
        })
        .await;
    assert!(result.is_err());
    let maybe_frame = timeout(Duration::from_millis(100), body.next()).await;
    assert!(
        maybe_frame.is_err() || maybe_frame.unwrap().is_none(),
        "SSE stream should not receive an event that failed durable append"
    );
}

#[tokio::test]
async fn stream_events_rejects_unknown_run() {
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: RuntimeConfig::from_env(),
        store: RuntimeStore::new(),
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .oneshot(
            Request::builder()
                .uri("/runs/run-missing/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["error"], "run not found: run-missing");
}

#[tokio::test]
async fn project_conversation_returns_user_visible_items_by_default() {
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            "project-1".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    store
        .append_conversation_item(
            "project-1",
            Some(&run.id),
            "assistant_message",
            Some("assistant"),
            "Brief is ready.",
            None,
        )
        .await;
    store
        .append_conversation_item_with_visibility(
            "project-1",
            Some(&run.id),
            "tool_summary",
            Some("system"),
            "Debug-only tool detail",
            None,
            "debug",
        )
        .await;
    let app = http_api::router_with_state(AppState {
        supervisor: http_api::RuntimeSupervisor::new(),
        config: RuntimeConfig::from_env(),
        store,
        model: Arc::new(MockModelClient::new(vec![])),
    });

    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/conversation")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["projectId"], "project-1");
    assert_eq!(payload["items"].as_array().unwrap().len(), 1);
    assert_eq!(payload["items"][0]["text"], "Brief is ready.");

    let response = app
        .oneshot(
            Request::builder()
                .uri("/projects/project-1/conversation?includeDebug=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), 4096).await.unwrap();
    let payload: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(payload["items"].as_array().unwrap().len(), 2);
    assert!(payload["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["visibility"] == "debug"));
}
