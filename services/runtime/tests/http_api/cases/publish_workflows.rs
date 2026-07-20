use super::*;
use anydesign_runtime::artifact_publisher::{ArtifactFile, FileArtifactPublisher};
use sha2::{Digest, Sha256};

fn source_hash(files: &[ArtifactFile]) -> String {
    let mut files = files
        .iter()
        .map(|file| {
            (
                file.path.to_string_lossy().replace('\\', "/"),
                String::from_utf8(file.bytes.clone()).unwrap(),
            )
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    for (path, content) in files {
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        digest.update((content.len() as u64).to_be_bytes());
        digest.update(content.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

fn publish_config(storage: PathBuf) -> RuntimeConfig {
    let mut config = phase_a_contract_config();
    config.runtime_storage_dir = storage;
    config.release_base_image_digest = Some(format!("sha256:{}", "b".repeat(64)));
    config.release_packager_version = Some("packager@1".to_string());
    config.release_registry_repository = Some("registry.example/works".to_string());
    config.release_scan_policy_version = Some("scan@1".to_string());
    config.release_packaging_helper_path = Some(PathBuf::from("/configured/release-packager"));
    config.release_packaging_helper_sha256 = Some("c".repeat(64));
    config.works_base_domain = Some("works.example.test".to_string());
    config
}

#[tokio::test]
async fn publish_workflow_api_is_idempotent_authorized_and_restart_durable() {
    let storage = unique_temp_dir("publish-workflow-http");
    let config = publish_config(storage.clone());
    let state = http_api::app_state(config.clone());
    let run = state
        .store
        .create_run(
            "publish-project".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let files = vec![ArtifactFile {
        path: PathBuf::from("app/page.tsx"),
        bytes: b"export default function Page(){return <main>Publish</main>}\n".to_vec(),
    }];
    let fingerprint = source_hash(&files);
    let source_uri = FileArtifactPublisher::new(&storage)
        .publish_source_snapshot("publish-project", "build-source-1", files)
        .await
        .unwrap();
    let snapshot = state
        .store
        .create_draft_snapshot(
            "publish-project",
            source_uri,
            fingerprint.clone(),
            "next-app".to_string(),
            "next-app@1".to_string(),
            "runtime-dependency-policy@1".to_string(),
            "d".repeat(64),
            &run.id,
            None,
            None,
        )
        .await
        .unwrap();
    let request = json!({
        "source": {
            "kind": "static-snapshot",
            "projectId": "publish-project",
            "snapshotId": snapshot.snapshot_id,
            "expectedSourceHash": fingerprint,
        },
        "idempotencyKey": "publish-click-1",
        "expectedGeneration": 0,
        "visualReviewMode": "advisory",
        "runtimeProfileId": "static-web-v1",
    });
    let app = http_api::router_with_state(state);
    let created = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/publish-project/publish-workflows")
                .header("content-type", "application/json")
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::ACCEPTED);
    let body: Value =
        serde_json::from_slice(&to_bytes(created.into_body(), 64 * 1024).await.unwrap()).unwrap();
    let workflow_id = body["workflow"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["workflow"]["status"], "requested");

    let retry = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/projects/publish-project/publish-workflows")
                .header("content-type", "application/json")
                .body(Body::from(request.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(retry.status(), StatusCode::OK);

    let cancelled = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/publish-workflows/{workflow_id}/cancel"))
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(cancelled.status(), StatusCode::OK);
    let cancelled_body: Value =
        serde_json::from_slice(&to_bytes(cancelled.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(cancelled_body["workflow"]["status"], "cancelled");

    let restarted = http_api::router(config);
    let recovered = restarted
        .oneshot(
            Request::builder()
                .uri(format!("/publish-workflows/{workflow_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(recovered.status(), StatusCode::OK);
    let recovered_body: Value =
        serde_json::from_slice(&to_bytes(recovered.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(recovered_body["workflow"]["id"], workflow_id);
    assert_eq!(recovered_body["workflow"]["status"], "cancelled");

    fs::remove_dir_all(storage).unwrap();
}
