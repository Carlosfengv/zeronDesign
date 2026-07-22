use super::{
    BuildSandboxProvisioner, EditWorkspaceRestorer, RunLifecycleService, RunSessionLauncher,
};
use crate::{
    config::{ContentPlanAttestationMode, GenerationContextMode, RuntimeConfig},
    conversation::RuntimeStore,
    design_context::{
        compile_website_design_context, DesignContextCompileOptions,
        VerificationEnvironmentBinding, VerifierCapability, VerifierRegistry,
        VERIFIER_REGISTRY_VERSION,
    },
    draft_preview::StartDraftPreview,
    templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry},
    types::{
        AgentEvent, AgentPhase, AgentRunStatus, Brief, DesignProfile, EffectiveDesignProfile,
        SandboxBindingStatus, SandboxChannelProtocol,
    },
    visual_contracts::{EditBase, EditImpactOperation, EditImpactRisk, EditImpactScope},
};

#[tokio::test]
async fn enforced_build_rejects_missing_content_plan_before_creating_a_run() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-content-plan-enforce".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief_draft(
            &brief_run.id,
            Brief {
                project_type: "website".to_string(),
                audience: "operators".to_string(),
                content_hierarchy: vec!["hero".to_string()],
                page_structure: serde_json::json!([]),
                visual_direction: "clear".to_string(),
                recommended_template: "next-app".to_string(),
                assumptions: vec![],
                missing_information: vec![],
            },
        )
        .await
        .unwrap();
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let mut config = RuntimeConfig::from_env();
    config.generation_context_mode = GenerationContextMode::Enabled;
    config.content_plan_attestation_mode = ContentPlanAttestationMode::Enforce;
    let service = RunLifecycleService::new(
        config,
        store.clone(),
        Arc::new(RecordingSessionLauncher::default()),
        Arc::new(UnusedSandboxProvisioner),
        Arc::new(UnusedEditWorkspaceRestorer),
        crate::design_profile_service::DesignProfileService::new(store.clone()),
    );

    let error = service
        .start(super::StartRunCommand {
            project_id: "project-content-plan-enforce".to_string(),
            phase: AgentPhase::Build,
            agent_profile: "build".to_string(),
            input_context: super::StartRunContext {
                brief_id: Some(brief_id),
                ..Default::default()
            },
        })
        .await
        .unwrap_err();

    assert!(
        matches!(error, super::RunLifecycleError::Conflict(message) if message.contains("content_plan.approval_rejected"))
    );
    let runs = store
        .project_runs("project-content-plan-enforce")
        .await
        .unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].phase, AgentPhase::Brief);
}
use chrono::Utc;
use std::{
    collections::BTreeMap,
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

#[tokio::test]
async fn replan_successor_lineage_survives_runtime_restart() {
    let temp = std::env::temp_dir().join(format!(
        "zerondesign-replan-lineage-{}-{}",
        std::process::id(),
        Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    fs::create_dir_all(&temp).unwrap();
    let checkpoint_dir = temp.join("checkpoints");
    let run_log_dir = temp.join("run-log");
    let store = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    let predecessor = store
        .create_run(
            "project-replan-restart".to_string(),
            AgentPhase::Repair,
            "repair".to_string(),
            "internal-balanced".to_string(),
            Vec::new(),
        )
        .await;
    store
        .append_event(AgentEvent::RunWorkflowProgress {
            run_id: predecessor.id.clone(),
            turn: 1,
            stage: "replan_required".to_string(),
            completed_steps: vec!["replan_required".to_string()],
            next_action: serde_json::json!({ "tool": "orchestrator.create_successor_run" }),
            budgets: serde_json::json!({}),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
    store
        .ensure_initial_checkpoint(&predecessor.id)
        .await
        .unwrap();
    store
        .update_run_status(&predecessor.id, AgentRunStatus::Partial)
        .await
        .unwrap();
    let successor = store
        .create_replan_successor_run_with_context(
            &predecessor.id,
            predecessor.project_id.clone(),
            AgentPhase::Repair,
            "repair".to_string(),
            "internal-balanced".to_string(),
            Vec::new(),
            None,
            None,
        )
        .await
        .unwrap();
    drop(store);

    let reloaded = RuntimeStore::with_storage_dirs(&checkpoint_dir, &run_log_dir);
    assert_eq!(
        reloaded
            .get_run(&predecessor.id)
            .await
            .unwrap()
            .successor_run_id
            .as_deref(),
        Some(successor.id.as_str())
    );
    assert_eq!(
        reloaded
            .get_run(&successor.id)
            .await
            .unwrap()
            .predecessor_run_id
            .as_deref(),
        Some(predecessor.id.as_str())
    );
    drop(reloaded);
    fs::remove_dir_all(temp).unwrap();
}

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
    async fn prepare_build(
        &self,
        _store: &RuntimeStore,
        _config: &RuntimeConfig,
        _run: &crate::types::AgentRun,
    ) -> anyhow::Result<()> {
        Ok(())
    }

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
    brief_versions: Mutex<Vec<Option<String>>>,
}

#[async_trait::async_trait]
impl EditWorkspaceRestorer for FailingEditWorkspaceRestorer {
    async fn prepare_build(
        &self,
        _store: &RuntimeStore,
        _config: &RuntimeConfig,
        _run: &crate::types::AgentRun,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn restore(
        &self,
        _store: &RuntimeStore,
        _config: &RuntimeConfig,
        _run: &crate::types::AgentRun,
        _source_snapshot_uri: &str,
    ) -> anyhow::Result<()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.brief_versions
            .lock()
            .unwrap()
            .push(_run.brief_version.clone());
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
    assert_eq!(
        launcher.launched.lock().unwrap().as_slice(),
        [run.id.clone()]
    );
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
                recommended_template: "next-app".to_string(),
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
async fn start_edit_rehydrates_a_released_project_before_restoring_workspace() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-edit-rehydrate".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief_draft(
            &brief_run.id,
            Brief {
                project_type: "website".to_string(),
                audience: "operators".to_string(),
                content_hierarchy: vec!["hero".to_string()],
                page_structure: serde_json::json!([]),
                visual_direction: "clear".to_string(),
                recommended_template: "next-app".to_string(),
                assumptions: vec![],
                missing_information: vec![],
            },
        )
        .await
        .unwrap();
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let build_run = store
        .create_run_with_context(
            "project-edit-rehydrate".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
            Some(brief_id),
            None,
        )
        .await;
    let version = store
        .create_project_version_candidate(
            "project-edit-rehydrate",
            &build_run.id,
            "http://preview.invalid".to_string(),
            None,
            Some("runtime://source-snapshots/project-edit-rehydrate/base".to_string()),
        )
        .await;
    store
        .promote_project_version("project-edit-rehydrate", &build_run.id, &version.id)
        .await
        .unwrap();
    store
        .update_run_status(&build_run.id, AgentRunStatus::Completed)
        .await
        .unwrap();
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
            project_id: "project-edit-rehydrate".to_string(),
            phase: AgentPhase::Edit,
            agent_profile: "edit".to_string(),
            input_context: super::StartRunContext {
                base_version_id: Some(version.id),
                ..Default::default()
            },
        })
        .await
        .unwrap_err();

    assert_eq!(provisioner.calls.load(Ordering::SeqCst), 1);
    assert!(matches!(error, super::RunLifecycleError::Conflict(_)));
    assert!(store
        .active_mutable_run_for_project("project-edit-rehydrate")
        .await
        .is_none());
}

#[tokio::test]
async fn start_edit_cancels_created_run_when_workspace_restore_fails() {
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            "project-restore-failure".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let brief_id = store
        .write_brief_draft(
            &brief_run.id,
            Brief {
                project_type: "website".to_string(),
                audience: "operators".to_string(),
                content_hierarchy: vec!["hero".to_string()],
                page_structure: serde_json::json!([]),
                visual_direction: "clear".to_string(),
                recommended_template: "next-app".to_string(),
                assumptions: vec![],
                missing_information: vec![],
            },
        )
        .await
        .unwrap();
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let build_run = store
        .create_run_with_context(
            "project-restore-failure".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
            Some(brief_id.clone()),
            None,
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
    assert_eq!(
        restorer.brief_versions.lock().unwrap().as_slice(),
        [Some(brief_id)]
    );
    assert!(matches!(error, super::RunLifecycleError::Conflict(_)));
    assert!(store
        .active_mutable_run_for_project("project-restore-failure")
        .await
        .is_none());
}

#[tokio::test]
async fn start_draft_edit_freezes_revision_and_consumes_confirmation_at_first_mutation() {
    let store = RuntimeStore::new();
    let binding = store
        .create_sandbox_binding(
            "project-draft-edit",
            "sandbox-draft-edit".to_string(),
            "claim-draft-edit".to_string(),
            "workspace-draft-edit".to_string(),
            "pool-draft-edit".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let session = store
        .draft_preview_store()
        .start(StartDraftPreview {
            project_id: "project-draft-edit".to_string(),
            sandbox_binding_id: binding.id.clone(),
            template_id: "next-app".to_string(),
            base_snapshot_id: "snapshot-draft-edit".to_string(),
            base_version_id: None,
            proxy_url: "https://runtime.test/previews/draft-edit/".to_string(),
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
    let plan = store
        .edit_guard_store()
        .create_plan(
            &store.draft_preview_store(),
            crate::edit_guard::CreateEditImpactPlan {
                observation_id: None,
                scope: EditImpactScope::Global,
                targets: vec!["app/layout.tsx".to_string()],
                operations: vec![EditImpactOperation::Navigation],
                risk: EditImpactRisk::Medium,
                edit_base: edit_base.clone(),
            },
        )
        .unwrap();
    assert!(plan.requires_confirmation);
    store
        .edit_guard_store()
        .confirm(&store.draft_preview_store(), &plan.plan_hash)
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
        .start(super::StartRunCommand {
            project_id: "project-draft-edit".to_string(),
            phase: AgentPhase::Edit,
            agent_profile: "edit".to_string(),
            input_context: super::StartRunContext {
                edit_base: Some(edit_base.clone()),
                edit_impact_plan_hash: Some(plan.plan_hash.clone()),
                sandbox_binding_id: Some(binding.id),
                ..Default::default()
            },
        })
        .await
        .unwrap();

    assert_eq!(outcome.status, "queued");
    let run = store.get_run(&outcome.run_id).await.unwrap();
    assert_eq!(run.edit_base, Some(edit_base));
    assert_eq!(
        run.edit_impact_plan_hash.as_deref(),
        Some(plan.plan_hash.as_str())
    );
    assert!(launcher.launched.lock().unwrap().is_empty());
    assert!(store
        .edit_guard_store()
        .validate_executable(&store.draft_preview_store(), &plan.plan_hash)
        .is_ok());
    let scope_error = store
        .preflight_edit_mutation(&run.id, Some("app/outside-plan.tsx"))
        .await
        .unwrap_err();
    assert!(scope_error
        .to_string()
        .contains("edit.plan_scope_violation"));
    assert!(store
        .edit_guard_store()
        .validate_executable(&store.draft_preview_store(), &plan.plan_hash)
        .is_ok());
    let run = store
        .preflight_edit_mutation(&run.id, Some("app/layout.tsx"))
        .await
        .unwrap();
    assert!(run.edit_mutation_preflight_completed);
    assert!(store
        .edit_guard_store()
        .validate_executable(&store.draft_preview_store(), &plan.plan_hash)
        .is_err());
    let later_scope_error = store
        .preflight_edit_mutation(&run.id, Some("app/outside-after-first-mutation.tsx"))
        .await
        .unwrap_err();
    assert!(later_scope_error
        .to_string()
        .contains("edit.plan_scope_violation"));

    let replacement_plan = store
        .edit_guard_store()
        .create_plan(
            &store.draft_preview_store(),
            crate::edit_guard::CreateEditImpactPlan {
                observation_id: None,
                scope: EditImpactScope::Local,
                targets: vec!["app/page.tsx".to_string()],
                operations: vec![EditImpactOperation::Copy],
                risk: EditImpactRisk::Low,
                edit_base: run.edit_base.clone().unwrap(),
            },
        )
        .unwrap();
    assert!(!replacement_plan.requires_confirmation);
    store
        .append_event(AgentEvent::RunWorkflowProgress {
            run_id: run.id.clone(),
            turn: 1,
            stage: "replan_required".to_string(),
            completed_steps: vec!["replan_required".to_string()],
            next_action: serde_json::json!({ "tool": "orchestrator.create_successor_run" }),
            budgets: serde_json::json!({}),
            timestamp: Utc::now(),
        })
        .await
        .unwrap();
    store
        .update_run_status(&run.id, AgentRunStatus::Partial)
        .await
        .unwrap();
    store
        .append_conversation_item(
            &run.project_id,
            Some(&run.id),
            "user_message",
            Some("user"),
            "Update the homepage copy.",
            None,
        )
        .await;
    let successor = service
        .dispatch_replan_successor(&run.id, &replacement_plan.plan_hash)
        .await
        .unwrap();
    assert_eq!(successor.status, "running");
    let repair = store.get_run(&successor.run_id).await.unwrap();
    assert_eq!(
        repair.edit_impact_plan_hash.as_deref(),
        Some(replacement_plan.plan_hash.as_str())
    );
    assert_ne!(repair.edit_impact_plan_hash, run.edit_impact_plan_hash);
    assert_eq!(repair.predecessor_run_id.as_deref(), Some(run.id.as_str()));
    assert_eq!(
        store
            .get_run(&run.id)
            .await
            .unwrap()
            .successor_run_id
            .as_deref(),
        Some(repair.id.as_str())
    );
    assert_eq!(
        launcher.launched.lock().unwrap().as_slice(),
        [repair.id.clone()]
    );
    let idempotent = service
        .dispatch_replan_successor(&run.id, &replacement_plan.plan_hash)
        .await
        .unwrap();
    assert_eq!(idempotent.run_id, repair.id);
    let duplicate = store
        .create_replan_successor_run_with_context(
            &run.id,
            run.project_id.clone(),
            AgentPhase::Edit,
            "edit".to_string(),
            "internal-balanced".to_string(),
            Vec::new(),
            run.brief_version.clone(),
            None,
        )
        .await
        .unwrap_err();
    assert!(duplicate
        .to_string()
        .contains("already has a successor Run"));
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
    assert!(
        store.latest_checkpoint_for_run(&run_id).await.is_some(),
        "queued run must have a recovery checkpoint before session registration"
    );
    assert!(store.events(&run_id).await.iter().any(|event| matches!(
        event,
        crate::types::AgentEvent::StateChanged { state, .. }
            if state == "queued:session_registration_failed"
    )));
}

#[test]
fn enforced_dcp_rejects_an_unavailable_verifier_before_session_launch() {
    let template = BuiltInTemplateRegistry::built_in()
        .current(&TemplateId::parse("next-app").unwrap())
        .unwrap();
    let profile = EffectiveDesignProfile {
        design_profile_id: "profile-enforced".to_string(),
        version: 1,
        surface: "website".to_string(),
        template: "next-app".to_string(),
        base_profile_hash: "a".repeat(64),
        surface_override_hash: None,
        template_override_hash: None,
        effective_profile_hash: "b".repeat(64),
        profile: serde_json::json!({
            "runtimeTokenMapping": { "color.primary": "#2563eb" },
            "websiteContext": { "enforcementMode": "enforced" }
        }),
    };
    let brief = Brief {
        project_type: "website".to_string(),
        audience: "operators".to_string(),
        content_hierarchy: vec!["hero".to_string()],
        page_structure: serde_json::json!(["hero"]),
        visual_direction: "clear".to_string(),
        recommended_template: "next-app".to_string(),
        assumptions: Vec::new(),
        missing_information: Vec::new(),
    };
    let compiled = compile_website_design_context(
        &profile,
        &brief,
        &template,
        &DesignContextCompileOptions {
            enforcement_enabled: true,
            ..Default::default()
        },
    )
    .unwrap();
    let environment = VerificationEnvironmentBinding {
        registry_version: VERIFIER_REGISTRY_VERSION.to_string(),
        capability_snapshot_hash: "snapshot".to_string(),
        browser_executable: None,
        browser_collector_executable: None,
        capabilities: BTreeMap::from([
            ("token".to_string(), available_capability()),
            ("dom".to_string(), available_capability()),
            ("computed-style".to_string(), unavailable_capability()),
            ("a11y".to_string(), unavailable_capability()),
            ("viewport".to_string(), unavailable_capability()),
        ]),
    };

    let error = super::start::ensure_enforced_verifiers_available(&compiled.manifest, &environment)
        .unwrap_err();
    assert!(
        error.to_string().contains(
            "design verification unavailable for enforced DCP: computed-style,a11y,viewport"
        ),
        "{error}"
    );
}

fn available_capability() -> VerifierCapability {
    VerifierCapability {
        available: true,
        detail: "available".to_string(),
    }
}

fn unavailable_capability() -> VerifierCapability {
    VerifierCapability {
        available: false,
        detail: "unavailable".to_string(),
    }
}

#[tokio::test]
async fn edit_inherits_frozen_dcp_from_its_base_version_creator() {
    let store = RuntimeStore::new();
    let source = store
        .create_run(
            "project-edit-dcp".to_string(),
            AgentPhase::Build,
            "build".to_string(),
            "fixture".to_string(),
            Vec::new(),
        )
        .await;
    let brief = Brief {
        project_type: "website".to_string(),
        audience: "operators".to_string(),
        content_hierarchy: vec!["hero".to_string()],
        page_structure: serde_json::json!(["hero"]),
        visual_direction: "clear".to_string(),
        recommended_template: "next-app".to_string(),
        assumptions: Vec::new(),
        missing_information: Vec::new(),
    };
    store.write_brief(&source.id, brief.clone()).await.unwrap();
    let profile = dcp_profile("project-edit-dcp");
    let source = store
        .attach_run_effective_design_profile(
            &source.id,
            &profile,
            Some("website"),
            Some("next-app"),
        )
        .await
        .unwrap();
    let template = BuiltInTemplateRegistry::built_in()
        .current(&TemplateId::parse("next-app").unwrap())
        .unwrap();
    let dcp = compile_website_design_context(
        &profile.effective_for("website", "next-app").unwrap(),
        &brief,
        &template,
        &DesignContextCompileOptions::default(),
    )
    .unwrap();
    let mut tampered_dcp = dcp.clone();
    tampered_dcp.files.insert(
        "inputs/design-profile.json".to_string(),
        "{\"id\":\"tampered-profile\",\"version\":1}".to_string(),
    );
    assert!(store
        .attach_run_design_context(&source.id, &tampered_dcp, &VerifierRegistry::discover(),)
        .await
        .is_err());
    assert!(store
        .get_run(&source.id)
        .await
        .unwrap()
        .design_context_manifest
        .is_none());
    let source = store
        .attach_run_design_context(&source.id, &dcp, &VerifierRegistry::discover())
        .await
        .unwrap();
    assert!(store
        .record_design_context_file_read(&source.id, "inputs/design-profile.json")
        .await
        .is_err());
    assert!(store
        .record_run_design_context_materialization(&source.id, "wrong-materialization-hash")
        .await
        .is_err());
    store
        .record_run_design_context_materialization(
            &source.id,
            &dcp.manifest.payload.artifact_manifest_hash,
        )
        .await
        .unwrap();
    store
        .record_design_context_file_read(&source.id, "inputs/design-profile.json")
        .await
        .unwrap();
    store
        .set_run_design_context_style_contract_verified(&source.id, true)
        .await
        .unwrap();
    let version = store
        .create_project_version_candidate(
            "project-edit-dcp",
            &source.id,
            "http://preview.invalid".to_string(),
            None,
            Some("runtime://source-snapshots/project-edit-dcp/source".to_string()),
        )
        .await;
    store
        .promote_project_version("project-edit-dcp", &source.id, &version.id)
        .await
        .unwrap();
    let edit = store
        .create_run_with_context(
            "project-edit-dcp".to_string(),
            AgentPhase::Edit,
            "edit".to_string(),
            "fixture".to_string(),
            Vec::new(),
            None,
            Some(version.id.clone()),
        )
        .await;
    let service = RunLifecycleService::new(
        RuntimeConfig::from_env(),
        store.clone(),
        Arc::new(RecordingSessionLauncher::default()),
        Arc::new(UnusedSandboxProvisioner),
        Arc::new(UnusedEditWorkspaceRestorer),
        crate::design_profile_service::DesignProfileService::new(store.clone()),
    );
    let inherited = super::start::inherit_edit_design_context_from_base_version(&service, &edit)
        .await
        .unwrap();

    assert_eq!(
        inherited.design_context_content_hash,
        source.design_context_content_hash
    );
    assert_eq!(
        inherited.design_context_manifest,
        source.design_context_manifest
    );
    assert_eq!(
        inherited.design_context_artifacts,
        source.design_context_artifacts
    );
    assert_eq!(
        inherited.design_profile_effective_hash,
        source.design_profile_effective_hash
    );
    assert_eq!(inherited.brief_version, source.brief_version);
    assert!(inherited.design_context_materialization_hash.is_none());
    assert!(inherited.design_context_read_files.is_empty());
    assert_eq!(inherited.design_context_style_contract_verified, None);
}

#[tokio::test]
async fn persistent_disabled_policy_overrides_matching_config_allowlist_and_records_observe() {
    let project_id = "project-enforcement-observe";
    let store = RuntimeStore::new();
    let brief_run = store
        .create_run(
            project_id.to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "fixture".to_string(),
            Vec::new(),
        )
        .await;
    let brief_id = store
        .write_brief_draft(
            &brief_run.id,
            Brief {
                project_type: "website".to_string(),
                audience: "operators".to_string(),
                content_hierarchy: vec!["hero".to_string()],
                page_structure: serde_json::json!(["hero"]),
                visual_direction: "clear".to_string(),
                recommended_template: "next-app".to_string(),
                assumptions: Vec::new(),
                missing_information: Vec::new(),
            },
        )
        .await
        .unwrap();
    store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
    let mut profile = dcp_profile(project_id);
    profile.website_context = serde_json::json!({ "enforcementMode": "enforced" });
    store.create_design_profile(profile.clone()).await.unwrap();
    store
        .bind_project_design_profile(project_id, &profile.id)
        .await
        .unwrap();
    let binding = store
        .create_sandbox_binding(
            project_id,
            "sandbox-enforcement-observe".to_string(),
            "sandbox-claim-enforcement-observe".to_string(),
            "workspace-sandbox-claim-enforcement-observe".to_string(),
            "anydesign-next-app-pool".to_string(),
            "anydesign-sandboxes".to_string(),
            SandboxChannelProtocol::Websocket,
        )
        .await
        .unwrap();
    store
        .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
        .await
        .unwrap();
    let mut config = RuntimeConfig::from_env();
    config.enable_design_context_package = true;
    config.enable_design_context_enforcement = true;
    config.design_context_enforcement_allowlist_json = Some(
        serde_json::json!([{
            "projectId": project_id,
            "designProfileId": profile.id,
            "designProfileVersion": profile.version,
        }])
        .to_string(),
    );
    store
        .upsert_design_context_enforcement_policy(
            project_id,
            &profile.id,
            profile.version,
            false,
            Some(0),
            "fixture-rollback".to_string(),
        )
        .await
        .unwrap();
    let launcher = Arc::new(RecordingSessionLauncher::default());
    let service = RunLifecycleService::new(
        config,
        store.clone(),
        launcher.clone(),
        Arc::new(UnusedSandboxProvisioner),
        Arc::new(UnusedEditWorkspaceRestorer),
        crate::design_profile_service::DesignProfileService::new(store.clone()),
    );

    let outcome = service
        .start(super::StartRunCommand {
            project_id: project_id.to_string(),
            phase: AgentPhase::Build,
            agent_profile: "build".to_string(),
            input_context: super::StartRunContext {
                brief_id: Some(brief_id),
                sandbox_binding_id: Some(binding.id),
                ..Default::default()
            },
        })
        .await
        .unwrap();
    assert_eq!(outcome.status, "queued");
    let run = store.get_run(&outcome.run_id).await.unwrap();
    assert_eq!(
        run.design_context_effective_compatibility_mode.as_deref(),
        Some("observe")
    );
    let enforcement_binding = run.design_context_enforcement_binding.as_ref().unwrap();
    assert_eq!(enforcement_binding.source, "persistent");
    assert!(!enforcement_binding.enabled);
    assert_eq!(enforcement_binding.policy_revision, Some(1));
    assert_eq!(
        enforcement_binding.policy_updated_by.as_deref(),
        Some("fixture-rollback")
    );
    let run_id = run.id.clone();
    assert_eq!(launcher.launched.lock().unwrap().as_slice(), [run_id]);
    assert!(store.audit_records().await.iter().any(|record| {
        record.tool == "design_context.enforcement_allowlist"
            && record.run_id == run.id
            && record.decision == "observe"
            && record.input_summary.contains("policyRevision=1")
            && record
                .input_summary
                .contains("policyUpdatedBy=fixture-rollback")
    }));
    assert!(store.events(&run.id).await.iter().any(|event| matches!(
        event,
        AgentEvent::MetricRecorded {
            name,
            value: 1,
            metadata: Some(metadata),
            ..
        } if name == "design_context_package_compiled_total"
            && metadata["mode"] == "observe"
            && metadata["surface"] == "website"
            && metadata["status"] == "passed"
            && metadata["reason"] == "compiled"
    )));
}

fn dcp_profile(project_id: &str) -> DesignProfile {
    DesignProfile {
        id: "dcp-profile".to_string(),
        schema_version: "design-profile@1".to_string(),
        name: "DCP Profile".to_string(),
        status: "active".to_string(),
        version: 1,
        scope: serde_json::json!({ "projectId": project_id }),
        source: serde_json::json!({ "kind": "manual" }),
        product: serde_json::json!({ "name": "DCP Profile", "category": "fixture" }),
        brand: serde_json::json!({}),
        visual: serde_json::json!({ "direction": "quiet operational interface" }),
        tokens: serde_json::json!({}),
        runtime_token_mapping: serde_json::json!({
            "color.background": "#ffffff",
            "color.surface": "#f8fafc",
            "color.surfaceStrong": "#e2e8f0",
            "color.text": "#0f172a",
            "color.muted": "#475569",
            "color.primary": "#2563eb",
            "color.primaryContrast": "#ffffff",
            "color.border": "#cbd5e1",
            "radius.card": "8px",
            "radius.control": "6px",
            "font.sans": "Inter, sans-serif",
            "shadow.soft": "0 1px 2px rgba(15, 23, 42, 0.12)"
        }),
        extended_token_mapping: serde_json::json!({}),
        components: serde_json::json!({}),
        website_context: serde_json::json!({ "enforcementMode": "observe" }),
        content: serde_json::json!({}),
        accessibility: serde_json::json!({}),
        technical: serde_json::json!({ "allowedTemplates": ["next-app"] }),
        governance: serde_json::json!({ "conflictBehavior": "ask" }),
        signature_rules: Vec::new(),
        overrides: serde_json::json!({}),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}
