use crate::{
    templates::TemplateSpec,
    types::{
        canonical_json_bytes, canonical_json_hash, sha256_hex, AgentPhase, AgentRun, Brief,
        EffectiveDesignProfile,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::{path::Path, process::Command};

pub const DESIGN_CONTEXT_SCHEMA_V1: &str = "design-context@1";
pub const DESIGN_CONTEXT_ARTIFACT_SCHEMA_V1: &str = "design-context-artifacts@1";
pub const DESIGN_CONTEXT_MANIFEST_SCHEMA_V1: &str = "design-context-manifest@1";
pub const BRIEF_SCHEMA_V1: &str = "brief@1";
pub const DEFAULT_WEBSITE_APP_ROOT: &str = "project";
pub const DEFAULT_COMPILER_VERSION: &str = "design-context-compiler@1";
pub const DEFAULT_VERIFICATION_POLICY_ID: &str = "website-verification@1";
pub const DEFAULT_A11Y_RULESET_VERSION: &str = "accessibility-baseline@1";
pub const DEFAULT_VIEWPORT_MATRIX_ID: &str = "website@375-768-1440/v1";
pub const VERIFIER_REGISTRY_VERSION: &str = "runtime-verifier-registry@1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileEnforcementMode {
    Observe,
    Enforced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileCompatibilityMode {
    Observe,
    Enforced,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationPolicySnapshot {
    pub policy_id: String,
    pub a11y_ruleset_version: String,
    pub viewport_matrix_id: String,
    pub required_verifier_kinds: Vec<String>,
}

/// Runtime availability is deliberately outside the deterministic DCP payload.
/// It is captured on the run binding so replay can distinguish a policy from
/// the concrete verifier environment that executed (or rejected) it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerificationEnvironmentBinding {
    pub registry_version: String,
    pub capability_snapshot_hash: String,
    #[serde(default)]
    pub browser_executable: Option<String>,
    #[serde(default)]
    pub browser_collector_executable: Option<String>,
    pub capabilities: BTreeMap<String, VerifierCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifierCapability {
    pub available: bool,
    pub detail: String,
}

pub struct VerifierRegistry;

impl VerifierRegistry {
    pub fn discover() -> VerificationEnvironmentBinding {
        Self::discover_with_browser_executable(
            std::env::var("RUNTIME_BROWSER_EXECUTABLE")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .as_deref(),
        )
    }

    pub fn discover_with_browser_executable(
        browser_worker_path: Option<&str>,
    ) -> VerificationEnvironmentBinding {
        let browser_collector_executable = std::env::var("RUNTIME_BROWSER_COLLECTOR_EXECUTABLE")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "node".to_string());
        Self::discover_with_executables(
            browser_worker_path,
            Some(browser_collector_executable.as_str()),
        )
    }

    pub fn discover_with_executables(
        browser_worker_path: Option<&str>,
        browser_collector_executable: Option<&str>,
    ) -> VerificationEnvironmentBinding {
        let (browser_worker_available, browser_worker_detail) =
            probe_browser_worker(browser_worker_path);
        let (collector_available, collector_detail) = match browser_collector_executable {
            Some(executable) => match Command::new(executable).arg("--version").output() {
                Ok(output) if output.status.success() => {
                    let version = String::from_utf8_lossy(&output.stdout)
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ");
                    let version = version.chars().take(160).collect::<String>();
                    (
                        true,
                        format!(
                            "browser evidence collector runtime passed health probe: {executable}{}",
                            if version.is_empty() {
                                String::new()
                            } else {
                                format!(" ({version})")
                            }
                        ),
                    )
                }
                Ok(output) => (
                    false,
                    format!(
                        "RUNTIME_BROWSER_COLLECTOR_EXECUTABLE health probe failed (status {}): {executable}",
                        output.status
                    ),
                ),
                Err(error) => (
                    false,
                    format!(
                        "RUNTIME_BROWSER_COLLECTOR_EXECUTABLE health probe could not start {executable}: {error}"
                    ),
                ),
            },
            None => (
                false,
                "RUNTIME_BROWSER_COLLECTOR_EXECUTABLE is not configured".to_string(),
            ),
        };
        let browser_evidence_available = browser_worker_available && collector_available;
        let browser_evidence_detail = format!("{browser_worker_detail}; {collector_detail}");
        let mut capabilities = BTreeMap::new();
        capabilities.insert(
            "token".to_string(),
            VerifierCapability {
                available: true,
                detail: "style contract and token-file parser".to_string(),
            },
        );
        capabilities.insert(
            "dom".to_string(),
            VerifierCapability {
                available: true,
                detail: "runtime preview document selector evaluator".to_string(),
            },
        );
        capabilities.insert(
            "computed-style".to_string(),
            VerifierCapability {
                available: browser_evidence_available,
                detail: browser_evidence_detail.clone(),
            },
        );
        // The Runtime-owned browser evidence collector evaluates the fixed
        // a11y baseline and viewport matrix through the same configured
        // browser worker used for computed-style assertions.
        for kind in ["a11y", "viewport"] {
            capabilities.insert(
                kind.to_string(),
                VerifierCapability {
                    available: browser_evidence_available,
                    detail: browser_evidence_detail.clone(),
                },
            );
        }
        let capability_snapshot_hash = canonical_json_hash(&json!({
            "registryVersion": VERIFIER_REGISTRY_VERSION,
            "browserExecutable": browser_worker_path,
            "browserCollectorExecutable": browser_collector_executable,
            "capabilities": capabilities,
        }));
        VerificationEnvironmentBinding {
            registry_version: VERIFIER_REGISTRY_VERSION.to_string(),
            capability_snapshot_hash,
            browser_executable: browser_worker_path.map(ToString::to_string),
            browser_collector_executable: browser_collector_executable.map(ToString::to_string),
            capabilities,
        }
    }
}

fn probe_browser_worker(browser_worker_path: Option<&str>) -> (bool, String) {
    let Some(path) = browser_worker_path else {
        return (
            false,
            "RUNTIME_BROWSER_EXECUTABLE is not configured".to_string(),
        );
    };
    if !Path::new(path).is_file() {
        return (
            false,
            format!("RUNTIME_BROWSER_EXECUTABLE is not an executable file: {path}"),
        );
    }
    let version = match Command::new(path).arg("--version").output() {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(160)
            .collect::<String>(),
        Ok(output) => {
            return (
                false,
                format!(
                    "RUNTIME_BROWSER_EXECUTABLE version probe failed (status {}): {path}",
                    output.status
                ),
            )
        }
        Err(error) => {
            return (
                false,
                format!("RUNTIME_BROWSER_EXECUTABLE version probe could not start {path}: {error}"),
            )
        }
    };
    match Command::new(path)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-sandbox",
            "--no-first-run",
            "--no-default-browser-check",
            "--dump-dom",
            "about:blank",
        ])
        .output()
    {
        Ok(output) if output.status.success() => (
            true,
            format!(
                "configured Runtime browser worker passed container-isolated headless launch probe: {path}{}",
                if version.is_empty() {
                    String::new()
                } else {
                    format!(" ({version})")
                }
            ),
        ),
        Ok(output) => {
            let diagnostic = String::from_utf8_lossy(&output.stderr)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(500)
                .collect::<String>();
            (
                false,
                format!(
                    "RUNTIME_BROWSER_EXECUTABLE headless launch probe failed (status {}): {path}{}",
                    output.status,
                    if diagnostic.is_empty() {
                        String::new()
                    } else {
                        format!(" ({diagnostic})")
                    }
                ),
            )
        }
        Err(error) => (
            false,
            format!(
                "RUNTIME_BROWSER_EXECUTABLE headless launch probe could not start {path}: {error}"
            ),
        ),
    }
}

impl VerificationEnvironmentBinding {
    pub fn missing_required_verifiers(&self, policy: &VerificationPolicySnapshot) -> Vec<String> {
        policy
            .required_verifier_kinds
            .iter()
            .filter(|kind| {
                self.capabilities
                    .get(kind.as_str())
                    .is_none_or(|capability| !capability.available)
            })
            .cloned()
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignContextArtifact {
    pub path: String,
    pub kind: String,
    pub bytes: u64,
    pub sha256: String,
    pub required_before_mutation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignContextArtifactManifest {
    pub schema_version: String,
    pub artifacts: Vec<DesignContextArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignContextReadRequirement {
    pub path: String,
    pub reason: String,
    pub phases: Vec<AgentPhase>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignContextPackagePayload {
    pub schema_version: String,
    pub design_profile_id: String,
    pub design_profile_version: u32,
    pub base_profile_hash: String,
    pub effective_profile_hash: String,
    pub brief_hash: String,
    pub brief_schema_version: String,
    pub surface: String,
    pub template: String,
    pub template_manifest_sha256: String,
    pub expected_app_root: String,
    pub compiler_version: String,
    pub declared_enforcement_mode: ProfileEnforcementMode,
    pub effective_compatibility_mode: ProfileCompatibilityMode,
    pub verification_policy: VerificationPolicySnapshot,
    pub artifact_manifest_hash: String,
    pub resolved_runtime_tokens: BTreeMap<String, String>,
    pub resolved_token_snapshot_hash: String,
    pub required_reads: Vec<DesignContextReadRequirement>,
    pub craft_packs: Vec<String>,
    pub layout_guidance: Vec<Value>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesignContextManifest {
    pub schema_version: String,
    pub payload: DesignContextPackagePayload,
    pub content_hash: String,
    pub artifact_manifest: DesignContextArtifactManifest,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledDesignContext {
    pub manifest: DesignContextManifest,
    pub files: BTreeMap<String, String>,
}

pub fn frozen_run_design_context_manifest(
    run: &AgentRun,
) -> Result<Option<DesignContextManifest>, String> {
    let Some(value) = run.design_context_manifest.as_ref() else {
        return Ok(None);
    };
    let manifest = serde_json::from_value::<DesignContextManifest>(value.clone())
        .map_err(|error| format!("frozen Design Context manifest is invalid: {error}"))?;
    validate_run_design_context_identity(run, &manifest)?;
    Ok(Some(manifest))
}

pub fn validate_run_design_context_identity(
    run: &AgentRun,
    manifest: &DesignContextManifest,
) -> Result<(), String> {
    if manifest.schema_version != DESIGN_CONTEXT_MANIFEST_SCHEMA_V1 {
        return Err(format!(
            "unsupported manifest schema: {}",
            manifest.schema_version
        ));
    }
    if manifest.artifact_manifest.schema_version != DESIGN_CONTEXT_ARTIFACT_SCHEMA_V1 {
        return Err(format!(
            "unsupported artifact manifest schema: {}",
            manifest.artifact_manifest.schema_version
        ));
    }

    let computed_content_hash = canonical_json_hash(
        &serde_json::to_value(&manifest.payload).map_err(|error| error.to_string())?,
    );
    if manifest.content_hash != computed_content_hash {
        return Err("manifest contentHash does not match its payload".to_string());
    }
    let computed_artifact_manifest_hash = canonical_json_hash(
        &serde_json::to_value(&manifest.artifact_manifest).map_err(|error| error.to_string())?,
    );
    if manifest.payload.artifact_manifest_hash != computed_artifact_manifest_hash {
        return Err(
            "manifest artifactManifestHash does not match its artifact manifest".to_string(),
        );
    }

    if run.design_context_artifacts.len() != manifest.artifact_manifest.artifacts.len() {
        return Err(
            "run Design Context artifacts do not match the frozen artifact manifest".to_string(),
        );
    }
    let mut artifact_paths = BTreeSet::new();
    for artifact in &manifest.artifact_manifest.artifacts {
        if !artifact_paths.insert(artifact.path.as_str()) {
            return Err(format!(
                "frozen artifact manifest contains duplicate path: {}",
                artifact.path
            ));
        }
        let content = run
            .design_context_artifacts
            .get(&artifact.path)
            .ok_or_else(|| format!("run is missing frozen artifact: {}", artifact.path))?;
        if artifact.bytes != content.len() as u64
            || artifact.sha256 != sha256_hex(content.as_bytes())
        {
            return Err(format!(
                "run frozen artifact does not match its manifest metadata: {}",
                artifact.path
            ));
        }
    }

    let payload = &manifest.payload;
    let profile_text = run
        .design_context_artifacts
        .get("inputs/design-profile.json")
        .ok_or_else(|| "run is missing inputs/design-profile.json".to_string())?;
    let profile_value: Value = serde_json::from_str(profile_text)
        .map_err(|error| format!("frozen DesignProfile artifact is invalid: {error}"))?;
    if canonical_json_hash(&profile_value) != payload.effective_profile_hash {
        return Err(
            "frozen DesignProfile artifact does not match effectiveProfileHash".to_string(),
        );
    }
    if profile_value.get("id").and_then(Value::as_str) != Some(payload.design_profile_id.as_str())
        || profile_value.get("version").and_then(Value::as_u64)
            != Some(payload.design_profile_version as u64)
    {
        return Err(
            "frozen DesignProfile artifact identity does not match the manifest".to_string(),
        );
    }
    if profile_value
        .get("scope")
        .and_then(|scope| scope.get("projectId"))
        .and_then(Value::as_str)
        .is_some_and(|project_id| project_id != run.project_id)
    {
        return Err("frozen DesignProfile artifact is not visible to the run project".to_string());
    }

    let declared_enforcement_mode = match payload.declared_enforcement_mode {
        ProfileEnforcementMode::Observe => "observe",
        ProfileEnforcementMode::Enforced => "enforced",
    };
    let effective_compatibility_mode = match payload.effective_compatibility_mode {
        ProfileCompatibilityMode::Observe => "observe",
        ProfileCompatibilityMode::Enforced => "enforced",
    };
    for (field, actual, expected) in [
        (
            "designProfileId",
            run.design_profile_id.as_deref(),
            payload.design_profile_id.as_str(),
        ),
        (
            "baseProfileHash",
            run.design_profile_hash.as_deref(),
            payload.base_profile_hash.as_str(),
        ),
        (
            "effectiveProfileHash",
            run.design_profile_effective_hash.as_deref(),
            payload.effective_profile_hash.as_str(),
        ),
        (
            "surface",
            run.design_profile_surface.as_deref(),
            payload.surface.as_str(),
        ),
        (
            "template",
            run.design_profile_template.as_deref(),
            payload.template.as_str(),
        ),
        (
            "packageVersion",
            run.design_context_package_version.as_deref(),
            payload.schema_version.as_str(),
        ),
        (
            "contentHash",
            run.design_context_content_hash.as_deref(),
            manifest.content_hash.as_str(),
        ),
        (
            "artifactManifestHash",
            run.design_context_artifact_manifest_hash.as_deref(),
            payload.artifact_manifest_hash.as_str(),
        ),
        (
            "compilerVersion",
            run.design_context_compiler_version.as_deref(),
            payload.compiler_version.as_str(),
        ),
        (
            "briefHash",
            run.design_context_brief_hash.as_deref(),
            payload.brief_hash.as_str(),
        ),
        (
            "verificationPolicyId",
            run.design_context_verification_policy_id.as_deref(),
            payload.verification_policy.policy_id.as_str(),
        ),
        (
            "expectedAppRoot",
            run.design_context_expected_app_root.as_deref(),
            payload.expected_app_root.as_str(),
        ),
        (
            "declaredEnforcementMode",
            run.design_context_declared_enforcement_mode.as_deref(),
            declared_enforcement_mode,
        ),
        (
            "effectiveCompatibilityMode",
            run.design_context_effective_compatibility_mode.as_deref(),
            effective_compatibility_mode,
        ),
    ] {
        if actual != Some(expected) {
            return Err(format!(
                "run {field} does not match the frozen Design Context manifest"
            ));
        }
    }
    if run.design_profile_version != Some(payload.design_profile_version) {
        return Err(
            "run designProfileVersion does not match the frozen Design Context manifest"
                .to_string(),
        );
    }
    if run.design_context_warnings != payload.warnings {
        return Err("run warnings do not match the frozen Design Context manifest".to_string());
    }
    if run
        .design_context_materialization_hash
        .as_deref()
        .is_some_and(|hash| hash != payload.artifact_manifest_hash)
    {
        return Err(
            "run materializationHash does not match the frozen artifact manifest".to_string(),
        );
    }
    if run.design_context_style_contract_verified == Some(true)
        && run.design_context_materialization_hash.is_none()
    {
        return Err("run cannot verify the style contract before DCP materialization".to_string());
    }
    if let Some(binding) = run.design_context_enforcement_binding.as_ref() {
        match binding.source.as_str() {
            "persistent" => {
                if binding.policy_revision.is_none_or(|revision| revision == 0)
                    || binding
                        .policy_updated_by
                        .as_deref()
                        .is_none_or(str::is_empty)
                {
                    return Err(
                        "persistent enforcement binding requires revision and updatedBy"
                            .to_string(),
                    );
                }
            }
            "config" => {
                if binding.policy_revision.is_some() || binding.policy_updated_by.is_some() {
                    return Err(
                        "config enforcement binding cannot claim a persistent policy revision"
                            .to_string(),
                    );
                }
            }
            _ => return Err("unsupported enforcement binding source".to_string()),
        }
        if payload.effective_compatibility_mode == ProfileCompatibilityMode::Enforced
            && !binding.enabled
        {
            return Err("enforced DCP cannot have a disabled rollout binding".to_string());
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct DesignContextCompileOptions {
    pub expected_app_root: String,
    pub compiler_version: String,
    pub enforcement_enabled: bool,
    pub verification_policy: VerificationPolicySnapshot,
}

impl Default for DesignContextCompileOptions {
    fn default() -> Self {
        Self {
            expected_app_root: DEFAULT_WEBSITE_APP_ROOT.to_string(),
            compiler_version: DEFAULT_COMPILER_VERSION.to_string(),
            enforcement_enabled: false,
            verification_policy: VerificationPolicySnapshot {
                policy_id: DEFAULT_VERIFICATION_POLICY_ID.to_string(),
                a11y_ruleset_version: DEFAULT_A11Y_RULESET_VERSION.to_string(),
                viewport_matrix_id: DEFAULT_VIEWPORT_MATRIX_ID.to_string(),
                required_verifier_kinds: vec![
                    "token".to_string(),
                    "dom".to_string(),
                    "computed-style".to_string(),
                    "a11y".to_string(),
                    "viewport".to_string(),
                ],
            },
        }
    }
}

pub fn compile_website_design_context(
    effective_profile: &EffectiveDesignProfile,
    brief: &Brief,
    template: &TemplateSpec,
    options: &DesignContextCompileOptions,
) -> Result<CompiledDesignContext, String> {
    if effective_profile.surface != "website" || template.surface != "website" {
        return Err("design context compiler only supports website surface".to_string());
    }
    if effective_profile.template != template.id.as_str() {
        return Err(format!(
            "effective profile template {} does not match template {}",
            effective_profile.template, template.id
        ));
    }
    if options.expected_app_root.trim().is_empty()
        || options.expected_app_root.starts_with('/')
        || options.expected_app_root.contains("..")
    {
        return Err("expectedAppRoot must be a safe workspace-relative path".to_string());
    }

    let declared_enforcement_mode = declared_enforcement_mode(&effective_profile.profile)?;
    let effective_compatibility_mode = if options.enforcement_enabled
        && declared_enforcement_mode == ProfileEnforcementMode::Enforced
    {
        ProfileCompatibilityMode::Enforced
    } else {
        ProfileCompatibilityMode::Observe
    };
    let (resolved_runtime_tokens, mut warnings) =
        resolve_runtime_tokens(&effective_profile.profile, template)?;
    let resolved_token_snapshot_hash = canonical_json_hash(&json!(resolved_runtime_tokens));
    let (recipes, recipe_warnings) = component_recipes(&effective_profile.profile, template)?;
    warnings.extend(recipe_warnings);
    let (craft_packs, craft_pack_warnings) = craft_packs(&effective_profile.profile, template)?;
    warnings.extend(craft_pack_warnings);
    let layout_guidance = layout_guidance(&effective_profile.profile)?;

    if declared_enforcement_mode == ProfileEnforcementMode::Enforced
        && effective_compatibility_mode == ProfileCompatibilityMode::Observe
    {
        warnings.push(
            "Profile declares enforcementMode=enforced but runtime enforcement is disabled"
                .to_string(),
        );
    }

    let style_contract = template.style.render(
        &template.id,
        std::path::Path::new(&options.expected_app_root),
    );
    let profile_bytes = canonical_json_bytes(&effective_profile.profile);
    let recipes_bytes = canonical_json_bytes(&Value::Array(recipes));
    let style_contract_bytes = canonical_json_bytes(&style_contract);
    let usage = render_usage(
        &options.expected_app_root,
        declared_enforcement_mode,
        effective_compatibility_mode,
    );

    let mut files = BTreeMap::new();
    files.insert(
        "inputs/design-profile.json".to_string(),
        String::from_utf8(profile_bytes).map_err(|error| error.to_string())?,
    );
    files.insert(
        "inputs/component-recipes.json".to_string(),
        String::from_utf8(recipes_bytes).map_err(|error| error.to_string())?,
    );
    files.insert(
        "inputs/template-style-contract.json".to_string(),
        String::from_utf8(style_contract_bytes).map_err(|error| error.to_string())?,
    );
    files.insert("inputs/design-profile-usage.md".to_string(), usage);

    let artifact_manifest = artifact_manifest(&files)?;
    let artifact_manifest_hash = canonical_json_hash(
        &serde_json::to_value(&artifact_manifest).map_err(|error| error.to_string())?,
    );
    let payload = DesignContextPackagePayload {
        schema_version: DESIGN_CONTEXT_SCHEMA_V1.to_string(),
        design_profile_id: effective_profile.design_profile_id.clone(),
        design_profile_version: effective_profile.version,
        base_profile_hash: effective_profile.base_profile_hash.clone(),
        effective_profile_hash: effective_profile.effective_profile_hash.clone(),
        brief_hash: canonical_json_hash(
            &serde_json::to_value(brief).map_err(|error| error.to_string())?,
        ),
        brief_schema_version: BRIEF_SCHEMA_V1.to_string(),
        surface: effective_profile.surface.clone(),
        template: effective_profile.template.clone(),
        template_manifest_sha256: template.manifest_sha256.as_str().to_string(),
        expected_app_root: options.expected_app_root.clone(),
        compiler_version: options.compiler_version.clone(),
        declared_enforcement_mode,
        effective_compatibility_mode,
        verification_policy: options.verification_policy.clone(),
        artifact_manifest_hash,
        resolved_runtime_tokens,
        resolved_token_snapshot_hash,
        required_reads: required_reads(),
        craft_packs,
        layout_guidance,
        warnings,
    };
    let content_hash =
        canonical_json_hash(&serde_json::to_value(&payload).map_err(|error| error.to_string())?);
    Ok(CompiledDesignContext {
        manifest: DesignContextManifest {
            schema_version: DESIGN_CONTEXT_MANIFEST_SCHEMA_V1.to_string(),
            payload,
            content_hash,
            artifact_manifest,
        },
        files,
    })
}

pub fn verify_materialization(
    compiled: &CompiledDesignContext,
    files: &BTreeMap<String, String>,
) -> Result<String, String> {
    let actual = artifact_manifest(files)?;
    if actual != compiled.manifest.artifact_manifest {
        return Err(
            "design context materialization does not match expected artifact manifest".to_string(),
        );
    }
    Ok(canonical_json_hash(
        &serde_json::to_value(&actual).map_err(|error| error.to_string())?,
    ))
}

fn artifact_manifest(
    files: &BTreeMap<String, String>,
) -> Result<DesignContextArtifactManifest, String> {
    let mut artifacts = files
        .iter()
        .map(|(path, content)| {
            Ok::<_, String>(DesignContextArtifact {
                path: path.clone(),
                kind: artifact_kind(path)?.to_string(),
                bytes: content.len() as u64,
                sha256: sha256_hex(content.as_bytes()),
                required_before_mutation: true,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    artifacts.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(DesignContextArtifactManifest {
        schema_version: DESIGN_CONTEXT_ARTIFACT_SCHEMA_V1.to_string(),
        artifacts,
    })
}

fn artifact_kind(path: &str) -> Result<&'static str, String> {
    match path {
        "inputs/design-profile.json" => Ok("profile"),
        "inputs/component-recipes.json" => Ok("component_recipes"),
        "inputs/template-style-contract.json" => Ok("template_style_contract"),
        "inputs/design-profile-usage.md" => Ok("usage"),
        _ => Err(format!("unexpected design context artifact path {path}")),
    }
}

fn declared_enforcement_mode(profile: &Value) -> Result<ProfileEnforcementMode, String> {
    let Some(value) = profile
        .get("websiteContext")
        .and_then(|context| context.get("enforcementMode"))
    else {
        return Ok(ProfileEnforcementMode::Observe);
    };
    match value.as_str() {
        Some("observe") => Ok(ProfileEnforcementMode::Observe),
        Some("enforced") => Ok(ProfileEnforcementMode::Enforced),
        Some(_) => Err("websiteContext.enforcementMode must be observe or enforced".to_string()),
        None => Err("websiteContext.enforcementMode must be a string".to_string()),
    }
}

fn resolve_runtime_tokens(
    profile: &Value,
    template: &TemplateSpec,
) -> Result<(BTreeMap<String, String>, Vec<String>), String> {
    let supported = template
        .style
        .tokens
        .iter()
        .map(|token| token.name)
        .collect::<BTreeSet<_>>();
    let runtime = profile
        .get("runtimeTokenMapping")
        .and_then(Value::as_object)
        .ok_or_else(|| "effective profile runtimeTokenMapping must be an object".to_string())?;
    let extended = profile
        .get("extendedTokenMapping")
        .and_then(Value::as_object);
    let mut resolved = BTreeMap::new();
    let mut warnings = Vec::new();
    for (name, value) in runtime {
        let value = value
            .as_str()
            .ok_or_else(|| format!("runtimeTokenMapping.{name} must be a string"))?;
        if !supported.contains(name.as_str()) {
            return Err(format!(
                "runtimeTokenMapping.{name} is unsupported by template {}",
                template.id
            ));
        }
        resolved.insert(name.clone(), value.to_string());
    }
    if let Some(extended) = extended {
        for (name, value) in extended {
            let value = value
                .as_str()
                .ok_or_else(|| format!("extendedTokenMapping.{name} must be a string"))?;
            if supported.contains(name.as_str()) {
                resolved.insert(name.clone(), value.to_string());
            } else {
                warnings.push(format!(
                    "extended token {name} is unsupported by template {}",
                    template.id
                ));
            }
        }
    }
    Ok((resolved, warnings))
}

fn component_recipes(
    profile: &Value,
    template: &TemplateSpec,
) -> Result<(Vec<Value>, Vec<String>), String> {
    let Some(recipes) = profile
        .get("components")
        .and_then(|components| components.get("recipes"))
    else {
        return Ok((Vec::new(), Vec::new()));
    };
    let recipes = recipes
        .as_array()
        .ok_or_else(|| "components.recipes must be an array".to_string())?;
    let mut warnings = Vec::new();
    for recipe in recipes {
        let priority = recipe
            .get("priority")
            .and_then(Value::as_str)
            .unwrap_or("preferred");
        if priority == "required"
            && recipe
                .get("verification")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
        {
            return Err("required component recipe must declare verification".to_string());
        }
        if let Some(role) = recipe.get("role").and_then(Value::as_str) {
            if !template.capabilities.supports_component_role(role) {
                if priority == "required" {
                    return Err(format!(
                        "required component recipe role {role} is unsupported by template {}",
                        template.id
                    ));
                }
                warnings.push(format!(
                    "preferred component recipe role {role} is unsupported by template {}",
                    template.id
                ));
            }
        }
    }
    Ok((recipes.clone(), warnings))
}

fn craft_packs(
    profile: &Value,
    template: &TemplateSpec,
) -> Result<(Vec<String>, Vec<String>), String> {
    let Some(packs) = profile
        .get("websiteContext")
        .and_then(|context| context.get("craftPacks"))
    else {
        return Ok((Vec::new(), Vec::new()));
    };
    let packs = packs
        .as_array()
        .ok_or_else(|| "websiteContext.craftPacks must be an array".to_string())?;
    let mut result = BTreeSet::new();
    let mut warnings = Vec::new();
    for pack in packs {
        let pack = pack
            .as_str()
            .ok_or_else(|| "websiteContext.craftPacks entries must be strings".to_string())?;
        if pack.trim().is_empty() {
            return Err("websiteContext.craftPacks entries must not be empty".to_string());
        }
        if template.capabilities.supports_craft_pack(pack) {
            result.insert(pack.to_string());
        } else {
            warnings.push(format!(
                "craft pack {pack} is unsupported by template {}",
                template.id
            ));
        }
    }
    Ok((result.into_iter().collect(), warnings))
}

fn layout_guidance(profile: &Value) -> Result<Vec<Value>, String> {
    let Some(guidance) = profile
        .get("websiteContext")
        .and_then(|context| context.get("layoutGuidance"))
    else {
        return Ok(Vec::new());
    };
    guidance
        .as_array()
        .cloned()
        .ok_or_else(|| "websiteContext.layoutGuidance must be an array".to_string())
}

fn required_reads() -> Vec<DesignContextReadRequirement> {
    vec![
        DesignContextReadRequirement {
            path: "inputs/brief.md".to_string(),
            reason: "brief".to_string(),
            phases: vec![AgentPhase::Build],
        },
        DesignContextReadRequirement {
            path: "inputs/design-profile.json".to_string(),
            reason: "profile".to_string(),
            phases: vec![AgentPhase::Build, AgentPhase::Edit],
        },
        DesignContextReadRequirement {
            path: "inputs/design-profile-usage.md".to_string(),
            reason: "usage".to_string(),
            phases: vec![AgentPhase::Build, AgentPhase::Edit, AgentPhase::Repair],
        },
        DesignContextReadRequirement {
            path: "inputs/component-recipes.json".to_string(),
            reason: "component_recipe".to_string(),
            phases: vec![AgentPhase::Build, AgentPhase::Edit, AgentPhase::Repair],
        },
        DesignContextReadRequirement {
            path: "inputs/template-style-contract.json".to_string(),
            reason: "bootstrap_token_contract".to_string(),
            phases: vec![AgentPhase::Build],
        },
    ]
}

fn render_usage(
    expected_app_root: &str,
    declared: ProfileEnforcementMode,
    effective: ProfileCompatibilityMode,
) -> String {
    format!(
        "# Design Context Usage\n\n- Expected app root: `{expected_app_root}`\n- Declared enforcement: `{}`\n- Effective compatibility mode: `{}`\n- Before project.init, read the required bootstrap files from `state/design-context-manifest.json`.\n- After project.init, read `state/style-contract.json` before mutating source or publishing.\n- Use only token names declared by the style contract.\n",
        enforcement_name(declared),
        compatibility_name(effective)
    )
}

fn enforcement_name(mode: ProfileEnforcementMode) -> &'static str {
    match mode {
        ProfileEnforcementMode::Observe => "observe",
        ProfileEnforcementMode::Enforced => "enforced",
    }
}

fn compatibility_name(mode: ProfileCompatibilityMode) -> &'static str {
    match mode {
        ProfileCompatibilityMode::Observe => "observe",
        ProfileCompatibilityMode::Enforced => "enforced",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry};

    fn template() -> std::sync::Arc<TemplateSpec> {
        BuiltInTemplateRegistry::built_in()
            .current(&TemplateId::parse("astro-website").unwrap())
            .unwrap()
    }

    fn brief() -> Brief {
        Brief {
            project_type: "website".to_string(),
            audience: "operators".to_string(),
            content_hierarchy: vec!["Hero".to_string()],
            page_structure: json!({ "sections": ["hero"] }),
            visual_direction: "calm".to_string(),
            recommended_template: "astro-website".to_string(),
            assumptions: Vec::new(),
            missing_information: Vec::new(),
        }
    }

    fn profile() -> EffectiveDesignProfile {
        EffectiveDesignProfile {
            design_profile_id: "dp-1".to_string(),
            version: 2,
            surface: "website".to_string(),
            template: "astro-website".to_string(),
            base_profile_hash: "a".repeat(64),
            surface_override_hash: None,
            template_override_hash: None,
            effective_profile_hash: "b".repeat(64),
            profile: json!({
                "runtimeTokenMapping": {
                    "color.primary": "#3456aa",
                    "color.background": "#ffffff"
                },
                "extendedTokenMapping": { "unsupported.token": "1px" },
                "components": {
                    "recipes": [{
                        "id": "button.primary",
                        "priority": "required",
                        "verification": [{ "kind": "dom", "selector": ".button-primary" }]
                    }]
                },
                "websiteContext": {
                    "enforcementMode": "enforced",
                    "craftPacks": ["responsive-layout", "accessibility-baseline"],
                    "layoutGuidance": [{ "area": "hero", "priority": "preferred" }]
                }
            }),
        }
    }

    #[test]
    fn compiler_is_deterministic_and_avoids_self_hashing() {
        let options = DesignContextCompileOptions::default();
        let first =
            compile_website_design_context(&profile(), &brief(), &template(), &options).unwrap();
        let second =
            compile_website_design_context(&profile(), &brief(), &template(), &options).unwrap();
        assert_eq!(first.manifest.content_hash, second.manifest.content_hash);
        assert_eq!(
            first.manifest.artifact_manifest,
            second.manifest.artifact_manifest
        );
        assert!(serde_json::to_value(&first.manifest.payload)
            .unwrap()
            .get("contentHash")
            .is_none());
        assert!(first
            .manifest
            .artifact_manifest
            .artifacts
            .iter()
            .all(|artifact| artifact.path != "state/design-context-manifest.json"));
    }

    #[test]
    fn brief_and_enforcement_change_the_content_hash() {
        let mut changed_brief = brief();
        changed_brief.audience = "designers".to_string();
        let observe = compile_website_design_context(
            &profile(),
            &brief(),
            &template(),
            &DesignContextCompileOptions::default(),
        )
        .unwrap();
        let changed = compile_website_design_context(
            &profile(),
            &changed_brief,
            &template(),
            &DesignContextCompileOptions::default(),
        )
        .unwrap();
        let enforced = compile_website_design_context(
            &profile(),
            &brief(),
            &template(),
            &DesignContextCompileOptions {
                enforcement_enabled: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_ne!(observe.manifest.content_hash, changed.manifest.content_hash);
        assert_ne!(
            observe.manifest.content_hash,
            enforced.manifest.content_hash
        );
        assert_eq!(
            observe.manifest.payload.effective_compatibility_mode,
            ProfileCompatibilityMode::Observe
        );
        assert_eq!(
            enforced.manifest.payload.effective_compatibility_mode,
            ProfileCompatibilityMode::Enforced
        );
    }

    #[test]
    fn materialization_must_match_the_expected_files() {
        let compiled = compile_website_design_context(
            &profile(),
            &brief(),
            &template(),
            &DesignContextCompileOptions::default(),
        )
        .unwrap();
        assert!(verify_materialization(&compiled, &compiled.files).is_ok());
        let mut tampered = compiled.files.clone();
        tampered.insert(
            "inputs/design-profile-usage.md".to_string(),
            "tampered".to_string(),
        );
        assert!(verify_materialization(&compiled, &tampered).is_err());
    }

    #[test]
    fn verifier_environment_reports_missing_policy_kinds_deterministically() {
        let environment = VerificationEnvironmentBinding {
            registry_version: VERIFIER_REGISTRY_VERSION.to_string(),
            capability_snapshot_hash: "snapshot".to_string(),
            browser_executable: None,
            browser_collector_executable: None,
            capabilities: BTreeMap::from([
                (
                    "token".to_string(),
                    VerifierCapability {
                        available: true,
                        detail: "available".to_string(),
                    },
                ),
                (
                    "viewport".to_string(),
                    VerifierCapability {
                        available: false,
                        detail: "not deployed".to_string(),
                    },
                ),
            ]),
        };
        let policy = VerificationPolicySnapshot {
            policy_id: "policy".to_string(),
            a11y_ruleset_version: "a11y".to_string(),
            viewport_matrix_id: "viewport".to_string(),
            required_verifier_kinds: vec![
                "token".to_string(),
                "viewport".to_string(),
                "unknown".to_string(),
            ],
        };
        assert_eq!(
            environment.missing_required_verifiers(&policy),
            vec!["viewport".to_string(), "unknown".to_string()]
        );
    }

    #[test]
    fn verifier_registry_marks_an_unhealthy_browser_worker_unavailable() {
        let environment =
            VerifierRegistry::discover_with_browser_executable(Some("/definitely-not-a-browser"));
        for kind in ["computed-style", "a11y", "viewport"] {
            let capability = environment.capabilities.get(kind).unwrap();
            assert!(!capability.available, "{kind} unexpectedly available");
            assert!(capability.detail.contains("not an executable file"));
        }
    }

    #[test]
    fn verifier_registry_requires_the_browser_collector_runtime() {
        let environment = VerifierRegistry::discover_with_executables(
            Some("/bin/true"),
            Some("/definitely-not-a-browser-collector"),
        );
        assert_eq!(
            environment.browser_collector_executable.as_deref(),
            Some("/definitely-not-a-browser-collector")
        );
        for kind in ["computed-style", "a11y", "viewport"] {
            let capability = environment.capabilities.get(kind).unwrap();
            assert!(!capability.available, "{kind} unexpectedly available");
            assert!(capability
                .detail
                .contains("RUNTIME_BROWSER_COLLECTOR_EXECUTABLE health probe could not start"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn verifier_registry_requires_a_real_headless_browser_launch() {
        use std::os::unix::fs::PermissionsExt;

        let executable = std::env::temp_dir().join(format!(
            "fake-browser-health-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        std::fs::write(
            &executable,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo fake-browser; exit 0; fi\necho headless-launch-failed >&2\nexit 1\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let environment =
            VerifierRegistry::discover_with_executables(executable.to_str(), Some("/bin/true"));
        let _ = std::fs::remove_file(executable);

        for kind in ["computed-style", "a11y", "viewport"] {
            let capability = environment.capabilities.get(kind).unwrap();
            assert!(!capability.available, "{kind} unexpectedly available");
            assert!(capability.detail.contains("headless launch probe failed"));
            assert!(capability.detail.contains("headless-launch-failed"));
        }
    }

    #[test]
    fn required_recipe_needs_verification() {
        let mut profile = profile();
        profile.profile["components"]["recipes"][0]["verification"] = json!([]);
        assert!(compile_website_design_context(
            &profile,
            &brief(),
            &template(),
            &DesignContextCompileOptions::default(),
        )
        .unwrap_err()
        .contains("required component recipe"));
    }
}
