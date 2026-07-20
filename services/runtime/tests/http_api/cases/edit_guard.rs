use super::*;
use anydesign_runtime::{
    draft_preview::StartDraftPreview, visual_artifact_store::VisualArtifactStore,
};
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
