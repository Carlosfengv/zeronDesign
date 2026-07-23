use crate::{
    acceptance_contract::AcceptanceContract,
    content_plan_approval::ContentPlanApproval,
    conversation::RuntimeStore,
    design_context::frozen_run_design_context_manifest,
    project::resolve_built_in_template_for_init,
    templates::TemplateSpec,
    types::{canonical_json_bytes, canonical_json_hash, sha256_hex, AgentPhase, AgentRun, Brief},
    visual_artifact_store::VisualArtifactStore,
    visual_contracts::RunVisualBindingRole,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::Path};

pub const GENERATION_CONTEXT_SCHEMA: &str = "generation-context@1";
pub const GENERATION_CONTEXT_COMPILER_VERSION: &str = "generation-context-compiler@1";
pub const GENERATION_CONTEXT_STATUS_SCHEMA: &str = "generation-context-status@1";
pub const GENERATION_CONTEXT_MAX_BYTES: usize = 64 * 1024;
const ACCEPTANCE_MAX_BYTES: usize = 8 * 1024;
const INLINE_CONTENT_MAX_BYTES: usize = 24 * 1024;
const EDITABLE_SURFACE_MAX_BYTES: usize = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentPlanIdentity {
    pub plan_id: String,
    pub revision: u64,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationContext {
    pub schema_version: String,
    pub run_binding: GenerationContextRunBinding,
    pub payload: GenerationContextPayload,
    pub attestation: GenerationContextAttestation,
    pub context_content_hash: String,
    pub run_context_binding_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationContextRunBinding {
    pub run_id: String,
    pub project_id: String,
    pub workspace_namespace: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationContextPayload {
    pub phase: AgentPhase,
    pub execution_profile: String,
    pub change: Option<Value>,
    pub identity: GenerationContextIdentity,
    pub content_plan: GenerationContentPlanContext,
    pub acceptance: GenerationAcceptanceContext,
    pub design: GenerationDesignContext,
    pub content: GenerationContentContext,
    pub visuals: GenerationVisualContext,
    pub project: GenerationProjectContext,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationContextIdentity {
    pub brief_hash: String,
    pub base_revision: Option<String>,
    pub design_source: GenerationDesignSource,
    pub template_id: String,
    pub template_version: String,
    pub template_manifest_hash: String,
    pub surface: String,
    pub app_root: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GenerationDesignSource {
    DesignProfile {
        #[serde(rename = "dcpContentHash")]
        dcp_content_hash: String,
        #[serde(rename = "designProfileId")]
        design_profile_id: String,
        #[serde(rename = "designProfileVersion")]
        design_profile_version: u32,
        #[serde(rename = "effectiveProfileHash")]
        effective_profile_hash: String,
    },
    TemplateDefault {
        #[serde(rename = "templateDefaultContractHash")]
        template_default_contract_hash: String,
    },
}

impl GenerationDesignSource {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::DesignProfile { .. } => "design_profile",
            Self::TemplateDefault { .. } => "template_default",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationContentPlanContext {
    pub plan_id: String,
    pub revision: u64,
    pub content_hash: String,
    pub approval: GenerationContentPlanApproval,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationContentPlanApproval {
    pub state: String,
    pub approval_id: String,
    pub approved_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationAcceptanceContext {
    pub required_routes: Vec<String>,
    pub required_text: Vec<String>,
    pub forbidden_text: Vec<String>,
    pub responsive_viewports: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationDesignContext {
    pub visual_direction: String,
    pub required_rules: Vec<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferred_rules: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rule_selection: Option<GenerationRuleSelection>,
    pub runtime_tokens: BTreeMap<String, String>,
    pub component_recipes: Vec<Value>,
    pub layout_guidance: Vec<Value>,
    pub fidelity_mode: String,
    pub compatibility_mode: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationRuleSelection {
    pub rule_selection_version: String,
    pub context_rule_set_hash: String,
    pub enforced_assertion_set_hash: String,
    pub included_required_rule_ids: Vec<String>,
    pub included_preferred_rule_ids: Vec<String>,
    pub enforced_required_rule_ids: Vec<String>,
    pub excluded_rule_ids: Vec<String>,
    pub reason_codes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationContentContext {
    pub inline_sources: Vec<Value>,
    pub indexed_sources: Vec<Value>,
    pub required_source_sections: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationVisualContext {
    pub bindings: Vec<Value>,
    pub delivery_mode: String,
    pub review_mode: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationProjectContext {
    pub editable_surface: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationState {
    Verified,
    Pending,
    NotApplicable,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationContextAttestation {
    pub artifact_state: AttestationState,
    pub materialization_state: AttestationState,
    pub template_identity_state: AttestationState,
    pub app_root_declaration_state: AttestationState,
    pub content_approval_state: AttestationState,
    pub visual_bindings_state: AttestationState,
    pub frozen_resources_hash: String,
    pub runtime_attestation_hash: String,
    pub verified_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationContextStatus {
    pub schema_version: String,
    pub run_id: String,
    pub run_contract_version: String,
    pub status: String,
    pub runtime_mode: Option<String>,
    pub compiler_version: Option<String>,
    pub context_content_hash: Option<String>,
    pub run_context_binding_hash: Option<String>,
    pub runtime_attestation_hash: Option<String>,
    pub visual_binding_set_hash: Option<String>,
    pub visual_delivery_state: Option<String>,
    pub execution_profile: Option<String>,
    pub budget_profile_id: Option<String>,
    pub budget_profile_hash: Option<String>,
    pub budget_profile_rollout_mode: Option<String>,
    pub workflow_state: Option<String>,
    pub context_window_epoch: u64,
    pub context_injected_turn: Option<u32>,
    pub operation_id: Option<String>,
    pub operation_attempt: u32,
    pub predecessor_run_id: Option<String>,
    pub successor_run_id: Option<String>,
    pub continuation_snapshot_id: Option<String>,
    pub content_plan: Option<ContentPlanIdentity>,
    pub approval_id: Option<String>,
    pub approval_state: Option<String>,
    pub design_source_kind: Option<String>,
}

#[derive(Debug)]
pub enum GenerationContextError {
    Invalid(String),
    RequiredContentOverflow { section: &'static str, bytes: usize },
    Storage(String),
}

impl std::fmt::Display for GenerationContextError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => write!(formatter, "{message}"),
            Self::RequiredContentOverflow { section, bytes } => write!(
                formatter,
                "generation_context.required_content_overflow: {section} is {bytes} bytes"
            ),
            Self::Storage(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for GenerationContextError {}

pub async fn compile_generation_context(
    store: &RuntimeStore,
    runtime_storage_dir: &Path,
    run: &AgentRun,
    approval: &ContentPlanApproval,
) -> Result<GenerationContext, GenerationContextError> {
    approval
        .validate()
        .map_err(|error| GenerationContextError::Invalid(error.to_string()))?;
    if !approval.is_verified() || approval.project_id != run.project_id {
        return Err(GenerationContextError::Invalid(
            "generation_context.content_approval_invalid".to_string(),
        ));
    }
    let authoritative = store
        .content_plan_approval_store()
        .verify_exact(
            &run.project_id,
            &approval.plan_id,
            approval.revision,
            &approval.content_hash,
        )
        .map_err(|error| GenerationContextError::Invalid(error.to_string()))?;
    if authoritative.state
        != crate::content_plan_approval::ContentPlanApprovalVerificationState::Verified
        || authoritative
            .approval
            .as_ref()
            .map(|record| record.approval_id.as_str())
            != Some(approval.approval_id.as_str())
    {
        return Err(GenerationContextError::Invalid(
            "generation_context.content_approval_not_authoritative".to_string(),
        ));
    }
    let brief_id = run.brief_version.as_deref().ok_or_else(|| {
        GenerationContextError::Invalid("generation_context.brief_missing".to_string())
    })?;
    let brief = store.get_brief(brief_id).await.ok_or_else(|| {
        GenerationContextError::Invalid("generation_context.brief_missing".to_string())
    })?;
    let sources = store.content_sources(run.id.as_str()).await;
    let acceptance = match store.get_acceptance_contract(brief_id).await {
        Some(contract) => contract,
        None => AcceptanceContract::compile(brief_id, &brief, &sources, None)
            .map_err(GenerationContextError::Invalid)?,
    };
    acceptance
        .validate()
        .map_err(GenerationContextError::Invalid)?;
    let template_id = run
        .design_profile_template
        .as_deref()
        .unwrap_or(&brief.recommended_template);
    let template = resolve_built_in_template_for_init(template_id)
        .await
        .map_err(|error| GenerationContextError::Invalid(error.to_string()))?;
    let workspace_namespace = store
        .get_project_access(&run.project_id)
        .await
        .map(|record| record.workspace_namespace)
        .ok_or_else(|| {
            GenerationContextError::Invalid(
                "generation_context.workspace_namespace_missing".to_string(),
            )
        })?;
    let dcp = frozen_run_design_context_manifest(run).map_err(GenerationContextError::Invalid)?;
    let app_root = dcp
        .as_ref()
        .map(|manifest| manifest.payload.expected_app_root.clone())
        .or_else(|| {
            run.project_state_snapshot
                .as_ref()
                .map(|state| state.app_root.clone())
        })
        .unwrap_or_else(|| "project".to_string());
    validate_app_root(&app_root)?;

    let style_contract = template.style.render(&template.id, Path::new(&app_root));
    let template_default_contract_hash = canonical_json_hash(&json!({
        "domain": "template-default-contract-hash@1",
        "templateManifestHash": template.manifest_sha256.as_str(),
        "styleContract": style_contract,
        "defaultTokenSnapshot": {},
    }));
    let design_source = if let Some(manifest) = dcp.as_ref() {
        GenerationDesignSource::DesignProfile {
            dcp_content_hash: manifest.content_hash.clone(),
            design_profile_id: manifest.payload.design_profile_id.clone(),
            design_profile_version: manifest.payload.design_profile_version,
            effective_profile_hash: manifest.payload.effective_profile_hash.clone(),
        }
    } else {
        GenerationDesignSource::TemplateDefault {
            template_default_contract_hash: template_default_contract_hash.clone(),
        }
    };
    let acceptance_context = GenerationAcceptanceContext {
        required_routes: if acceptance.required_routes.is_empty() {
            vec!["/".to_string()]
        } else {
            acceptance.required_routes.clone()
        },
        required_text: acceptance.required_text.clone(),
        forbidden_text: acceptance.forbidden_text.clone(),
        responsive_viewports: vec![375, 1440],
    };
    enforce_section_budget("acceptance", &acceptance_context, ACCEPTANCE_MAX_BYTES)?;

    let execution_profile = execution_profile(store, run)?;
    let change = change_context(store, run)?;
    let design = design_context(&brief, run, dcp.as_ref(), change.as_ref());
    let content = content_context(&sources);
    let visuals = visual_context(store, runtime_storage_dir, run).await?;
    let editable_surface = editable_surface(&template, &app_root)?;
    enforce_value_budget(
        "editable_surface",
        &editable_surface,
        EDITABLE_SURFACE_MAX_BYTES,
    )?;
    let cache_key = canonical_json_hash(&json!({
        "domain": "generation-context-cache-key@1",
        "compilerVersion": GENERATION_CONTEXT_COMPILER_VERSION,
        "ruleSelectionVersion": "conservative-context-rules@1",
        "phase": run.phase,
        "change": &change,
        "briefHash": acceptance.brief_digest,
        "baseRevision": run.base_version_id,
        "designSource": &design_source,
        "template": {
            "id": template.id.as_str(),
            "version": template.version.as_str(),
            "manifestHash": template.manifest_sha256.as_str(),
            "surface": template.surface,
            "appRoot": app_root,
        },
        "contentPlanApproval": {
            "approvalId": approval.approval_id,
            "planId": approval.plan_id,
            "revision": approval.revision,
            "contentHash": approval.content_hash,
            "confirmationEventId": approval.confirmation_event_id,
            "approvedAt": approval.approved_at,
        },
        "acceptanceHash": acceptance.contract_digest,
        "design": &design,
        "content": &content,
        "visualBindings": &visuals.bindings,
        "editableSurface": &editable_surface,
    }));
    let payload = if let Some(cached) = store.cached_generation_context_payload(&cache_key).await {
        cached
    } else {
        let payload = GenerationContextPayload {
            phase: run.phase,
            execution_profile,
            change,
            identity: GenerationContextIdentity {
                brief_hash: acceptance.brief_digest.clone(),
                base_revision: run.base_version_id.clone(),
                design_source: design_source.clone(),
                template_id: template.id.as_str().to_string(),
                template_version: template.version.as_str().to_string(),
                template_manifest_hash: template.manifest_sha256.as_str().to_string(),
                surface: template.surface.to_string(),
                app_root: app_root.clone(),
            },
            content_plan: GenerationContentPlanContext {
                plan_id: approval.plan_id.clone(),
                revision: approval.revision,
                content_hash: approval.content_hash.clone(),
                approval: GenerationContentPlanApproval {
                    state: "verified".to_string(),
                    approval_id: approval.approval_id.clone(),
                    approved_revision: approval.revision,
                },
            },
            acceptance: acceptance_context,
            design,
            content,
            visuals,
            project: GenerationProjectContext { editable_surface },
        };
        store
            .cache_generation_context_payload(cache_key, payload.clone())
            .await;
        payload
    };
    let payload_bytes = canonical_json_bytes(&serde_json::to_value(&payload).map_err(|error| {
        GenerationContextError::Storage(format!("serialize GenerationContext payload: {error}"))
    })?);
    if payload_bytes.len() > GENERATION_CONTEXT_MAX_BYTES {
        return Err(GenerationContextError::RequiredContentOverflow {
            section: "payload",
            bytes: payload_bytes.len(),
        });
    }
    let context_content_hash = canonical_json_hash(&json!({
        "domain": "generation-context-content-hash@1",
        "compilerVersion": GENERATION_CONTEXT_COMPILER_VERSION,
        "payload": payload,
    }));
    let visual_binding_set_hash = canonical_json_hash(
        &serde_json::to_value(&payload.visuals.bindings)
            .map_err(|error| GenerationContextError::Storage(error.to_string()))?,
    );
    let frozen_resources_hash = canonical_json_hash(&json!({
        "domain": "generation-context-frozen-resources-hash@1",
        "briefHash": payload.identity.brief_hash,
        "contentPlanHash": payload.content_plan.content_hash,
        "contentApprovalIdentity": {
            "approvalId": approval.approval_id,
            "confirmationEventId": approval.confirmation_event_id,
            "approvedAt": approval.approved_at,
        },
        "acceptanceHash": acceptance.contract_digest,
        "visualBindingSetHash": visual_binding_set_hash,
        "templateManifestHash": payload.identity.template_manifest_hash,
        "baseRevision": payload.identity.base_revision,
        "dcpContentHash": dcp.as_ref().map(|manifest| manifest.content_hash.as_str()),
        "dcpArtifactManifestHash": run.design_context_artifact_manifest_hash,
        "dcpMaterializationHash": run.design_context_materialization_hash,
        "templateDefaultContractHash": match &design_source {
            GenerationDesignSource::TemplateDefault { template_default_contract_hash } => Some(template_default_contract_hash),
            GenerationDesignSource::DesignProfile { .. } => None,
        },
        "editImpactPlanHash": run.edit_impact_plan_hash,
    }));
    let artifact_state = if dcp.is_some() {
        AttestationState::Verified
    } else {
        AttestationState::NotApplicable
    };
    let materialization_state = materialization_attestation_state(
        dcp.as_ref()
            .map(|manifest| manifest.payload.artifact_manifest_hash.as_str()),
        run.design_context_materialization_hash.as_deref(),
    );
    let visual_bindings_state = if payload.visuals.bindings.is_empty() {
        AttestationState::NotApplicable
    } else {
        AttestationState::Verified
    };
    let runtime_attestation_hash = canonical_json_hash(&json!({
        "domain": "generation-context-runtime-attestation-hash@1",
        "artifactState": artifact_state,
        "materializationState": materialization_state,
        "templateIdentityState": AttestationState::Verified,
        "appRootDeclarationState": AttestationState::Verified,
        "contentApprovalState": AttestationState::Verified,
        "visualBindingsState": visual_bindings_state,
        "expectedAppRoot": app_root,
        "templateManifestHash": template.manifest_sha256.as_str(),
    }));
    let run_binding = GenerationContextRunBinding {
        run_id: run.id.clone(),
        project_id: run.project_id.clone(),
        workspace_namespace,
    };
    let run_context_binding_hash = canonical_json_hash(&json!({
        "domain": "run-context-binding-hash@1",
        "contextContentHash": context_content_hash,
        "runId": run_binding.run_id,
        "projectId": run_binding.project_id,
        "workspaceNamespace": run_binding.workspace_namespace,
        "frozenResourcesHash": frozen_resources_hash,
        "runtimeAttestationHash": runtime_attestation_hash,
    }));
    Ok(GenerationContext {
        schema_version: GENERATION_CONTEXT_SCHEMA.to_string(),
        run_binding,
        payload,
        attestation: GenerationContextAttestation {
            artifact_state,
            materialization_state,
            template_identity_state: AttestationState::Verified,
            app_root_declaration_state: AttestationState::Verified,
            content_approval_state: AttestationState::Verified,
            visual_bindings_state,
            frozen_resources_hash,
            runtime_attestation_hash,
            verified_at: Utc::now(),
        },
        context_content_hash,
        run_context_binding_hash,
    })
}

fn materialization_attestation_state(
    dcp_artifact_manifest_hash: Option<&str>,
    materialization_hash: Option<&str>,
) -> AttestationState {
    let Some(expected_hash) = dcp_artifact_manifest_hash else {
        return AttestationState::NotApplicable;
    };
    if materialization_hash == Some(expected_hash) {
        AttestationState::Verified
    } else {
        // GenerationContext is frozen before AgentLoop bootstraps the sandbox.
        // The immutable attestation therefore records the expected transition;
        // the Runtime gate verifies the live materialization hash before any
        // project operation is allowed.
        AttestationState::Pending
    }
}

fn execution_profile(
    store: &RuntimeStore,
    run: &AgentRun,
) -> Result<String, GenerationContextError> {
    if run.phase == AgentPhase::Build {
        return Ok("greenfield_static".to_string());
    }
    let cold = if let Some(plan_hash) = run.edit_impact_plan_hash.as_deref() {
        let (project_id, plan) = store
            .edit_guard_store()
            .get_plan(plan_hash)
            .ok_or_else(|| {
                GenerationContextError::Invalid(
                    "generation_context.edit_impact_plan_missing".to_string(),
                )
            })?;
        if project_id != run.project_id || plan.plan_hash != plan_hash {
            return Err(GenerationContextError::Invalid(
                "generation_context.edit_impact_plan_mismatch".to_string(),
            ));
        }
        edit_impact_plan_requires_cold_dev(&plan)
    } else {
        false
    };
    Ok(execution_profile_for_phase(run.phase, cold).to_string())
}

pub(crate) fn edit_impact_plan_requires_cold_dev(
    plan: &crate::visual_contracts::EditImpactPlan,
) -> bool {
    plan.operations.iter().any(|operation| {
        matches!(
            operation,
            crate::visual_contracts::EditImpactOperation::Dependency
        )
    }) || plan.targets.iter().any(|target| {
        let target = target.to_ascii_lowercase();
        target.ends_with("package.json")
            || target.ends_with("package-lock.json")
            || target.ends_with("pnpm-lock.yaml")
            || target.ends_with("yarn.lock")
            || target.contains("next.config")
            || target.contains("vite.config")
    })
}

pub(crate) fn execution_profile_for_phase(phase: AgentPhase, cold: bool) -> &'static str {
    match (phase, cold) {
        (AgentPhase::Edit, true) => "cold_dev",
        (AgentPhase::Edit, false) => "warm_hmr",
        (AgentPhase::Repair, true) => "repair_cold_dev",
        (AgentPhase::Repair, false) => "repair_warm",
        _ => "greenfield_static",
    }
}

fn design_context(
    brief: &Brief,
    run: &AgentRun,
    dcp: Option<&crate::design_context::DesignContextManifest>,
    change: Option<&Value>,
) -> GenerationDesignContext {
    let signature_rules = run
        .design_context_artifacts
        .get("inputs/design-profile.json")
        .and_then(|text| serde_json::from_str::<Value>(text).ok())
        .and_then(|profile| profile.get("signatureRules").cloned())
        .and_then(|rules| rules.as_array().cloned())
        .unwrap_or_default();
    let mut required_rules = Vec::new();
    let mut preferred_rules = Vec::new();
    let mut included_required_rule_ids = Vec::new();
    let mut included_preferred_rule_ids = Vec::new();
    let mut excluded_rule_ids = Vec::new();
    let mut reason_codes = BTreeMap::new();
    let operations = change_operations(change);
    for rule in signature_rules {
        let id = rule
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        if !crate::design_profile::signature_rule_applies_to_surface(&rule, "website") {
            excluded_rule_ids.push(id.clone());
            reason_codes.insert(id, "surface_not_applicable".to_string());
            continue;
        }
        match rule.get("priority").and_then(Value::as_str) {
            Some("required") => {
                included_required_rule_ids.push(id.clone());
                reason_codes.insert(id, "required_surface_conservative".to_string());
                required_rules.push(rule);
            }
            Some("preferred") if preferred_rule_applies(run.phase, &rule, &operations) => {
                included_preferred_rule_ids.push(id.clone());
                reason_codes.insert(id, "preferred_operation_match".to_string());
                preferred_rules.push(rule);
            }
            Some("preferred") => {
                excluded_rule_ids.push(id.clone());
                reason_codes.insert(id, "preferred_operation_not_applicable".to_string());
            }
            _ => {
                excluded_rule_ids.push(id.clone());
                reason_codes.insert(id, "unsupported_priority".to_string());
            }
        }
    }
    included_required_rule_ids.sort();
    included_preferred_rule_ids.sort();
    excluded_rule_ids.sort();
    let enforced_required_rule_ids = if run.phase == AgentPhase::Build
        || change
            .and_then(|change| change.pointer("/editImpactPlan/scope"))
            .and_then(Value::as_str)
            == Some("global")
    {
        included_required_rule_ids.clone()
    } else {
        Vec::new()
    };
    let context_rule_set_hash = canonical_json_hash(&json!({
        "domain": "context-rule-set-hash@1",
        "ruleSelectionVersion": "conservative-context-rules@1",
        "requiredRules": required_rules,
        "preferredRules": preferred_rules,
    }));
    let enforced_assertion_set_hash = canonical_json_hash(&json!({
        "domain": "enforced-assertion-set-hash@1",
        "ruleSelectionVersion": "conservative-context-rules@1",
        "ruleIds": enforced_required_rule_ids,
    }));
    let rule_selection = dcp.map(|_| GenerationRuleSelection {
        rule_selection_version: "conservative-context-rules@1".to_string(),
        context_rule_set_hash,
        enforced_assertion_set_hash,
        included_required_rule_ids,
        included_preferred_rule_ids,
        enforced_required_rule_ids,
        excluded_rule_ids,
        reason_codes,
    });
    let component_recipes = run
        .design_context_artifacts
        .get("inputs/component-recipes.json")
        .and_then(|text| serde_json::from_str::<Vec<Value>>(text).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|recipe| {
            run.phase == AgentPhase::Build
                || recipe.get("priority").and_then(Value::as_str) == Some("required")
        })
        .collect();
    let layout_guidance = dcp
        .map(|manifest| manifest.payload.layout_guidance.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|guidance| {
            run.phase == AgentPhase::Build
                || guidance.get("priority").and_then(Value::as_str) == Some("required")
        })
        .collect();
    GenerationDesignContext {
        visual_direction: brief.visual_direction.clone(),
        required_rules,
        preferred_rules,
        rule_selection,
        runtime_tokens: dcp
            .map(|manifest| manifest.payload.resolved_runtime_tokens.clone())
            .unwrap_or_default(),
        component_recipes,
        layout_guidance,
        fidelity_mode: run
            .design_fidelity_mode
            .clone()
            .unwrap_or_else(|| "template_default".to_string()),
        compatibility_mode: dcp
            .map(|manifest| {
                serde_json::to_value(manifest.payload.effective_compatibility_mode)
                    .ok()
                    .and_then(|value| value.as_str().map(ToOwned::to_owned))
                    .unwrap_or_else(|| "observe".to_string())
            })
            .unwrap_or_else(|| "observe".to_string()),
    }
}

fn change_operations(change: Option<&Value>) -> Vec<String> {
    change
        .and_then(|change| change.pointer("/editImpactPlan/operations"))
        .and_then(Value::as_array)
        .map(|operations| {
            operations
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn preferred_rule_applies(phase: AgentPhase, rule: &Value, operations: &[String]) -> bool {
    if phase == AgentPhase::Build {
        return true;
    }
    let category = rule
        .get("category")
        .and_then(Value::as_str)
        .unwrap_or_default();
    operations.iter().any(|operation| match operation.as_str() {
        "copy" => category == "content",
        "style" => matches!(category, "color" | "typography" | "spacing"),
        "layout" => matches!(category, "composition" | "spacing"),
        "component" => category == "component",
        "navigation" => matches!(category, "navigation" | "composition"),
        _ => false,
    })
}

fn content_context(sources: &[crate::types::ContentSource]) -> GenerationContentContext {
    let mut inline_sources = Vec::new();
    let mut indexed_sources = Vec::new();
    let mut inline_bytes = 0usize;
    for source in sources.iter().filter(|source| source.readable) {
        let bytes = source.text.len();
        let user_original = source.kind == "user" || source.kind.starts_with("user_");
        if bytes <= 4 * 1024 && inline_bytes.saturating_add(bytes) <= INLINE_CONTENT_MAX_BYTES {
            inline_sources.push(json!({
                "sourceId": source.id,
                "provenance": if user_original { "user_original" } else { "pending" },
                "confirmationState": if user_original { "confirmed" } else { "unconfirmed" },
                "content": source.text,
            }));
            inline_bytes += bytes;
        } else {
            indexed_sources.push(json!({
                "sourceId": source.id,
                "provenance": if user_original { "user_original" } else { "pending" },
                "confirmationState": if user_original { "confirmed" } else { "unconfirmed" },
                "contentHash": sha256_hex(source.text.as_bytes()),
                "bytes": bytes,
            }));
        }
    }
    GenerationContentContext {
        inline_sources,
        indexed_sources,
        required_source_sections: Vec::new(),
    }
}

async fn visual_context(
    store: &RuntimeStore,
    runtime_storage_dir: &Path,
    run: &AgentRun,
) -> Result<GenerationVisualContext, GenerationContextError> {
    let bindings = store
        .run_visual_bindings(&run.id)
        .await
        .map_err(|error| GenerationContextError::Storage(error.to_string()))?;
    let artifact_store = VisualArtifactStore::open(runtime_storage_dir.join("visual-artifacts"))
        .map_err(|error| GenerationContextError::Storage(error.to_string()))?;
    let mut projected = Vec::new();
    for binding in bindings
        .into_iter()
        .filter(|binding| binding.role == RunVisualBindingRole::Reference)
    {
        let artifact = artifact_store
            .get(&binding.artifact_id)
            .map_err(|error| GenerationContextError::Storage(error.to_string()))?
            .ok_or_else(|| {
                GenerationContextError::Invalid(format!(
                    "generation_context.visual_artifact_missing: {}",
                    binding.artifact_id
                ))
            })?;
        if artifact.project_id != run.project_id {
            return Err(GenerationContextError::Invalid(
                "generation_context.visual_artifact_project_mismatch".to_string(),
            ));
        }
        artifact_store
            .read_content(&artifact.id)
            .map_err(|error| GenerationContextError::Invalid(error.to_string()))?;
        projected.push(json!({
            "artifactId": artifact.id,
            "role": "reference",
            "sha256": artifact.sha256,
            "mediaType": artifact.media_type,
            "width": artifact.width,
            "height": artifact.height,
            "route": binding.route,
            "viewport": binding.viewport,
            "order": binding.order,
        }));
    }
    Ok(GenerationVisualContext {
        bindings: projected,
        delivery_mode: "provider_image_content_blocks".to_string(),
        review_mode: "advisory".to_string(),
    })
}

fn editable_surface(
    template: &TemplateSpec,
    app_root: &str,
) -> Result<Value, GenerationContextError> {
    let view = template
        .editable_surface_view(app_root)
        .map_err(GenerationContextError::Invalid)?;
    serde_json::to_value(view).map_err(|error| GenerationContextError::Storage(error.to_string()))
}

fn change_context(
    store: &RuntimeStore,
    run: &AgentRun,
) -> Result<Option<Value>, GenerationContextError> {
    let Some(plan_hash) = run.edit_impact_plan_hash.as_deref() else {
        return Ok(run
            .edit_base
            .as_ref()
            .map(|edit_base| json!({ "editBase": edit_base })));
    };
    let (project_id, plan) = store
        .edit_guard_store()
        .get_plan(plan_hash)
        .ok_or_else(|| {
            GenerationContextError::Invalid(
                "generation_context.edit_impact_plan_missing".to_string(),
            )
        })?;
    if project_id != run.project_id || plan.plan_hash != plan_hash {
        return Err(GenerationContextError::Invalid(
            "generation_context.edit_impact_plan_mismatch".to_string(),
        ));
    }
    let approval_state = if plan.requires_confirmation {
        store
            .edit_guard_store()
            .validate_executable(&store.draft_preview_store(), plan_hash)
            .map_err(|error| GenerationContextError::Invalid(error.to_string()))?;
        "verified"
    } else {
        "not_required"
    };
    Ok(Some(json!({
        "editBase": run.edit_base,
        "editImpactPlan": plan,
        "approvalState": approval_state,
    })))
}

fn validate_app_root(app_root: &str) -> Result<(), GenerationContextError> {
    if app_root.trim().is_empty()
        || app_root.starts_with('/')
        || app_root
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Err(GenerationContextError::Invalid(
            "generation_context.app_root_invalid".to_string(),
        ));
    }
    Ok(())
}

fn enforce_section_budget<T: Serialize>(
    section: &'static str,
    value: &T,
    max: usize,
) -> Result<(), GenerationContextError> {
    let value = serde_json::to_value(value)
        .map_err(|error| GenerationContextError::Storage(error.to_string()))?;
    enforce_value_budget(section, &value, max)
}

fn enforce_value_budget(
    section: &'static str,
    value: &Value,
    max: usize,
) -> Result<(), GenerationContextError> {
    let bytes = canonical_json_bytes(value).len();
    if bytes > max {
        Err(GenerationContextError::RequiredContentOverflow { section, bytes })
    } else {
        Ok(())
    }
}

pub fn status_for_run(run: &AgentRun) -> GenerationContextStatus {
    let design_source_kind = run
        .generation_context
        .as_ref()
        .and_then(|context| context.pointer("/payload/identity/designSource/kind"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    GenerationContextStatus {
        schema_version: GENERATION_CONTEXT_STATUS_SCHEMA.to_string(),
        run_id: run.id.clone(),
        run_contract_version: run
            .run_contract_version
            .clone()
            .unwrap_or_else(|| "legacy@1".to_string()),
        status: run
            .generation_context_status
            .clone()
            .unwrap_or_else(|| "not_compiled".to_string()),
        runtime_mode: run.generation_context_runtime_mode.clone(),
        compiler_version: run.generation_context_compiler_version.clone(),
        context_content_hash: run.generation_context_content_hash.clone(),
        run_context_binding_hash: run.generation_context_binding_hash.clone(),
        runtime_attestation_hash: run.generation_context_runtime_attestation_hash.clone(),
        visual_binding_set_hash: run.visual_binding_set_hash.clone(),
        visual_delivery_state: run.visual_delivery_state.clone(),
        execution_profile: run.execution_profile.clone(),
        budget_profile_id: run
            .budget_profile
            .as_ref()
            .map(|profile| profile.profile_id.clone()),
        budget_profile_hash: run
            .budget_profile
            .as_ref()
            .map(|profile| profile.profile_hash.clone()),
        budget_profile_rollout_mode: run
            .budget_profile
            .as_ref()
            .map(|profile| profile.rollout_mode.clone()),
        workflow_state: run.workflow_state.clone(),
        context_window_epoch: run.context_window_epoch,
        context_injected_turn: run.context_injected_turn,
        operation_id: run.operation_id.clone(),
        operation_attempt: run.operation_attempt.max(1),
        predecessor_run_id: run.predecessor_run_id.clone(),
        successor_run_id: run.successor_run_id.clone(),
        continuation_snapshot_id: run.continuation_snapshot_id.clone(),
        content_plan: match (
            run.content_plan_id.clone(),
            run.content_plan_revision,
            run.content_plan_hash.clone(),
        ) {
            (Some(plan_id), Some(revision), Some(content_hash)) => Some(ContentPlanIdentity {
                plan_id,
                revision,
                content_hash,
            }),
            _ => None,
        },
        approval_id: run.content_plan_approval_id.clone(),
        approval_state: run.content_plan_approval_state.clone(),
        design_source_kind,
    }
}

pub fn validate_generation_context_binding(
    context: &GenerationContext,
) -> Result<(), GenerationContextError> {
    if context.schema_version != GENERATION_CONTEXT_SCHEMA {
        return Err(GenerationContextError::Invalid(
            "generation_context.schema_mismatch".to_string(),
        ));
    }
    let profile_valid = matches!(
        (
            context.payload.phase,
            context.payload.execution_profile.as_str()
        ),
        (AgentPhase::Build, "greenfield_static")
            | (AgentPhase::Edit, "warm_hmr" | "cold_dev")
            | (AgentPhase::Repair, "repair_warm" | "repair_cold_dev")
    );
    if !profile_valid {
        return Err(GenerationContextError::Invalid(
            "generation_context.execution_profile_invalid".to_string(),
        ));
    }
    let content_hash = canonical_json_hash(&json!({
        "domain": "generation-context-content-hash@1",
        "compilerVersion": GENERATION_CONTEXT_COMPILER_VERSION,
        "payload": context.payload,
    }));
    if content_hash != context.context_content_hash {
        return Err(GenerationContextError::Invalid(
            "generation_context.content_hash_mismatch".to_string(),
        ));
    }
    let runtime_attestation_hash = canonical_json_hash(&json!({
        "domain": "generation-context-runtime-attestation-hash@1",
        "artifactState": context.attestation.artifact_state,
        "materializationState": context.attestation.materialization_state,
        "templateIdentityState": context.attestation.template_identity_state,
        "appRootDeclarationState": context.attestation.app_root_declaration_state,
        "contentApprovalState": context.attestation.content_approval_state,
        "visualBindingsState": context.attestation.visual_bindings_state,
        "expectedAppRoot": context.payload.identity.app_root,
        "templateManifestHash": context.payload.identity.template_manifest_hash,
    }));
    if runtime_attestation_hash != context.attestation.runtime_attestation_hash {
        return Err(GenerationContextError::Invalid(
            "generation_context.runtime_attestation_hash_mismatch".to_string(),
        ));
    }
    let binding_hash = canonical_json_hash(&json!({
        "domain": "run-context-binding-hash@1",
        "contextContentHash": context.context_content_hash,
        "runId": context.run_binding.run_id,
        "projectId": context.run_binding.project_id,
        "workspaceNamespace": context.run_binding.workspace_namespace,
        "frozenResourcesHash": context.attestation.frozen_resources_hash,
        "runtimeAttestationHash": context.attestation.runtime_attestation_hash,
    }));
    if binding_hash != context.run_context_binding_hash {
        return Err(GenerationContextError::Invalid(
            "generation_context.binding_hash_mismatch".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        content_plan_approval::{RecordContentPlanApproval, RecordContentPlanChange},
        types::{AgentPhase, ContentSource},
    };

    #[test]
    fn materialization_attestation_is_pending_until_the_frozen_dcp_is_verified() {
        assert_eq!(
            materialization_attestation_state(None, None),
            AttestationState::NotApplicable
        );
        assert_eq!(
            materialization_attestation_state(Some("artifact-hash"), None),
            AttestationState::Pending
        );
        assert_eq!(
            materialization_attestation_state(Some("artifact-hash"), Some("wrong-hash")),
            AttestationState::Pending
        );
        assert_eq!(
            materialization_attestation_state(Some("artifact-hash"), Some("artifact-hash")),
            AttestationState::Verified
        );
    }

    async fn fixture() -> (
        RuntimeStore,
        AgentRun,
        ContentPlanApproval,
        std::path::PathBuf,
    ) {
        let store = RuntimeStore::new();
        let project_id = "project-generation-context";
        store
            .upsert_project_access(
                project_id,
                "principal-generation-context".to_string(),
                "ws-generation-context".to_string(),
            )
            .await
            .unwrap();
        let sources = vec![ContentSource::readable(
            "source-1",
            "user",
            "Authoritative homepage copy",
        )];
        let brief_run = store
            .create_run(
                project_id.to_string(),
                AgentPhase::Brief,
                "brief".to_string(),
                "internal-balanced".to_string(),
                sources.clone(),
            )
            .await;
        let brief_id = store
            .write_brief_draft(
                &brief_run.id,
                Brief {
                    project_type: "website".to_string(),
                    audience: "operators".to_string(),
                    content_hierarchy: vec!["hero".to_string()],
                    page_structure: json!([]),
                    visual_direction: "calm editorial SaaS".to_string(),
                    recommended_template: "next-app".to_string(),
                    assumptions: Vec::new(),
                    missing_information: Vec::new(),
                },
            )
            .await
            .unwrap();
        store.confirm_brief(&brief_run.id, &brief_id).await.unwrap();
        let run = store
            .create_run_with_context(
                project_id.to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "internal-balanced".to_string(),
                sources,
                Some(brief_id),
                None,
            )
            .await;
        let approval = store
            .content_plan_approval_store()
            .approve(RecordContentPlanApproval {
                project_id: project_id.to_string(),
                plan_id: "content-plan-1".to_string(),
                revision: 1,
                content_hash: "a".repeat(64),
                confirmation_event_id: "confirmation-event-1".to_string(),
            })
            .unwrap();
        let storage = std::env::temp_dir().join(format!(
            "zerondesign-generation-context-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        (store, run, approval, storage)
    }

    #[tokio::test]
    async fn template_default_hash_is_stable_and_binding_is_run_specific() {
        let (store, run, approval, storage) = fixture().await;
        let first = compile_generation_context(&store, &storage, &run, &approval)
            .await
            .unwrap();
        let second = compile_generation_context(&store, &storage, &run, &approval)
            .await
            .unwrap();
        assert_eq!(first.context_content_hash, second.context_content_hash);
        assert_eq!(
            first.run_context_binding_hash,
            second.run_context_binding_hash
        );
        assert_eq!(
            first.payload.identity.design_source.kind(),
            "template_default"
        );
        validate_generation_context_binding(&first).unwrap();

        let sibling = store
            .create_run_with_context(
                run.project_id.clone(),
                AgentPhase::Build,
                "build".to_string(),
                "internal-balanced".to_string(),
                store.content_sources(&run.id).await,
                run.brief_version.clone(),
                None,
            )
            .await;
        let sibling_context = compile_generation_context(&store, &storage, &sibling, &approval)
            .await
            .unwrap();
        assert_eq!(
            first.context_content_hash,
            sibling_context.context_content_hash
        );
        assert_ne!(
            first.run_context_binding_hash,
            sibling_context.run_context_binding_hash
        );
        let _ = std::fs::remove_dir_all(storage);
    }

    #[tokio::test]
    async fn rule_selection_keeps_surface_required_and_filters_edit_preferred_by_operation() {
        let (store, mut run, _approval, storage) = fixture().await;
        let brief = store
            .get_brief(run.brief_version.as_deref().unwrap())
            .await
            .unwrap();
        run.design_context_artifacts.insert(
            "inputs/design-profile.json".to_string(),
            json!({
                "signatureRules": [
                    { "id": "required-web", "priority": "required", "appliesTo": ["website"], "category": "color" },
                    { "id": "required-docs", "priority": "required", "appliesTo": ["docs"], "category": "color" },
                    { "id": "preferred-color", "priority": "preferred", "appliesTo": "all", "category": "color" },
                    { "id": "preferred-component", "priority": "preferred", "appliesTo": ["website"], "category": "component" }
                ]
            })
            .to_string(),
        );
        run.phase = AgentPhase::Edit;
        let change = json!({
            "editImpactPlan": {
                "scope": "local",
                "operations": ["style"]
            }
        });
        let selected = design_context(&brief, &run, None, Some(&change));
        assert_eq!(selected.required_rules.len(), 1);
        assert_eq!(selected.required_rules[0]["id"], "required-web");
        assert_eq!(selected.preferred_rules.len(), 1);
        assert_eq!(selected.preferred_rules[0]["id"], "preferred-color");
        assert!(selected.rule_selection.is_none());
        let _ = std::fs::remove_dir_all(storage);
    }

    #[tokio::test]
    async fn content_plan_change_changes_both_hashes_and_binding_is_immutable() {
        let (store, run, approval, storage) = fixture().await;
        let first = compile_generation_context(&store, &storage, &run, &approval)
            .await
            .unwrap();
        store.bind_run_generation_context(&first).await.unwrap();
        store
            .content_plan_approval_store()
            .record_plan_change(RecordContentPlanChange {
                project_id: run.project_id.clone(),
                plan_id: approval.plan_id.clone(),
                revision: 2,
                content_hash: "b".repeat(64),
                change_event_id: "change-event-2".to_string(),
            })
            .unwrap();
        let changed_approval = store
            .content_plan_approval_store()
            .approve(RecordContentPlanApproval {
                project_id: run.project_id.clone(),
                plan_id: approval.plan_id.clone(),
                revision: 2,
                content_hash: "b".repeat(64),
                confirmation_event_id: "confirmation-event-changed".to_string(),
            })
            .unwrap();
        let changed = compile_generation_context(&store, &storage, &run, &changed_approval)
            .await
            .unwrap();
        assert_ne!(first.context_content_hash, changed.context_content_hash);
        assert_ne!(
            first.run_context_binding_hash,
            changed.run_context_binding_hash
        );
        assert!(store.bind_run_generation_context(&changed).await.is_err());
        let _ = std::fs::remove_dir_all(storage);
    }

    #[test]
    fn binding_validation_detects_payload_tampering() {
        let payload = GenerationContextPayload {
            phase: AgentPhase::Build,
            execution_profile: "greenfield_static".to_string(),
            change: None,
            identity: GenerationContextIdentity {
                brief_hash: "a".repeat(64),
                base_revision: None,
                design_source: GenerationDesignSource::TemplateDefault {
                    template_default_contract_hash: "b".repeat(64),
                },
                template_id: "next-app".to_string(),
                template_version: "next-app@1".to_string(),
                template_manifest_hash: "c".repeat(64),
                surface: "website".to_string(),
                app_root: "project".to_string(),
            },
            content_plan: GenerationContentPlanContext {
                plan_id: "plan-1".to_string(),
                revision: 1,
                content_hash: "d".repeat(64),
                approval: GenerationContentPlanApproval {
                    state: "verified".to_string(),
                    approval_id: "approval-1".to_string(),
                    approved_revision: 1,
                },
            },
            acceptance: GenerationAcceptanceContext {
                required_routes: vec!["/".to_string()],
                required_text: Vec::new(),
                forbidden_text: Vec::new(),
                responsive_viewports: vec![375, 1440],
            },
            design: GenerationDesignContext {
                visual_direction: "calm".to_string(),
                required_rules: Vec::new(),
                preferred_rules: Vec::new(),
                rule_selection: None,
                runtime_tokens: BTreeMap::new(),
                component_recipes: Vec::new(),
                layout_guidance: Vec::new(),
                fidelity_mode: "template_default".to_string(),
                compatibility_mode: "observe".to_string(),
            },
            content: GenerationContentContext {
                inline_sources: Vec::new(),
                indexed_sources: Vec::new(),
                required_source_sections: Vec::new(),
            },
            visuals: GenerationVisualContext {
                bindings: Vec::new(),
                delivery_mode: "provider_image_content_blocks".to_string(),
                review_mode: "advisory".to_string(),
            },
            project: GenerationProjectContext {
                editable_surface: json!({}),
            },
        };
        let content_hash = canonical_json_hash(&json!({
            "domain": "generation-context-content-hash@1",
            "compilerVersion": GENERATION_CONTEXT_COMPILER_VERSION,
            "payload": payload,
        }));
        let mut context = GenerationContext {
            schema_version: GENERATION_CONTEXT_SCHEMA.to_string(),
            run_binding: GenerationContextRunBinding {
                run_id: "run-1".to_string(),
                project_id: "project-1".to_string(),
                workspace_namespace: "ws-test".to_string(),
            },
            payload,
            attestation: GenerationContextAttestation {
                artifact_state: AttestationState::NotApplicable,
                materialization_state: AttestationState::NotApplicable,
                template_identity_state: AttestationState::Verified,
                app_root_declaration_state: AttestationState::Verified,
                content_approval_state: AttestationState::Verified,
                visual_bindings_state: AttestationState::NotApplicable,
                frozen_resources_hash: "e".repeat(64),
                runtime_attestation_hash: "f".repeat(64),
                verified_at: Utc::now(),
            },
            context_content_hash: content_hash,
            run_context_binding_hash: String::new(),
        };
        context.attestation.runtime_attestation_hash = canonical_json_hash(&json!({
            "domain": "generation-context-runtime-attestation-hash@1",
            "artifactState": context.attestation.artifact_state,
            "materializationState": context.attestation.materialization_state,
            "templateIdentityState": context.attestation.template_identity_state,
            "appRootDeclarationState": context.attestation.app_root_declaration_state,
            "contentApprovalState": context.attestation.content_approval_state,
            "visualBindingsState": context.attestation.visual_bindings_state,
            "expectedAppRoot": context.payload.identity.app_root,
            "templateManifestHash": context.payload.identity.template_manifest_hash,
        }));
        context.run_context_binding_hash = canonical_json_hash(&json!({
            "domain": "run-context-binding-hash@1",
            "contextContentHash": context.context_content_hash,
            "runId": context.run_binding.run_id,
            "projectId": context.run_binding.project_id,
            "workspaceNamespace": context.run_binding.workspace_namespace,
            "frozenResourcesHash": context.attestation.frozen_resources_hash,
            "runtimeAttestationHash": context.attestation.runtime_attestation_hash,
        }));
        validate_generation_context_binding(&context).unwrap();
        context
            .payload
            .acceptance
            .required_routes
            .push("/changed".to_string());
        assert!(validate_generation_context_binding(&context).is_err());
    }

    #[test]
    fn golden_vector_matches_runtime_canonical_hashes() {
        let vector: Value = serde_json::from_str(include_str!(
            "../../../packages/shared/fixtures/generation-context-golden-vector.json"
        ))
        .unwrap();
        let content_hash = canonical_json_hash(&json!({
            "domain": "generation-context-content-hash@1",
            "compilerVersion": vector["compilerVersion"],
            "payload": vector["payload"],
        }));
        assert_eq!(content_hash, vector["contextContentHash"]);
        let binding = &vector["runBindingInput"];
        let binding_hash = canonical_json_hash(&json!({
            "domain": "run-context-binding-hash@1",
            "contextContentHash": content_hash,
            "runId": binding["runId"],
            "projectId": binding["projectId"],
            "workspaceNamespace": binding["workspaceNamespace"],
            "frozenResourcesHash": binding["frozenResourcesHash"],
            "runtimeAttestationHash": binding["runtimeAttestationHash"],
        }));
        assert_eq!(binding_hash, vector["runContextBindingHash"]);
    }
}
