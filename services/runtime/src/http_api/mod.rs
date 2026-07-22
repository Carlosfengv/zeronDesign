mod artifact_presenter;
mod auth;
mod candidate_preview_proxy;
mod composition;
mod contracts;
mod error;
mod profile_support;
mod routes;
mod support;
mod workspace;

pub use crate::runtime::recover_startup_runs;
pub use crate::runtime::RuntimeSupervisor;
use artifact_presenter::*;
use auth::*;
use candidate_preview_proxy::*;
pub use contracts::*;
use error::*;
use profile_support::*;
use support::*;
use workspace::*;

pub(crate) use workspace::effective_workspace_root as resolved_workspace_root;

#[cfg(test)]
use crate::project::ProjectInitWorkspaceTransaction;
#[cfg(test)]
use crate::tools::sandbox::LocalWorkspaceBackend;
#[cfg(test)]
use routes::artifacts::artifact_project_id_from_referer;

use crate::{
    artifact_access::ArtifactAccessService,
    authorization::{ApplicationAuthorizationPolicy, AuthorizationPolicyError},
    config::{PublicPrincipalAuthMode, RuntimeConfig, SandboxBackendMode},
    conversation::RuntimeStore,
    design_profile_service::DesignProfileService,
    model_gateway::{model_client_from_config, ModelClient},
    preview::{promote_preview, PromotionGateReport},
    preview_access::{PreviewAccessContext, PreviewAccessError, PreviewAccessService},
    profiles::build::{run_template_build, TemplateBuildRequest},
    project::resolve_built_in_template_for_init,
    project_asset::{ProjectAssetError, ProjectAssetStore},
    public_principal::{
        PublicPrincipalError, PublicPrincipalVerifier, PREVIEW_READ_OPERATION,
        PROJECT_READ_OPERATION, PROJECT_WRITE_OPERATION, PUBLICATION_READ_OPERATION,
        PUBLICATION_WRITE_OPERATION,
    },
    publish_workflow::PublishWorkflowService,
    release_evidence::{ReleaseEvidenceError, ReleaseEvidenceService},
    run_lifecycle::RunLifecycleService,
    runtime::{
        RuntimeBuildSandboxProvisioner, RuntimeEditWorkspaceRestorer, RuntimeSessionLauncher,
    },
    tools::{
        control_plane::sandbox_backend_for_config,
        runtime::ToolContext,
        sandbox::{SandboxChannelWorkspaceBackend, WorkspaceBackend},
    },
    types::{
        sha256_hex, AgentPhase, AgentRun, Brief, ConversationItem, DesignProfile,
        DesignProfileConversionReport, DesignProfileDraft, DesignProfileFidelityReport,
        DesignProfileValidationIssue, DesignSourceArtifact, ProjectAccessRecord,
        DESIGN_PROFILE_SCHEMA_V2, MAX_DESIGN_SOURCE_BYTES,
    },
    visual_artifact_store::{VisualArtifactStore, MAX_VISUAL_ARTIFACT_INPUT_BYTES},
    visual_contracts::{HistoryItem, RunVisualBinding, RunVisualTarget},
    visual_review::{ScheduleVisualReviewRequest, VisualReviewResult, VisualReviewService},
};
use axum::extract::ws::rejection::WebSocketUpgradeRejection;
use axum::{
    body::Body,
    extract::{ws::WebSocketUpgrade, DefaultBodyLimit, Extension, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::{get, post, put},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{path::PathBuf, sync::Arc};

const MAX_DESIGN_SOURCE_REQUEST_BYTES: usize = 384 * 1024;
const MAX_DESIGN_SOURCE_BASE64_BYTES: usize = MAX_DESIGN_SOURCE_BYTES.div_ceil(3) * 4;
const MAX_VISUAL_ARTIFACT_BASE64_BYTES: usize = MAX_VISUAL_ARTIFACT_INPUT_BYTES.div_ceil(3) * 4;
const MAX_VISUAL_ARTIFACT_REQUEST_BYTES: usize = MAX_VISUAL_ARTIFACT_BASE64_BYTES + 64 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub config: RuntimeConfig,
    pub store: RuntimeStore,
    pub model: Arc<dyn ModelClient>,
    pub supervisor: RuntimeSupervisor,
}

pub fn app_state(config: RuntimeConfig) -> AppState {
    let supervisor = RuntimeSupervisor::new();
    supervisor.mark_recovered();
    app_state_with_supervisor(config, supervisor)
}

pub fn app_state_with_supervisor(config: RuntimeConfig, supervisor: RuntimeSupervisor) -> AppState {
    try_app_state_with_supervisor(config, supervisor)
        .expect("runtime state configuration should be valid")
}

pub fn try_app_state_with_supervisor(
    config: RuntimeConfig,
    supervisor: RuntimeSupervisor,
) -> anyhow::Result<AppState> {
    let model = Arc::new(model_client_from_config(&config)?);
    let store = RuntimeStore::try_with_database_url(
        config.runtime_storage_dir.clone(),
        &config.database_url,
    )?;
    if config.content_plan_approval_producer_required {
        let producer = store.content_plan_approval_store().producer_status();
        if !producer.ready
            || producer.schema_version
                != crate::content_plan_approval::CONTENT_PLAN_APPROVAL_PRODUCER_SCHEMA
            || producer.transaction_schema_version
                != crate::content_plan_approval::CONTENT_PLAN_APPROVAL_TRANSACTION_SCHEMA
        {
            return Err(anyhow::anyhow!(
                "Content Plan Approval producer readiness/schema probe failed"
            ));
        }
    }
    Ok(AppState {
        model,
        store,
        config,
        supervisor,
    })
}

pub async fn recovered_router(config: RuntimeConfig) -> anyhow::Result<Router> {
    let runtime = crate::runtime::RuntimeBootstrap::new(config)
        .recover()
        .await?;
    Ok(router_with_state(runtime.state))
}

pub fn router(config: RuntimeConfig) -> Router {
    router_with_state(app_state(config))
}

pub fn router_with_state(state: AppState) -> Router {
    composition::router_with_services(state)
}

pub fn capture_router_with_state(state: AppState) -> Router {
    composition::capture_router_with_services(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    // remote-fs-boundary: allow-begin startup-recovery-test-fixture
    #[tokio::test]
    async fn startup_recovery_completes_committed_project_init_journal() {
        let root = std::env::temp_dir().join(format!(
            "runtime-project-init-startup-recovery-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut config = RuntimeConfig::from_env();
        config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
        config.runtime_storage_dir = root.join("runtime");
        config.workspace_root = root.join("workspaces");
        let store = RuntimeStore::with_checkpoint_dir(&config.runtime_storage_dir);
        let run = store
            .create_run(
                "startup-recovery-project".to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "fixture".to_string(),
                vec![],
            )
            .await;
        let workspace_root = project_workspace_root(&config, &run.project_id);
        std::fs::create_dir_all(&workspace_root).unwrap();
        let ctx = ToolContext::new(store, run.clone(), workspace_root.clone());
        let mut transaction = ProjectInitWorkspaceTransaction::begin(
            &LocalWorkspaceBackend,
            &ctx,
            &workspace_root.join("project"),
            "next-app",
        )
        .await
        .unwrap();
        transaction
            .mark_workspace_committed(serde_json::json!({
                "projectId": run.project_id,
                "runId": run.id,
                "appRoot": "project",
                "templateKey": "next-app",
                "templateVersion": "next-app@1",
                "templateManifestSha256": "919771231a9745aee050a3280518189d4b8d9f106d6ba334a896f41eac253067",
                "framework": "nextjs",
                "sandboxExecutionProfileId": "next-app",
                "sandboxExecutionProfileVersion": "0.1.0",
                "packageManager": "npm",
                "lockfile": "package-lock.json",
                "registry": "https://registry.npmjs.org/"
            }))
            .await
            .unwrap();
        drop(ctx);

        let recovered = crate::runtime::TestRuntimeBuilder::recover_state(AppState {
            supervisor: RuntimeSupervisor::new(),
            store: RuntimeStore::with_checkpoint_dir(&config.runtime_storage_dir),
            model: Arc::new(crate::model_gateway::EmptyModelClient),
            config: config.clone(),
        })
        .await
        .unwrap()
        .state;
        let state = recovered
            .store
            .get_project_runtime_state("startup-recovery-project")
            .await
            .unwrap();
        assert_eq!(state.template_key, "next-app");
        assert!(!workspace_root
            .join("state/project-init-transactions/startup-recovery-project/journal.json")
            .exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    // remote-fs-boundary: allow-end startup-recovery-test-fixture

    // remote-fs-boundary: allow-begin startup-template-audit-test-fixture
    #[tokio::test]
    async fn startup_rejects_persisted_project_with_missing_historical_template_spec() {
        let root = std::env::temp_dir().join(format!(
            "runtime-template-compatibility-audit-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut config = RuntimeConfig::from_env();
        config.sandbox_backend_mode = SandboxBackendMode::PhaseAContract;
        config.runtime_storage_dir = root.join("runtime");
        config.workspace_root = root.join("workspaces");
        let store = RuntimeStore::with_checkpoint_dir(&config.runtime_storage_dir);
        store
            .upsert_project_runtime_state_with_template_identity(
                "incompatible-project",
                "project".to_string(),
                "next-app".to_string(),
                "next-app@1".to_string(),
                Some(
                    "0000000000000000000000000000000000000000000000000000000000000000".to_string(),
                ),
                "next".to_string(),
                Some("next-app".to_string()),
                Some("0.1.0".to_string()),
                "npm".to_string(),
                "package-lock.json".to_string(),
                "https://registry.npmjs.org/".to_string(),
            )
            .await
            .unwrap();

        let error = match crate::runtime::TestRuntimeBuilder::recover_state(AppState {
            supervisor: RuntimeSupervisor::new(),
            store: RuntimeStore::with_checkpoint_dir(&config.runtime_storage_dir),
            model: Arc::new(crate::model_gateway::EmptyModelClient),
            config,
        })
        .await
        {
            Ok(_) => panic!("startup must reject incompatible persisted template identity"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("persisted project template compatibility audit failed"));
        assert!(error.to_string().contains("template.version_incompatible"));
        std::fs::remove_dir_all(root).unwrap();
    }
    // remote-fs-boundary: allow-end startup-template-audit-test-fixture

    #[test]
    fn artifact_project_id_from_referer_extracts_current_artifact_project() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("http://127.0.0.1:8080/artifacts/project-docs-1/current/docs"),
        );

        assert_eq!(
            artifact_project_id_from_referer(&headers).as_deref(),
            Some("project-docs-1")
        );
    }

    #[test]
    fn artifact_project_id_from_referer_rejects_non_artifact_referer() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::REFERER,
            HeaderValue::from_static("http://127.0.0.1:8080/docs"),
        );

        assert_eq!(artifact_project_id_from_referer(&headers), None);
    }
}
