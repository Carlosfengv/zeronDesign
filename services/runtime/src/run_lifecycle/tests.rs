use super::{
    BuildSandboxProvisioner, EditWorkspaceRestorer, RunLifecycleService, RunSessionLauncher,
};
use crate::{
    config::RuntimeConfig,
    conversation::RuntimeStore,
    design_context::{
        compile_website_design_context, DesignContextCompileOptions,
        VerificationEnvironmentBinding, VerifierCapability, VerifierRegistry,
        VERIFIER_REGISTRY_VERSION,
    },
    templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry},
    types::{
        AgentEvent, AgentPhase, AgentRunStatus, Brief, DesignProfile, EffectiveDesignProfile,
        SandboxBindingStatus, SandboxChannelProtocol,
    },
};
use chrono::Utc;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
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

#[test]
fn enforced_dcp_rejects_an_unavailable_verifier_before_session_launch() {
    let template = BuiltInTemplateRegistry::built_in()
        .current(&TemplateId::parse("astro-website").unwrap())
        .unwrap();
    let profile = EffectiveDesignProfile {
        design_profile_id: "profile-enforced".to_string(),
        version: 1,
        surface: "website".to_string(),
        template: "astro-website".to_string(),
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
        recommended_template: "astro-website".to_string(),
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
        recommended_template: "astro-website".to_string(),
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
            Some("astro-website"),
        )
        .await
        .unwrap();
    let template = BuiltInTemplateRegistry::built_in()
        .current(&TemplateId::parse("astro-website").unwrap())
        .unwrap();
    let dcp = compile_website_design_context(
        &profile.effective_for("website", "astro-website").unwrap(),
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
                recommended_template: "astro-website".to_string(),
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
            "anydesign-astro-website-pool".to_string(),
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
        technical: serde_json::json!({ "allowedTemplates": ["astro-website"] }),
        governance: serde_json::json!({ "conflictBehavior": "ask" }),
        signature_rules: Vec::new(),
        overrides: serde_json::json!({}),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}
