use anydesign_runtime::{
    config::{ModelProvider, SandboxBackendMode},
    model_gateway::{ModelClient, ModelRequest, ModelResponse, ToolCall},
    types::{AgentPhase, AgentRunStatus},
    visual_artifact_store::VisualArtifactStore,
    visual_contracts::{
        RunVisualBindingRole, RunVisualTarget, VisualReviewMode, VisualReviewStatus, VisualViewport,
    },
    visual_review::{
        FileVisualReviewStore, ScheduleVisualReviewRequest, VisualReviewBindingInput,
        VisualReviewService,
    },
    RuntimeConfig, RuntimeStore,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
struct RecordingModel {
    response: ModelResponse,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

#[async_trait]
impl ModelClient for RecordingModel {
    async fn next_response(&self, request: ModelRequest) -> Result<ModelResponse> {
        self.requests.lock().await.push(request);
        Ok(self.response.clone())
    }
}

#[derive(Clone)]
struct UnavailableModel;

#[async_trait]
impl ModelClient for UnavailableModel {
    async fn next_response(&self, _request: ModelRequest) -> Result<ModelResponse> {
        Err(anyhow!("vision_resource_unavailable"))
    }
}

fn one_pixel_png() -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 1, 1);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[255, 255, 255, 255]).unwrap();
    drop(writer);
    bytes
}

async fn fixture() -> (
    RuntimeConfig,
    RuntimeStore,
    anydesign_runtime::visual_contracts::DraftSnapshot,
    anydesign_runtime::visual_contracts::VisualArtifact,
) {
    let root = std::env::temp_dir().join(format!(
        "runtime-visual-review-{}-{}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let mut config = RuntimeConfig::from_env();
    config.runtime_storage_dir = root.clone();
    config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
    config.model_provider = ModelProvider::InternalGateway;
    let store = RuntimeStore::with_checkpoint_dir(&root);
    let parent = store
        .create_run(
            "project-visual-review".to_string(),
            AgentPhase::Build,
            "builder".to_string(),
            "text-model".to_string(),
            vec![],
        )
        .await;
    let snapshot = store
        .create_draft_snapshot(
            "project-visual-review",
            "runtime://snapshots/project-visual-review/draft".to_string(),
            "a".repeat(64),
            "next-app".to_string(),
            "next-app@1".to_string(),
            "runtime-dependency-policy@1".to_string(),
            "b".repeat(64),
            &parent.id,
            None,
            None,
        )
        .await
        .unwrap();
    let artifact = VisualArtifactStore::open(root.join("visual-artifacts"))
        .unwrap()
        .create_upload(
            "project-visual-review",
            &one_pixel_png(),
            Default::default(),
        )
        .unwrap();
    (config, store, snapshot, artifact)
}

fn request(
    snapshot: &anydesign_runtime::visual_contracts::DraftSnapshot,
    artifact_id: &str,
    mode: VisualReviewMode,
) -> ScheduleVisualReviewRequest {
    ScheduleVisualReviewRequest {
        project_id: "project-visual-review".to_string(),
        mode,
        target: RunVisualTarget::StaticSnapshot {
            snapshot_id: snapshot.snapshot_id.clone(),
            source_hash: snapshot.source_hash.clone(),
        },
        model: "resource:vision-model".to_string(),
        bindings: vec![VisualReviewBindingInput {
            artifact_id: artifact_id.to_string(),
            role: RunVisualBindingRole::Candidate,
            route: "/".to_string(),
            viewport: VisualViewport {
                width: 1440,
                height: 900,
                device_scale_factor: 1.0,
            },
            order: 0,
        }],
    }
}

#[tokio::test]
async fn visual_review_passes_real_artifact_reference_to_an_independent_model_run() {
    let (config, store, snapshot, artifact) = fixture().await;
    let requests = Arc::new(Mutex::new(Vec::new()));
    let model = RecordingModel {
        response: ModelResponse::TextOnly("VISUAL_REVIEW_PASS".to_string()),
        requests: requests.clone(),
    };
    let service = VisualReviewService::new(store.clone(), Arc::new(model), config.clone());
    let result = service
        .schedule(request(&snapshot, &artifact.id, VisualReviewMode::Advisory))
        .await
        .unwrap();

    assert_eq!(result.state.status, VisualReviewStatus::Passed);
    assert!(result.findings.is_empty());
    let model_requests = requests.lock().await;
    assert_eq!(model_requests.len(), 1);
    assert_eq!(model_requests[0].phase, AgentPhase::Review);
    let image = &model_requests[0].messages[0]["content"][2];
    assert_eq!(image["artifactId"], artifact.id);
    assert!(image.get("dataBase64").is_none());
    let review_run_id = result.state.run_id.as_deref().unwrap();
    assert_eq!(
        store.get_run(review_run_id).await.unwrap().status,
        AgentRunStatus::Completed
    );
    assert_eq!(
        FileVisualReviewStore::new(&config.runtime_storage_dir)
            .latest(
                "project-visual-review",
                result.state.target.as_ref().unwrap()
            )
            .unwrap()
            .unwrap()
            .state
            .status,
        VisualReviewStatus::Passed
    );
    std::fs::remove_dir_all(config.runtime_storage_dir).unwrap();
}

#[tokio::test]
async fn unavailable_visual_model_is_explicit_and_does_not_fail_the_parent_generation() {
    let (config, store, snapshot, artifact) = fixture().await;
    let parent = store
        .project_runs("project-visual-review")
        .await
        .unwrap()
        .into_iter()
        .find(|run| run.phase == AgentPhase::Build)
        .unwrap();
    let service =
        VisualReviewService::new(store.clone(), Arc::new(UnavailableModel), config.clone());
    let result = service
        .schedule(request(&snapshot, &artifact.id, VisualReviewMode::Required))
        .await
        .unwrap();

    assert_eq!(result.state.status, VisualReviewStatus::Unavailable);
    assert!(result
        .state
        .reason
        .as_deref()
        .unwrap()
        .contains("vision_resource_unavailable"));
    assert_eq!(
        store.get_run(&parent.id).await.unwrap().status,
        AgentRunStatus::Queued
    );
    assert_eq!(
        store
            .get_run(result.state.run_id.as_deref().unwrap())
            .await
            .unwrap()
            .status,
        AgentRunStatus::Failed
    );
    std::fs::remove_dir_all(config.runtime_storage_dir).unwrap();
}

#[tokio::test]
async fn visual_findings_must_reference_bound_evidence() {
    let (config, store, snapshot, artifact) = fixture().await;
    let model = RecordingModel {
        response: ModelResponse::ToolCalls(vec![ToolCall::new(
            "finding-1",
            "review.report_finding",
            json!({
                "route": "/",
                "viewport": { "width": 1440, "height": 900, "deviceScaleFactor": 1 },
                "category": "spacing",
                "severity": "warning",
                "summary": "Hero spacing is inconsistent",
                "evidenceArtifactIds": [artifact.id],
                "suggestedChange": "Align the hero vertical rhythm to the reference."
            }),
        )]),
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let service = VisualReviewService::new(store, Arc::new(model), config.clone());
    let result = service
        .schedule(request(&snapshot, &artifact.id, VisualReviewMode::Advisory))
        .await
        .unwrap();
    assert_eq!(result.state.status, VisualReviewStatus::Findings);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].evidence_artifact_ids, vec![artifact.id]);
    std::fs::remove_dir_all(config.runtime_storage_dir).unwrap();
}
