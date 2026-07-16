use crate::{
    design_context::{
        frozen_run_design_context_manifest, CompiledDesignContext, DesignContextManifest,
    },
    style_contract::{read_contract_token_values, validate_token_value},
    types::{canonical_json_hash, AgentRun},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenValueSnapshot {
    pub hash: String,
    pub tokens: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenSyncState {
    AlreadyTarget,
    ApplyTarget,
    Conflict,
    NotManaged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenSyncResolution {
    KeepCurrent,
    ApplyTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenSyncItem {
    pub token: String,
    pub base: Option<String>,
    pub current: Option<String>,
    pub target: Option<String>,
    pub state: TokenSyncState,
    pub resolution: Option<TokenSyncResolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileTokenSyncPlan {
    pub base: TokenValueSnapshot,
    pub current: TokenValueSnapshot,
    pub target: TokenValueSnapshot,
    pub plan_hash: String,
    pub items: Vec<TokenSyncItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StyleContractIdentity {
    pub hash: String,
    pub version: String,
    pub template: String,
    pub app_root: String,
    pub token_mappings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileTokenSyncOperationStatus {
    Planned,
    Confirmed,
    Applying,
    Applied,
    Rejected,
    RecoveryRequired,
}

impl ProfileTokenSyncOperationStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Applied | Self::Rejected)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileTokenSyncOperation {
    pub schema_version: String,
    pub id: String,
    pub project_id: String,
    pub source_run_id: String,
    pub target_design_profile_id: String,
    pub target_design_profile_version: u32,
    pub target_effective_profile_hash: String,
    pub source_design_context_content_hash: String,
    pub style_contract_identity: StyleContractIdentity,
    pub plan: ProfileTokenSyncPlan,
    pub authorized_principal_id: String,
    pub idempotency_key: String,
    #[serde(default)]
    pub confirm_idempotency_key: Option<String>,
    pub status: ProfileTokenSyncOperationStatus,
    #[serde(default)]
    pub conflict_decisions: BTreeMap<String, TokenSyncResolution>,
    pub child_run_id: Option<String>,
    pub before_tokens: Option<TokenValueSnapshot>,
    pub after_tokens: Option<TokenValueSnapshot>,
    pub last_error: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ProfileTokenSyncOperation {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.expires_at <= now
    }
}

pub const PROFILE_TOKEN_SYNC_OPERATION_SCHEMA_V1: &str = "profile-token-sync-operation@1";

pub struct ProfileTokenSyncService;

impl ProfileTokenSyncService {
    #[allow(clippy::too_many_arguments)]
    pub fn plan_operation(
        operation_id: String,
        source_run: &AgentRun,
        target: &CompiledDesignContext,
        contract: &serde_json::Value,
        token_file_content: &str,
        authorized_principal_id: String,
        idempotency_key: String,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<ProfileTokenSyncOperation, String> {
        if operation_id.trim().is_empty()
            || authorized_principal_id.trim().is_empty()
            || idempotency_key.trim().is_empty()
        {
            return Err(
                "profile sync operation id, principal, and idempotency key are required"
                    .to_string(),
            );
        }
        if expires_at <= now {
            return Err("profile sync operation expiry must be in the future".to_string());
        }
        if source_run.design_context_materialization_hash.is_none()
            || source_run.design_context_style_contract_verified != Some(true)
        {
            return Err(
                "profile_sync_precondition_failed: source run has no verified materialized style contract"
                    .to_string(),
            );
        }
        let source_manifest = source_run_manifest(source_run)?;
        let target_payload = &target.manifest.payload;
        if target_payload.surface != "website"
            || target_payload.template != source_manifest.payload.template
            || target_payload.expected_app_root != source_manifest.payload.expected_app_root
        {
            return Err(
                "profile_sync_precondition_failed: target Design Context is not compatible with the source Website run"
                    .to_string(),
            );
        }
        let (plan, style_contract_identity) = plan_from_style_contract(
            source_manifest.payload.resolved_runtime_tokens.clone(),
            target_payload.resolved_runtime_tokens.clone(),
            contract,
            token_file_content,
        )?;
        if style_contract_identity.template != source_manifest.payload.template
            || !style_contract_app_root_matches(
                &style_contract_identity.app_root,
                &source_manifest.payload.expected_app_root,
            )
        {
            return Err(
                "profile_sync_precondition_failed: current style contract does not match the source DCP template/appRoot"
                    .to_string(),
            );
        }
        Ok(ProfileTokenSyncOperation {
            schema_version: PROFILE_TOKEN_SYNC_OPERATION_SCHEMA_V1.to_string(),
            id: operation_id,
            project_id: source_run.project_id.clone(),
            source_run_id: source_run.id.clone(),
            target_design_profile_id: target_payload.design_profile_id.clone(),
            target_design_profile_version: target_payload.design_profile_version,
            target_effective_profile_hash: target_payload.effective_profile_hash.clone(),
            source_design_context_content_hash: source_manifest.content_hash,
            style_contract_identity,
            plan,
            authorized_principal_id,
            idempotency_key,
            confirm_idempotency_key: None,
            status: ProfileTokenSyncOperationStatus::Planned,
            conflict_decisions: BTreeMap::new(),
            child_run_id: None,
            before_tokens: None,
            after_tokens: None,
            last_error: None,
            expires_at,
            created_at: now,
            updated_at: now,
        })
    }
}

fn style_contract_app_root_matches(contract_app_root: &str, expected_app_root: &str) -> bool {
    contract_app_root
        .strip_prefix("/workspace/")
        .unwrap_or(contract_app_root)
        .trim_matches('/')
        == expected_app_root
            .strip_prefix("/workspace/")
            .unwrap_or(expected_app_root)
            .trim_matches('/')
}

fn source_run_manifest(source_run: &AgentRun) -> Result<DesignContextManifest, String> {
    frozen_run_design_context_manifest(source_run)
        .map_err(|error| format!("profile_sync_precondition_failed: {error}"))?
        .ok_or_else(|| {
            "profile_sync_precondition_failed: source run has no frozen Design Context Package"
                .to_string()
        })
}

pub fn plan_from_style_contract(
    base: BTreeMap<String, String>,
    target: BTreeMap<String, String>,
    contract: &serde_json::Value,
    token_file_content: &str,
) -> Result<(ProfileTokenSyncPlan, StyleContractIdentity), String> {
    let identity = style_contract_identity(contract)?;
    for token in base.keys().chain(target.keys()) {
        if !identity.token_mappings.contains_key(token) {
            return Err(format!(
                "profile_sync_style_contract_changed: token {token} is not declared by the current style contract"
            ));
        }
    }
    let current = read_contract_token_values(contract, token_file_content).map_err(|error| {
        format!("profile_sync_precondition_failed: could not read current token values: {error}")
    })?;
    Ok((plan(base, current, target)?, identity))
}

pub fn style_contract_identity(
    contract: &serde_json::Value,
) -> Result<StyleContractIdentity, String> {
    let version = contract
        .get("version")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "style contract is missing version".to_string())?
        .to_string();
    let template = contract
        .get("template")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "style contract is missing template".to_string())?
        .to_string();
    let app_root = contract
        .get("appRoot")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "style contract is missing appRoot".to_string())?
        .to_string();
    let token_mappings = contract
        .get("tokens")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| "style contract is missing tokens map".to_string())?
        .iter()
        .map(|(token, variable)| {
            let variable = variable
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    format!("style contract token {token} must map to a CSS variable")
                })?;
            Ok((token.clone(), variable.to_string()))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let hash = canonical_json_hash(&json!({
        "version": version,
        "template": template,
        "appRoot": app_root,
        "tokens": token_mappings,
    }));
    Ok(StyleContractIdentity {
        hash,
        version,
        template,
        app_root,
        token_mappings,
    })
}

pub fn snapshot(tokens: BTreeMap<String, String>) -> Result<TokenValueSnapshot, String> {
    for (token, value) in &tokens {
        if token.trim().is_empty() {
            return Err("token names must be non-empty".to_string());
        }
        validate_token_value(value)
            .map_err(|error| format!("token {token} has invalid value: {error}"))?;
    }
    let hash = canonical_json_hash(&json!(tokens));
    Ok(TokenValueSnapshot { hash, tokens })
}

pub fn plan(
    base: BTreeMap<String, String>,
    current: BTreeMap<String, String>,
    target: BTreeMap<String, String>,
) -> Result<ProfileTokenSyncPlan, String> {
    let base = snapshot(base)?;
    let current = snapshot(current)?;
    let target = snapshot(target)?;
    let token_names = base
        .tokens
        .keys()
        .chain(current.tokens.keys())
        .chain(target.tokens.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let items = token_names
        .into_iter()
        .map(|token| {
            let base_value = base.tokens.get(&token).cloned();
            let current_value = current.tokens.get(&token).cloned();
            let target_value = target.tokens.get(&token).cloned();
            let state = match target_value.as_ref() {
                None => TokenSyncState::NotManaged,
                Some(target_value) if current_value.as_ref() == Some(target_value) => {
                    TokenSyncState::AlreadyTarget
                }
                Some(_) if current_value == base_value => TokenSyncState::ApplyTarget,
                Some(_) => TokenSyncState::Conflict,
            };
            TokenSyncItem {
                token,
                base: base_value,
                current: current_value,
                target: target_value,
                state,
                resolution: None,
            }
        })
        .collect::<Vec<_>>();
    let plan_hash = canonical_json_hash(&json!({
        "baseHash": base.hash,
        "currentHash": current.hash,
        "targetHash": target.hash,
        "items": items,
    }));
    Ok(ProfileTokenSyncPlan {
        base,
        current,
        target,
        plan_hash,
        items,
    })
}

pub fn resolve(
    plan: &ProfileTokenSyncPlan,
    decisions: &BTreeMap<String, TokenSyncResolution>,
) -> Result<ProfileTokenSyncPlan, String> {
    for token in decisions.keys() {
        let Some(item) = plan.items.iter().find(|item| &item.token == token) else {
            return Err("token sync decisions contain an unknown token".to_string());
        };
        if item.state != TokenSyncState::Conflict {
            return Err(format!(
                "token sync decision is only allowed for conflicting token {token}"
            ));
        }
    }
    let mut resolved = plan.clone();
    for item in &mut resolved.items {
        match item.state {
            TokenSyncState::Conflict => {
                let resolution = decisions.get(&item.token).copied().ok_or_else(|| {
                    format!(
                        "conflicting token {} requires an explicit resolution",
                        item.token
                    )
                })?;
                item.resolution = Some(resolution);
            }
            TokenSyncState::ApplyTarget => item.resolution = Some(TokenSyncResolution::ApplyTarget),
            TokenSyncState::AlreadyTarget | TokenSyncState::NotManaged => {}
        }
    }
    Ok(resolved)
}

pub fn resolved_target_tokens(
    plan: &ProfileTokenSyncPlan,
) -> Result<BTreeMap<String, String>, String> {
    let mut result = BTreeMap::new();
    for item in &plan.items {
        match item.state {
            TokenSyncState::ApplyTarget => {
                let target = item.target.clone().ok_or_else(|| {
                    format!("apply-target token {} has no target value", item.token)
                })?;
                result.insert(item.token.clone(), target);
            }
            TokenSyncState::Conflict => match item.resolution {
                Some(TokenSyncResolution::ApplyTarget) => {
                    let target = item.target.clone().ok_or_else(|| {
                        format!("apply-target token {} has no target value", item.token)
                    })?;
                    result.insert(item.token.clone(), target);
                }
                Some(TokenSyncResolution::KeepCurrent) => {}
                None => {
                    return Err(format!(
                        "conflicting token {} has not been resolved",
                        item.token
                    ))
                }
            },
            TokenSyncState::AlreadyTarget | TokenSyncState::NotManaged => {}
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::RuntimeStore;
    use serde_json::json;
    use std::fs;

    fn tokens(values: &[(&str, &str)]) -> BTreeMap<String, String> {
        values
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    fn operation(plan: ProfileTokenSyncPlan) -> ProfileTokenSyncOperation {
        let now = Utc::now();
        ProfileTokenSyncOperation {
            schema_version: "profile-token-sync-operation@1".to_string(),
            id: "profile-sync-1".to_string(),
            project_id: "project-1".to_string(),
            source_run_id: "run-1".to_string(),
            target_design_profile_id: "profile-2".to_string(),
            target_design_profile_version: 2,
            target_effective_profile_hash: "target-hash".to_string(),
            source_design_context_content_hash: "source-dcp-hash".to_string(),
            style_contract_identity: StyleContractIdentity {
                hash: "contract-hash".to_string(),
                version: "runtime-style-contract@1".to_string(),
                template: "astro-website".to_string(),
                app_root: "project".to_string(),
                token_mappings: tokens(&[("color.primary", "--runtime-primary")]),
            },
            plan,
            authorized_principal_id: "user-1".to_string(),
            idempotency_key: "idempotency-1".to_string(),
            confirm_idempotency_key: None,
            status: ProfileTokenSyncOperationStatus::Planned,
            conflict_decisions: BTreeMap::new(),
            child_run_id: None,
            before_tokens: None,
            after_tokens: None,
            last_error: None,
            expires_at: now + chrono::Duration::minutes(10),
            created_at: now,
            updated_at: now,
        }
    }

    fn compiled_context(
        profile_id: &str,
        profile_version: u32,
        tokens: BTreeMap<String, String>,
    ) -> CompiledDesignContext {
        let profile = json!({
            "id": profile_id,
            "version": profile_version,
            "scope": { "projectId": "project-1" },
        });
        let profile_text = String::from_utf8(crate::types::canonical_json_bytes(&profile)).unwrap();
        let files = BTreeMap::from([(
            "inputs/design-profile.json".to_string(),
            profile_text.clone(),
        )]);
        let artifact_manifest = crate::design_context::DesignContextArtifactManifest {
            schema_version: "design-context-artifacts@1".to_string(),
            artifacts: vec![crate::design_context::DesignContextArtifact {
                path: "inputs/design-profile.json".to_string(),
                kind: "profile".to_string(),
                bytes: profile_text.len() as u64,
                sha256: crate::types::sha256_hex(profile_text.as_bytes()),
                required_before_mutation: true,
            }],
        };
        let artifact_manifest_hash =
            canonical_json_hash(&serde_json::to_value(&artifact_manifest).unwrap());
        let payload = crate::design_context::DesignContextPackagePayload {
            schema_version: "design-context@1".to_string(),
            design_profile_id: profile_id.to_string(),
            design_profile_version: profile_version,
            base_profile_hash: "base-profile-hash".to_string(),
            effective_profile_hash: canonical_json_hash(&profile),
            brief_hash: "brief-hash".to_string(),
            brief_schema_version: "brief@1".to_string(),
            surface: "website".to_string(),
            template: "astro-website".to_string(),
            template_manifest_sha256: "template-hash".to_string(),
            expected_app_root: "project".to_string(),
            compiler_version: "compiler@1".to_string(),
            declared_enforcement_mode: crate::design_context::ProfileEnforcementMode::Observe,
            effective_compatibility_mode: crate::design_context::ProfileCompatibilityMode::Observe,
            verification_policy: crate::design_context::VerificationPolicySnapshot {
                policy_id: "policy@1".to_string(),
                a11y_ruleset_version: "a11y@1".to_string(),
                viewport_matrix_id: "viewport@1".to_string(),
                required_verifier_kinds: Vec::new(),
            },
            artifact_manifest_hash,
            resolved_token_snapshot_hash: canonical_json_hash(&json!(tokens)),
            resolved_runtime_tokens: tokens,
            required_reads: Vec::new(),
            craft_packs: Vec::new(),
            layout_guidance: Vec::new(),
            warnings: Vec::new(),
        };
        let content_hash = canonical_json_hash(&serde_json::to_value(&payload).unwrap());
        CompiledDesignContext {
            manifest: crate::design_context::DesignContextManifest {
                schema_version: "design-context-manifest@1".to_string(),
                content_hash,
                artifact_manifest,
                payload,
            },
            files,
        }
    }

    #[test]
    fn plans_three_way_token_changes_without_overwriting_manual_edits() {
        let plan = plan(
            tokens(&[("color.primary", "#111111"), ("color.surface", "#ffffff")]),
            tokens(&[("color.primary", "#222222"), ("color.surface", "#ffffff")]),
            tokens(&[("color.primary", "#333333"), ("color.surface", "#eeeeee")]),
        )
        .unwrap();
        assert_eq!(plan.items[0].state, TokenSyncState::Conflict);
        assert_eq!(plan.items[1].state, TokenSyncState::ApplyTarget);
        assert!(resolved_target_tokens(&plan).is_err());

        let resolved = resolve(
            &plan,
            &BTreeMap::from([(
                "color.primary".to_string(),
                TokenSyncResolution::KeepCurrent,
            )]),
        )
        .unwrap();
        assert_eq!(
            resolved_target_tokens(&resolved).unwrap(),
            tokens(&[("color.surface", "#eeeeee")])
        );
    }

    #[test]
    fn snapshot_hashes_are_canonical_and_change_with_values() {
        let first = snapshot(tokens(&[
            ("color.primary", "#111111"),
            ("color.surface", "#fff"),
        ]))
        .unwrap();
        let reordered = snapshot(tokens(&[
            ("color.surface", "#fff"),
            ("color.primary", "#111111"),
        ]))
        .unwrap();
        let changed = snapshot(tokens(&[
            ("color.primary", "#222222"),
            ("color.surface", "#fff"),
        ]))
        .unwrap();
        assert_eq!(first.hash, reordered.hash);
        assert_ne!(first.hash, changed.hash);
    }

    #[test]
    fn does_not_treat_target_absence_as_a_delete() {
        let plan = plan(
            tokens(&[("color.primary", "#111")]),
            tokens(&[("color.primary", "#222")]),
            BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(plan.items[0].state, TokenSyncState::NotManaged);
        assert!(resolved_target_tokens(&plan).unwrap().is_empty());
    }

    #[test]
    fn rejects_decisions_for_non_conflicting_tokens() {
        let plan = plan(
            tokens(&[("color.primary", "#111")]),
            tokens(&[("color.primary", "#111")]),
            tokens(&[("color.primary", "#222")]),
        )
        .unwrap();
        let error = resolve(
            &plan,
            &BTreeMap::from([(
                "color.primary".to_string(),
                TokenSyncResolution::KeepCurrent,
            )]),
        )
        .unwrap_err();
        assert!(error.contains("only allowed for conflicting"));
    }

    #[test]
    fn plans_from_actual_contract_token_file_and_freezes_mapping_identity() {
        let contract = json!({
            "version": "runtime-style-contract@1",
            "template": "astro-website",
            "appRoot": "project",
            "tokens": { "color.primary": "--runtime-primary" }
        });
        let (plan, identity) = plan_from_style_contract(
            tokens(&[("color.primary", "#111")]),
            tokens(&[("color.primary", "#333")]),
            &contract,
            ":root { --runtime-primary: #222; }",
        )
        .unwrap();
        assert_eq!(plan.current.tokens["color.primary"], "#222");
        assert_eq!(plan.items[0].state, TokenSyncState::Conflict);
        assert_eq!(
            identity.token_mappings["color.primary"],
            "--runtime-primary"
        );
        assert!(!identity.hash.is_empty());
    }

    #[test]
    fn refuses_token_sync_when_contract_mapping_drifted() {
        let contract = json!({
            "version": "runtime-style-contract@1",
            "template": "astro-website",
            "appRoot": "project",
            "tokens": { "color.secondary": "--runtime-secondary" }
        });
        let error = plan_from_style_contract(
            tokens(&[("color.primary", "#111")]),
            BTreeMap::new(),
            &contract,
            ":root { --runtime-secondary: #222; }",
        )
        .unwrap_err();
        assert!(error.contains("profile_sync_style_contract_changed"));
    }

    #[tokio::test]
    async fn operation_is_idempotent_persisted_and_terminal_state_is_immutable() {
        let root = std::env::temp_dir().join(format!(
            "anydesign-profile-token-sync-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let store = RuntimeStore::with_checkpoint_dir(&root);
        let plan = plan(
            tokens(&[("color.primary", "#111")]),
            tokens(&[("color.primary", "#111")]),
            tokens(&[("color.primary", "#222")]),
        )
        .unwrap();
        let operation = operation(plan);
        let created = store
            .create_profile_token_sync_operation(operation.clone())
            .await
            .unwrap();
        let replayed = store
            .create_profile_token_sync_operation(operation.clone())
            .await
            .unwrap();
        assert_eq!(created.id, replayed.id);

        let recovered = RuntimeStore::with_checkpoint_dir(&root)
            .profile_token_sync_operation(&operation.id)
            .await
            .unwrap();
        assert_eq!(recovered.plan.plan_hash, operation.plan.plan_hash);

        let mut applied = recovered.clone();
        applied.status = ProfileTokenSyncOperationStatus::Applied;
        store
            .update_profile_token_sync_operation(applied.clone())
            .await
            .unwrap();
        let mut invalid_reopen = applied;
        invalid_reopen.status = ProfileTokenSyncOperationStatus::Applying;
        let error = store
            .update_profile_token_sync_operation(invalid_reopen)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("terminal"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn operation_confirmation_requires_plan_hash_and_each_conflict_decision() {
        let root = std::env::temp_dir().join(format!(
            "anydesign-profile-token-sync-confirm-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let store = RuntimeStore::with_checkpoint_dir(&root);
        let plan = plan(
            tokens(&[("color.primary", "#111")]),
            tokens(&[("color.primary", "#222")]),
            tokens(&[("color.primary", "#333")]),
        )
        .unwrap();
        let operation = operation(plan.clone());
        store
            .create_profile_token_sync_operation(operation.clone())
            .await
            .unwrap();
        assert!(store
            .confirm_profile_token_sync_operation(
                &operation.id,
                &plan.plan_hash,
                BTreeMap::new(),
                "confirm-1".to_string(),
            )
            .await
            .is_err());
        let decisions = BTreeMap::from([(
            "color.primary".to_string(),
            TokenSyncResolution::KeepCurrent,
        )]);
        let confirmed = store
            .confirm_profile_token_sync_operation(
                &operation.id,
                &plan.plan_hash,
                decisions.clone(),
                "confirm-1".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(confirmed.status, ProfileTokenSyncOperationStatus::Confirmed);
        let replayed = store
            .confirm_profile_token_sync_operation(
                &operation.id,
                &plan.plan_hash,
                decisions,
                "confirm-1".to_string(),
            )
            .await
            .unwrap();
        assert_eq!(replayed.id, confirmed.id);
        assert!(store
            .confirm_profile_token_sync_operation(
                &operation.id,
                &plan.plan_hash,
                BTreeMap::from([(
                    "color.primary".to_string(),
                    TokenSyncResolution::KeepCurrent,
                )]),
                "confirm-2".to_string(),
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("idempotency"));
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn operation_apply_requires_confirmed_plan_and_exact_after_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "anydesign-profile-token-sync-apply-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let store = RuntimeStore::with_checkpoint_dir(&root);
        let plan = plan(
            tokens(&[("color.primary", "#111")]),
            tokens(&[("color.primary", "#111")]),
            tokens(&[("color.primary", "#333")]),
        )
        .unwrap();
        let operation = operation(plan.clone());
        store
            .create_profile_token_sync_operation(operation.clone())
            .await
            .unwrap();
        store
            .confirm_profile_token_sync_operation(
                &operation.id,
                &plan.plan_hash,
                BTreeMap::new(),
                "confirm-1".to_string(),
            )
            .await
            .unwrap();
        store
            .begin_profile_token_sync_apply(&operation.id, "child-run-1")
            .await
            .unwrap();
        let applied = store
            .complete_profile_token_sync_apply(
                &operation.id,
                tokens(&[("color.primary", "#111")]),
                tokens(&[("color.primary", "#333")]),
            )
            .await
            .unwrap();
        assert_eq!(applied.status, ProfileTokenSyncOperationStatus::Applied);
        assert_eq!(applied.child_run_id.as_deref(), Some("child-run-1"));

        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn service_plans_from_frozen_source_dcp_and_actual_token_file() {
        let store = RuntimeStore::new();
        let mut source_run = store
            .create_run(
                "project-1".to_string(),
                crate::types::AgentPhase::Edit,
                "edit".to_string(),
                "test".to_string(),
                Vec::new(),
            )
            .await;
        let source = compiled_context("profile-1", 1, tokens(&[("color.primary", "#111")]));
        source_run.design_profile_id = Some(source.manifest.payload.design_profile_id.clone());
        source_run.design_profile_version = Some(source.manifest.payload.design_profile_version);
        source_run.design_profile_hash = Some(source.manifest.payload.base_profile_hash.clone());
        source_run.design_profile_effective_hash =
            Some(source.manifest.payload.effective_profile_hash.clone());
        source_run.design_profile_surface = Some(source.manifest.payload.surface.clone());
        source_run.design_profile_template = Some(source.manifest.payload.template.clone());
        source_run.design_context_package_version =
            Some(source.manifest.payload.schema_version.clone());
        source_run.design_context_manifest = Some(serde_json::to_value(&source.manifest).unwrap());
        source_run.design_context_content_hash = Some(source.manifest.content_hash.clone());
        source_run.design_context_artifact_manifest_hash =
            Some(source.manifest.payload.artifact_manifest_hash.clone());
        source_run.design_context_materialization_hash =
            Some(source.manifest.payload.artifact_manifest_hash.clone());
        source_run.design_context_compiler_version =
            Some(source.manifest.payload.compiler_version.clone());
        source_run.design_context_brief_hash = Some(source.manifest.payload.brief_hash.clone());
        source_run.design_context_verification_policy_id = Some(
            source
                .manifest
                .payload
                .verification_policy
                .policy_id
                .clone(),
        );
        source_run.design_context_expected_app_root =
            Some(source.manifest.payload.expected_app_root.clone());
        source_run.design_context_declared_enforcement_mode = Some("observe".to_string());
        source_run.design_context_effective_compatibility_mode = Some("observe".to_string());
        source_run.design_context_warnings = source.manifest.payload.warnings.clone();
        source_run.design_context_artifacts = source.files.clone();
        source_run.design_context_style_contract_verified = Some(true);
        let target = compiled_context("profile-2", 2, tokens(&[("color.primary", "#333")]));
        let now = Utc::now();
        let operation = ProfileTokenSyncService::plan_operation(
            "operation-1".to_string(),
            &source_run,
            &target,
            &json!({
                "version": "runtime-style-contract@1",
                "template": "astro-website",
                "appRoot": "/workspace/project",
                "tokens": { "color.primary": "--runtime-primary" }
            }),
            ":root { --runtime-primary: #222; }",
            "user-1".to_string(),
            "idempotency-1".to_string(),
            now + chrono::Duration::minutes(5),
            now,
        )
        .unwrap();
        assert_eq!(operation.plan.base.tokens["color.primary"], "#111");
        assert_eq!(operation.plan.current.tokens["color.primary"], "#222");
        assert_eq!(operation.plan.target.tokens["color.primary"], "#333");
        assert_eq!(operation.plan.items[0].state, TokenSyncState::Conflict);
    }
}
