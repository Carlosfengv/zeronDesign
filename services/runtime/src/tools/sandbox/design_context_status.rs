use super::*;
use crate::design_context::frozen_run_design_context_manifest;
use crate::types::AgentRun;

pub(super) fn design_context_status_tool() -> Arc<dyn Tool> {
    Arc::new(DesignContextStatusTool)
}

struct DesignContextStatusTool;

#[async_trait]
impl Tool for DesignContextStatusTool {
    fn name(&self) -> &'static str {
        "design_context.status"
    }

    fn input_schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn is_enabled(&self, ctx: &ToolContext) -> bool {
        ctx.run.design_context_manifest.is_some()
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn check_permission(&self, input: &Value, _ctx: &ToolContext) -> PermissionResult {
        allow_with_input(input, "design context status is readable")
    }

    async fn call(
        &self,
        _input: Value,
        ctx: ToolContext,
        _progress: ProgressSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(status_payload(&ctx.run)?))
    }
}

fn status_payload(run: &AgentRun) -> Result<Value, ToolError> {
    let manifest = frozen_run_design_context_manifest(run)
        .map_err(|error| {
            ToolError::typed_recoverable(
                format!("frozen Design Context identity is invalid: {error}"),
                "design_context.integrity_failed",
                json!({}),
            )
        })?
        .ok_or_else(|| {
            ToolError::typed_recoverable(
                "this run has no frozen Design Context Package",
                "design_context.not_attached",
                json!({}),
            )
        })?;
    let required_reads = manifest
        .payload
        .required_reads
        .iter()
        .filter(|requirement| requirement.phases.contains(&run.phase))
        .map(|requirement| {
            json!({
                "path": requirement.path,
                "reason": requirement.reason,
            })
        })
        .collect::<Vec<_>>();
    let missing_reads = manifest
        .payload
        .required_reads
        .iter()
        .filter(|requirement| requirement.phases.contains(&run.phase))
        .filter(|requirement| !run.design_context_read_files.contains(&requirement.path))
        .map(|requirement| requirement.path.clone())
        .collect::<Vec<_>>();
    let verification_environment = run
        .design_context_verification_environment
        .as_ref()
        .cloned()
        .unwrap_or(Value::Null);
    let verification_capabilities = verification_environment
        .get("capabilities")
        .and_then(Value::as_object)
        .map(|capabilities| {
            capabilities
                .iter()
                .map(|(kind, capability)| {
                    (
                        kind.clone(),
                        json!({
                            "available": capability
                                .get("available")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                        }),
                    )
                })
                .collect::<serde_json::Map<_, _>>()
        })
        .unwrap_or_default();
    Ok(json!({
        "package": {
            "version": run.design_context_package_version,
            "contentHash": run.design_context_content_hash,
            "artifactManifestHash": run.design_context_artifact_manifest_hash,
            "compilerVersion": run.design_context_compiler_version,
            "briefHash": run.design_context_brief_hash,
            "expectedAppRoot": run.design_context_expected_app_root,
            "declaredEnforcementMode": run.design_context_declared_enforcement_mode,
            "effectiveCompatibilityMode": run.design_context_effective_compatibility_mode,
            "warnings": run.design_context_warnings,
        },
        "materialization": {
            "hash": run.design_context_materialization_hash,
            "ready": run.design_context_materialization_hash.is_some(),
        },
        "styleContract": {
            "verified": run.design_context_style_contract_verified,
        },
        "verification": {
            "policyId": run.design_context_verification_policy_id,
            "registryVersion": verification_environment.get("registryVersion"),
            "capabilitySnapshotHash": verification_environment.get("capabilitySnapshotHash"),
            "capabilities": verification_capabilities,
        },
        "requiredReads": required_reads,
        "readFiles": run.design_context_read_files,
        "missingRequiredReads": missing_reads,
        "gate": if missing_reads.is_empty() { "ready" } else { "read_required" },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        conversation::RuntimeStore,
        design_context::{
            DesignContextArtifact, DesignContextArtifactManifest, DesignContextManifest,
            DesignContextPackagePayload, DesignContextReadRequirement, ProfileCompatibilityMode,
            ProfileEnforcementMode, VerificationPolicySnapshot,
        },
    };
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn status_never_returns_dcp_artifact_text() {
        let store = RuntimeStore::new();
        let mut run = store
            .create_run(
                "project".to_string(),
                AgentPhase::Build,
                "build".to_string(),
                "test".to_string(),
                Vec::new(),
            )
            .await;
        let profile_value = json!({
            "id": "profile",
            "version": 1,
            "scope": { "projectId": "project" },
            "private": "secret profile text"
        });
        let profile_text =
            String::from_utf8(crate::types::canonical_json_bytes(&profile_value)).unwrap();
        let profile_hash = crate::types::canonical_json_hash(&profile_value);
        let mut manifest = DesignContextManifest {
            schema_version: "design-context-manifest@1".to_string(),
            content_hash: "content".to_string(),
            artifact_manifest: DesignContextArtifactManifest {
                schema_version: "design-context-artifacts@1".to_string(),
                artifacts: vec![DesignContextArtifact {
                    path: "inputs/design-profile.json".to_string(),
                    kind: "profile".to_string(),
                    bytes: profile_text.len() as u64,
                    sha256: crate::types::sha256_hex(profile_text.as_bytes()),
                    required_before_mutation: true,
                }],
            },
            payload: DesignContextPackagePayload {
                schema_version: "design-context@1".to_string(),
                design_profile_id: "profile".to_string(),
                design_profile_version: 1,
                base_profile_hash: "base".to_string(),
                effective_profile_hash: profile_hash,
                brief_hash: "brief".to_string(),
                brief_schema_version: "brief@1".to_string(),
                surface: "website".to_string(),
                template: "astro-website".to_string(),
                template_manifest_sha256: "template".to_string(),
                expected_app_root: "project".to_string(),
                compiler_version: "compiler".to_string(),
                declared_enforcement_mode: ProfileEnforcementMode::Observe,
                effective_compatibility_mode: ProfileCompatibilityMode::Observe,
                verification_policy: VerificationPolicySnapshot {
                    policy_id: "policy".to_string(),
                    a11y_ruleset_version: "a11y".to_string(),
                    viewport_matrix_id: "viewport".to_string(),
                    required_verifier_kinds: Vec::new(),
                },
                artifact_manifest_hash: "artifacts".to_string(),
                resolved_runtime_tokens: BTreeMap::new(),
                resolved_token_snapshot_hash: "tokens".to_string(),
                required_reads: vec![DesignContextReadRequirement {
                    path: "inputs/design-profile-usage.md".to_string(),
                    reason: "usage".to_string(),
                    phases: vec![AgentPhase::Build],
                }],
                craft_packs: Vec::new(),
                layout_guidance: Vec::new(),
                warnings: Vec::new(),
            },
        };
        manifest.payload.artifact_manifest_hash = crate::types::canonical_json_hash(
            &serde_json::to_value(&manifest.artifact_manifest).unwrap(),
        );
        manifest.content_hash =
            crate::types::canonical_json_hash(&serde_json::to_value(&manifest.payload).unwrap());
        run.design_profile_id = Some(manifest.payload.design_profile_id.clone());
        run.design_profile_version = Some(manifest.payload.design_profile_version);
        run.design_profile_hash = Some(manifest.payload.base_profile_hash.clone());
        run.design_profile_effective_hash = Some(manifest.payload.effective_profile_hash.clone());
        run.design_profile_surface = Some(manifest.payload.surface.clone());
        run.design_profile_template = Some(manifest.payload.template.clone());
        run.design_context_package_version = Some(manifest.payload.schema_version.clone());
        run.design_context_content_hash = Some(manifest.content_hash.clone());
        run.design_context_artifact_manifest_hash =
            Some(manifest.payload.artifact_manifest_hash.clone());
        run.design_context_compiler_version = Some(manifest.payload.compiler_version.clone());
        run.design_context_brief_hash = Some(manifest.payload.brief_hash.clone());
        run.design_context_verification_policy_id =
            Some(manifest.payload.verification_policy.policy_id.clone());
        run.design_context_expected_app_root = Some(manifest.payload.expected_app_root.clone());
        run.design_context_declared_enforcement_mode = Some("observe".to_string());
        run.design_context_effective_compatibility_mode = Some("observe".to_string());
        run.design_context_warnings = manifest.payload.warnings.clone();
        run.design_context_verification_environment = Some(json!({
            "registryVersion": "runtime-verifier-registry@1",
            "capabilitySnapshotHash": "capability-snapshot",
            "browserExecutable": "/private/runtime-browser",
            "browserCollectorExecutable": "/private/runtime-collector",
            "capabilities": {
                "dom": {
                    "available": true,
                    "detail": "private capability diagnostic"
                }
            }
        }));
        run.design_context_manifest = Some(serde_json::to_value(&manifest).unwrap());
        run.design_context_artifacts
            .insert("inputs/design-profile.json".to_string(), profile_text);

        let status = status_payload(&run).unwrap();
        assert_eq!(status["gate"], "read_required");
        assert_eq!(
            status["verification"]["capabilities"]["dom"]["available"],
            true
        );
        let serialized_status = status.to_string();
        for private_value in [
            "secret profile text",
            "/private/runtime-browser",
            "/private/runtime-collector",
            "private capability diagnostic",
            "browserExecutable",
            "browserCollectorExecutable",
            "environment",
        ] {
            assert!(
                !serialized_status.contains(private_value),
                "{private_value}"
            );
        }

        let mut mismatched = run.clone();
        mismatched.design_profile_id = Some("other-profile".to_string());
        match status_payload(&mismatched).unwrap_err() {
            ToolError::RecoverableWithMetadata { error_kind, .. } => {
                assert_eq!(error_kind, "design_context.integrity_failed");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
