use super::*;
use anydesign_runtime::{
    draft_preview::StartDraftPreview,
    types::{AgentEvent, AgentPhase, AgentRunStatus, SandboxBindingStatus, SandboxChannelProtocol},
    visual_artifact_store::VisualArtifactStore,
    visual_contracts::{EditBase, EditImpactOperation, EditImpactRisk, EditImpactScope},
};
use chrono::Utc;
use std::collections::BTreeMap;

fn one_pixel_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[32, 96, 224, 255]).unwrap();
    drop(writer);
    bytes
}

#[tokio::test]
async fn element_observation_and_impact_confirmation_are_revision_bound() {
    let config = phase_a_contract_config();
    let state = http_api::app_state(config.clone());
    let session = state
        .store
        .draft_preview_store()
        .start(StartDraftPreview {
            project_id: "edit-project".to_string(),
            sandbox_binding_id: "binding-edit".to_string(),
            template_id: "next-app".to_string(),
            base_snapshot_id: "snapshot-edit-base".to_string(),
            base_version_id: None,
            proxy_url: "https://runtime.test/previews/lease-edit/".to_string(),
            writer_ttl_seconds: 120,
        })
        .unwrap();
    let artifact = VisualArtifactStore::open(config.runtime_storage_dir.join("visual-artifacts"))
        .unwrap()
        .create_upload("edit-project", &one_pixel_png(), BTreeMap::new())
        .unwrap();
    let app = http_api::router_with_state(state.clone());
    let observation_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/edit-project/element-observations")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "sessionId": session.session_id,
                        "sessionEpoch": session.session_epoch,
                        "workspaceRevision": session.workspace_revision,
                        "route": "/",
                        "viewport": { "width": 1440, "height": 900, "deviceScaleFactor": 1.0 },
                        "domPath": "main > h1",
                        "dataSlot": "hero-title",
                        "accessibleName": "Build better",
                        "visibleTextHash": "a".repeat(64),
                        "boundingBox": { "x": 10.0, "y": 20.0, "width": 500.0, "height": 80.0 },
                        "sourceCandidates": [{ "path": "app/page.tsx", "line": 12, "column": 5, "exportName": "Page", "confidence": 0.9 }],
                        "screenshotCropArtifactId": artifact.id,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(observation_response.status(), StatusCode::OK);
    let observation: Value = serde_json::from_slice(
        &to_bytes(observation_response.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(observation["confidence"], 0.9);
    assert!(!observation["signature"].as_str().unwrap().is_empty());

    let plan_response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/edit-project/edit-impact-plans")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "observationId": observation["observationId"],
                        "scope": "global",
                        "targets": ["app/layout.tsx"],
                        "operations": ["navigation"],
                        "risk": "medium",
                        "editBase": {
                            "kind": "draft",
                            "snapshotId": session.durable_snapshot_id,
                            "sessionId": session.session_id,
                            "expectedSessionEpoch": session.session_epoch,
                            "expectedWorkspaceRevision": session.workspace_revision,
                            "writerLeaseId": session.writer_lease_id,
                        }
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(plan_response.status(), StatusCode::OK);
    let plan: Value = serde_json::from_slice(
        &to_bytes(plan_response.into_body(), 64 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(plan["requiresConfirmation"], true);
    let plan_hash = plan["planHash"].as_str().unwrap();
    let confirmed = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/edit-project/edit-impact-plans/{plan_hash}/confirm"
                ))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(confirmed.status(), StatusCode::OK);

    state
        .store
        .draft_preview_store()
        .commit_revision(
            &session.session_id,
            &session.writer_lease_id,
            session.session_epoch,
            0,
        )
        .unwrap();
    let stale = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/projects/edit-project/element-observations/{}",
                    observation["observationId"].as_str().unwrap()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let stale: Value =
        serde_json::from_slice(&to_bytes(stale.into_body(), 32 * 1024).await.unwrap()).unwrap();
    assert_eq!(stale["errorCode"], "edit.base_stale");
}

#[tokio::test]
async fn confirmed_replacement_plan_automatically_dispatches_replan_successor() {
    let config = phase_a_contract_config();
    let state = http_api::app_state(config);
    let binding = state
        .store
        .create_sandbox_binding(
            "replan-project",
            "sandbox-replan".to_string(),
            "claim-replan".to_string(),
            "workspace-replan".to_string(),
            "pool-replan".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    state
        .store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let session = state
        .store
        .draft_preview_store()
        .start(StartDraftPreview {
            project_id: "replan-project".to_string(),
            sandbox_binding_id: binding.id.clone(),
            template_id: "next-app".to_string(),
            base_snapshot_id: "snapshot-replan-base".to_string(),
            base_version_id: None,
            proxy_url: "https://runtime.test/previews/replan/".to_string(),
            writer_ttl_seconds: 120,
        })
        .unwrap();
    let edit_base = EditBase::Draft {
        snapshot_id: session.durable_snapshot_id.clone(),
        session_id: session.session_id.clone(),
        expected_session_epoch: session.session_epoch,
        expected_workspace_revision: session.workspace_revision,
        writer_lease_id: session.writer_lease_id.clone(),
    };
    let original_plan = state
        .store
        .edit_guard_store()
        .create_plan(
            &state.store.draft_preview_store(),
            anydesign_runtime::edit_guard::CreateEditImpactPlan {
                observation_id: None,
                scope: EditImpactScope::Local,
                targets: vec!["app/layout.tsx".to_string()],
                operations: vec![EditImpactOperation::Copy],
                risk: EditImpactRisk::Low,
                edit_base: edit_base.clone(),
            },
        )
        .unwrap();
    let predecessor = state
        .store
        .create_run(
            "replan-project".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let predecessor = state
        .store
        .set_run_edit_context(&predecessor.id, edit_base.clone(), original_plan.plan_hash)
        .await
        .unwrap();
    state
        .store
        .bind_run_to_sandbox(&predecessor.id, &binding.id)
        .await
        .unwrap();
    state
        .store
        .append_event(AgentEvent::RunWorkflowProgress {
            run_id: predecessor.id.clone(),
            turn: 1,
            stage: "replan_required".to_string(),
            completed_steps: vec!["replan_required".to_string()],
            next_action: json!({ "tool": "orchestrator.create_successor_run" }),
            budgets: json!({}),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
    state
        .store
        .ensure_initial_checkpoint(&predecessor.id)
        .await
        .unwrap();
    state
        .store
        .update_run_status(&predecessor.id, AgentRunStatus::Partial)
        .await
        .unwrap();
    state
        .store
        .append_conversation_item(
            "replan-project",
            Some(&predecessor.id),
            "user_message",
            Some("user"),
            "Update the homepage copy.",
            None,
        )
        .await;

    let app = http_api::router_with_state(state.clone());
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/replan-project/edit-impact-plans")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "predecessorRunId": predecessor.id,
                        "scope": "local",
                        "targets": ["app/page.tsx"],
                        "operations": ["copy"],
                        "risk": "high",
                        "editBase": edit_base,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let replacement: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(replacement["requiresConfirmation"], true);
    assert!(state
        .store
        .get_run(&predecessor.id)
        .await
        .unwrap()
        .successor_run_id
        .is_none());
    let confirmation = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!(
                    "/projects/replan-project/edit-impact-plans/{}/confirm",
                    replacement["planHash"].as_str().unwrap()
                ))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(confirmation.status(), StatusCode::OK);
    let predecessor = state.store.get_run(&predecessor.id).await.unwrap();
    let successor_id = predecessor
        .successor_run_id
        .expect("automatic successor Run");
    let successor = state.store.get_run(&successor_id).await.unwrap();
    assert_eq!(successor.predecessor_run_id, Some(predecessor.id));
    assert_eq!(
        successor.edit_impact_plan_hash.as_deref(),
        replacement["planHash"].as_str()
    );
    assert_ne!(
        successor.edit_impact_plan_hash,
        predecessor.edit_impact_plan_hash
    );
}
