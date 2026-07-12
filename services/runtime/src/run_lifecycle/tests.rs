use super::{
    BuildSandboxProvisioner, EditWorkspaceRestorer, RunLifecycleService, RunSessionLauncher,
};
use crate::{
    config::RuntimeConfig,
    conversation::RuntimeStore,
    types::{AgentPhase, AgentRunStatus, SandboxBindingStatus, SandboxChannelProtocol},
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};

struct UnusedSandboxProvisioner;

#[async_trait::async_trait]
impl BuildSandboxProvisioner for UnusedSandboxProvisioner {
    async fn provision_ready(
        &self,
        _store: &RuntimeStore,
        _project_id: &str,
        _template_key: &str,
    ) -> anyhow::Result<crate::types::SandboxBinding> {
        unreachable!("test does not provision a sandbox")
    }
}

#[derive(Default)]
struct FailingSandboxProvisioner {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl BuildSandboxProvisioner for FailingSandboxProvisioner {
    async fn provision_ready(
        &self,
        _store: &RuntimeStore,
        _project_id: &str,
        _template_key: &str,
    ) -> anyhow::Result<crate::types::SandboxBinding> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(anyhow::anyhow!("injected sandbox claim failure"))
    }
}

struct UnusedEditWorkspaceRestorer;

#[async_trait::async_trait]
impl EditWorkspaceRestorer for UnusedEditWorkspaceRestorer {
    async fn restore(
        &self,
        _store: &RuntimeStore,
        _config: &RuntimeConfig,
        _run: &crate::types::AgentRun,
        _source_snapshot_uri: &str,
    ) -> anyhow::Result<()> {
        unreachable!("test does not restore an edit workspace")
    }
}

#[derive(Default)]
struct FailingEditWorkspaceRestorer {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl EditWorkspaceRestorer for FailingEditWorkspaceRestorer {
    async fn restore(
        &self,
        _store: &RuntimeStore,
        _config: &RuntimeConfig,
        _run: &crate::types::AgentRun,
        _source_snapshot_uri: &str,
    ) -> anyhow::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(anyhow::anyhow!("injected edit restore failure"))
    }
}

#[derive(Default)]
struct RecordingSessionLauncher {
    launched: Mutex<Vec<String>>,
}

#[derive(Default)]
struct FailingSessionLauncher {
    attempted: Mutex<Vec<String>>,
}

impl RunSessionLauncher for FailingSessionLauncher {
    fn launch(&self, run_id: String) -> anyhow::Result<()> {
        self.attempted.lock().unwrap().push(run_id);
        Err(anyhow::anyhow!("injected session registration failure"))
    }
}

impl RunSessionLauncher for RecordingSessionLauncher {
    fn launch(&self, run_id: String) -> anyhow::Result<()> {
        self.launched.lock().unwrap().push(run_id);
        Ok(())
    }
}

#[tokio::test]
async fn continue_run_uses_injected_session_launcher() {
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
        .update_run_status(&run.id, AgentRunStatus::NeedsUserInput)
        .await
        .unwrap();
    let launcher = Arc::new(RecordingSessionLauncher::default());
    let service = RunLifecycleService::new(
        RuntimeConfig::from_env(),
        store.clone(),
        launcher.clone(),
        Arc::new(UnusedSandboxProvisioner),
        Arc::new(UnusedEditWorkspaceRestorer),
        crate::design_profile_service::DesignProfileService::new(store.clone()),
    );

    let outcome = service
        .continue_run(&run.id, "Continue".to_string())
        .await
        .unwrap();

    assert_eq!(outcome.status, "running");
    assert_eq!(launcher.launched.lock().unwrap().as_slice(), [run.id]);
}

#[tokio::test]
async fn start_build_cancels_created_run_when_sandbox_provisioning_fails() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-claim-failure".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief_draft(
            &brief_run.id,
            crate::types::Brief {
                project_type: "website".to_string(),
                audience: "operators".to_string(),
                content_hierarchy: vec!["hero".to_string()],
                page_structure: serde_json::json!([]),
                visual_direction: "clear".to_string(),
                recommended_template: "astro-website".to_string(),
                assumptions: vec![],
                missing_information: vec![],
            },
        )
        .await
        .unwrap();
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let provisioner = Arc::new(FailingSandboxProvisioner::default());
    let service = RunLifecycleService::new(
        RuntimeConfig::from_env(),
        store.clone(),
        Arc::new(RecordingSessionLauncher::default()),
        provisioner.clone(),
        Arc::new(UnusedEditWorkspaceRestorer),
        crate::design_profile_service::DesignProfileService::new(store.clone()),
    );

    let error = service
        .start(super::StartRunCommand {
            project_id: "project-claim-failure".to_string(),
            phase: AgentPhase::Build,
            agent_profile: "build".to_string(),
            input_context: super::StartRunContext {
                brief_id: Some(brief_id),
                ..Default::default()
            },
        })
        .await
        .unwrap_err();

    assert_eq!(provisioner.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(error, super::RunLifecycleError::Conflict(_)));
    assert!(store
        .active_mutable_run_for_project("project-claim-failure")
        .await
        .is_none());
}

#[tokio::test]
async fn start_edit_cancels_created_run_when_workspace_restore_fails() {
    let store = RuntimeStore::new();
    let build_run = store
        .create_run(
            "project-restore-failure".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let binding = store
        .create_sandbox_binding(
            "project-restore-failure",
            "sandbox-restore-failure".to_string(),
            "claim-restore-failure".to_string(),
            "workspace-restore-failure".to_string(),
            "pool-restore-failure".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let version = store
        .create_project_version_candidate(
            "project-restore-failure",
            &build_run.id,
            "http://preview.invalid".to_string(),
            None,
            Some("runtime://source-snapshots/project-restore-failure/base".to_string()),
        )
        .await;
    store
        .promote_project_version("project-restore-failure", &build_run.id, &version.id)
        .await
        .unwrap();
    store
        .update_run_status(&build_run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
    let restorer = Arc::new(FailingEditWorkspaceRestorer::default());
    let service = RunLifecycleService::new(
        RuntimeConfig::from_env(),
        store.clone(),
        Arc::new(RecordingSessionLauncher::default()),
        Arc::new(UnusedSandboxProvisioner),
        restorer.clone(),
        crate::design_profile_service::DesignProfileService::new(store.clone()),
    );

    let error = service
        .start(super::StartRunCommand {
            project_id: "project-restore-failure".to_string(),
            phase: AgentPhase::Edit,
            agent_profile: "edit".to_string(),
            input_context: super::StartRunContext {
                base_version_id: Some(version.id),
                sandbox_binding_id: Some(binding.id),
                ..Default::default()
            },
        })
        .await
        .unwrap_err();

    assert_eq!(restorer.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(error, super::RunLifecycleError::Conflict(_)));
    assert!(store
        .active_mutable_run_for_project("project-restore-failure")
        .await
        .is_none());
}

#[tokio::test]
async fn start_keeps_queued_run_recoverable_when_session_registration_fails() {
    let store = RuntimeStore::new();
    let launcher = Arc::new(FailingSessionLauncher::default());
    let service = RunLifecycleService::new(
        RuntimeConfig::from_env(),
        store.clone(),
        launcher.clone(),
        Arc::new(UnusedSandboxProvisioner),
        Arc::new(UnusedEditWorkspaceRestorer),
        crate::design_profile_service::DesignProfileService::new(store.clone()),
    );

    let error = service
        .start(super::StartRunCommand {
            project_id: "project-session-failure".to_string(),
            phase: AgentPhase::Brief,
            agent_profile: "brief".to_string(),
            input_context: Default::default(),
        })
        .await
        .unwrap_err();

    assert!(matches!(error, super::RunLifecycleError::Internal(_)));
    let run_id = launcher.attempted.lock().unwrap()[0].clone();
    assert_eq!(
        store.get_run(&run_id).await.unwrap().status,
        AgentRunStatus::Queued
    );
    assert!(store.events(&run_id).await.iter().any(|event| matches!(
        event,
        crate::types::AgentEvent::StateChanged { state, .. }
            if state == "queued:session_registration_failed"
    )));
}
