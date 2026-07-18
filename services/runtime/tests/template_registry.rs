use anydesign_runtime::{
    artifact_manifest::{ArtifactManifest, ArtifactResolver, ARTIFACT_MANIFEST_FILE},
    artifact_publisher::{ArtifactPublisher, FileArtifactPublisher},
    conversation::RuntimeStore,
    model_gateway::ToolCall,
    project::{
        audit_project_template_compatibility, BuiltInTemplateAvailabilityService,
        SandboxExecutionProfileReadiness, TemplateAvailabilityService,
    },
    sandbox_profiles::{
        BuiltInSandboxExecutionProfileRegistry, SandboxExecutionProfile,
        SandboxExecutionProfileRegistry,
    },
    templates::{
        BuiltInTemplateRegistry, ManifestHash, RenderPageRequest, RenderedFile, TemplateId,
        TemplateOperationError, TemplateOperations, TemplateRegistry, TemplateRegistryBuildError,
        TemplateVersion,
    },
    tools::{
        runtime::ToolExecutor,
        sandbox::sandbox_tools,
        streaming::{tool_result_error_text, StreamingToolExecutor},
    },
    types::{AgentPhase, ProjectRuntimeState},
};
use async_trait::async_trait;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug)]
struct NeverReady;

struct ThirdTemplateOperations;

static THIRD_TEMPLATE_OPERATIONS: ThirdTemplateOperations = ThirdTemplateOperations;

impl TemplateOperations for ThirdTemplateOperations {
    fn name(&self) -> &'static str {
        "third-template"
    }

    fn supports_render_page(&self) -> bool {
        true
    }

    fn render_page(
        &self,
        request: &RenderPageRequest,
    ) -> Result<Vec<RenderedFile>, TemplateOperationError> {
        Ok(vec![RenderedFile {
            path: "src/pages/index.astro".to_string(),
            content: format!("<h1>{}</h1>", request.title),
        }])
    }
}

#[async_trait]
impl SandboxExecutionProfileReadiness for NeverReady {
    async fn is_ready(&self, _profile: &SandboxExecutionProfile) -> Result<bool, String> {
        Ok(false)
    }
}

#[test]
fn template_ids_are_open_but_strictly_validated() {
    assert_eq!(
        TemplateId::parse("nextjs-website").unwrap().as_str(),
        "nextjs-website"
    );
    assert!(TemplateId::parse("Next JS").is_err());
    assert!(TemplateId::parse("../astro").is_err());
    assert!(TemplateId::parse("").is_err());
}

#[test]
fn registry_resolves_exact_version_and_manifest_without_fallback() {
    let registry = BuiltInTemplateRegistry::built_in();
    let id = TemplateId::parse("astro-website").unwrap();
    let current = registry.current(&id).unwrap();

    assert_eq!(
        registry
            .resolve_version(&id, &current.version, &current.manifest_sha256)
            .unwrap(),
        current
    );
    assert!(registry
        .resolve_version(
            &id,
            &current.version,
            &ManifestHash::parse(
                "0000000000000000000000000000000000000000000000000000000000000000"
            )
            .unwrap(),
        )
        .is_err());
}

#[test]
fn compatibility_audit_rejects_missing_or_ambiguous_historical_specs() {
    let built_in = BuiltInTemplateRegistry::built_in();
    let id = TemplateId::parse("astro-website").unwrap();
    let current = built_in.current(&id).unwrap();
    let mut state: ProjectRuntimeState = serde_json::from_value(json!({
        "projectId": "persisted-project",
        "revision": 1,
        "appRoot": "project",
        "templateKey": id.as_str(),
        "templateVersion": current.version.as_str(),
        "templateManifestSha256": current.manifest_sha256.as_str(),
        "framework": "astro",
        "packageManager": "npm",
        "lockfile": "package-lock.json",
        "registry": "https://registry.internal.example/npm/",
        "updatedAt": "2026-07-11T00:00:00Z"
    }))
    .unwrap();
    assert!(audit_project_template_compatibility(&[state.clone()], &built_in).is_empty());

    state.template_manifest_sha256 =
        Some("0000000000000000000000000000000000000000000000000000000000000000".to_string());
    assert_eq!(
        audit_project_template_compatibility(&[state.clone()], &built_in)[0].error_kind,
        "template.version_incompatible"
    );

    let first = (*current).clone();
    let mut second = first.clone();
    second.version = TemplateVersion::parse("astro-website@runtime-p4").unwrap();
    second.manifest_sha256 =
        ManifestHash::parse("1111111111111111111111111111111111111111111111111111111111111111")
            .unwrap();
    let multi_version = BuiltInTemplateRegistry::new([first, second]).unwrap();
    state.template_manifest_sha256 = None;
    assert_eq!(
        audit_project_template_compatibility(&[state], &multi_version)[0].error_kind,
        "template.legacy_state_ambiguous"
    );
}

#[test]
fn registry_supports_multiple_versions_and_rejects_duplicate_pairs() {
    let built_in = BuiltInTemplateRegistry::built_in();
    let id = TemplateId::parse("astro-website").unwrap();
    let first = (*built_in.current(&id).unwrap()).clone();
    let mut second = first.clone();
    second.version = TemplateVersion::parse("astro-website@runtime-p4").unwrap();
    second.manifest_sha256 =
        ManifestHash::parse("1111111111111111111111111111111111111111111111111111111111111111")
            .unwrap();

    let registry = BuiltInTemplateRegistry::new([first.clone(), second.clone()]).unwrap();
    assert_eq!(registry.current(&id).unwrap().version, second.version);
    assert_eq!(registry.versions(&id).len(), 2);

    assert!(matches!(
        BuiltInTemplateRegistry::new([first.clone(), first]),
        Err(TemplateRegistryBuildError::DuplicateVersion { .. })
    ));
}

#[test]
fn third_template_registers_through_spec_and_operations_without_generic_dispatch() {
    let built_in = BuiltInTemplateRegistry::built_in();
    let mut third = (*built_in.default_template().unwrap()).clone();
    third.id = TemplateId::parse("third-template").unwrap();
    third.version = TemplateVersion::parse("third-template@1").unwrap();
    third.manifest_sha256 =
        ManifestHash::parse("2222222222222222222222222222222222222222222222222222222222222222")
            .unwrap();
    third.operations = &THIRD_TEMPLATE_OPERATIONS;
    let registry = BuiltInTemplateRegistry::new([third]).unwrap();
    let resolved = registry
        .current(&TemplateId::parse("third-template").unwrap())
        .unwrap();
    let files = resolved
        .operations
        .render_page(&RenderPageRequest {
            route: "/".to_string(),
            title: "Third".to_string(),
            style_profile: "saas".to_string(),
            sections: vec![],
        })
        .unwrap();
    assert_eq!(files[0].content, "<h1>Third</h1>");

    let mut inconsistent = (*resolved).clone();
    inconsistent.capabilities.structured_page_write = false;
    assert!(matches!(
        BuiltInTemplateRegistry::new([inconsistent]),
        Err(TemplateRegistryBuildError::CapabilityOperationMismatch {
            operation: "render_page",
            ..
        })
    ));
}

#[tokio::test]
async fn third_template_publishes_through_the_generic_artifact_contract() {
    let built_in = BuiltInTemplateRegistry::built_in();
    let mut third = (*built_in.default_template().unwrap()).clone();
    third.id = TemplateId::parse("third-template").unwrap();
    third.version = TemplateVersion::parse("third-template@1").unwrap();
    third.manifest_sha256 =
        ManifestHash::parse("2222222222222222222222222222222222222222222222222222222222222222")
            .unwrap();
    third.operations = &THIRD_TEMPLATE_OPERATIONS;
    let registry = BuiltInTemplateRegistry::new([third]).unwrap();
    let resolved = registry
        .current(&TemplateId::parse("third-template").unwrap())
        .unwrap();

    let workspace = unique_temp_dir("third-artifact");
    let output = workspace.join("output");
    let storage = workspace.join("runtime");
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("index.html"), "<h1>Third artifact</h1>").unwrap();
    let publisher = FileArtifactPublisher::new(&storage);
    let staged = publisher
        .stage_directory(
            "third-project",
            "third-version",
            &"a".repeat(64),
            &output,
            &resolved,
        )
        .await
        .unwrap();
    publisher.promote(&staged).await.unwrap();

    let immutable = FileArtifactPublisher::version_root(&storage, "third-project", "third-version");
    let manifest: ArtifactManifest =
        serde_json::from_slice(&fs::read(immutable.join(ARTIFACT_MANIFEST_FILE)).unwrap()).unwrap();
    assert_eq!(manifest.template_id, "third-template");
    assert_eq!(manifest.mounts[0].url_prefix, "/");
    let artifact = ArtifactResolver::load(&immutable, &staged.artifact_manifest_hash)
        .unwrap()
        .unwrap()
        .resolve("")
        .unwrap()
        .unwrap();
    assert_eq!(artifact.bytes, b"<h1>Third artifact</h1>");
    fs::remove_dir_all(workspace).unwrap();
}

#[test]
fn legacy_project_runtime_state_deserializes_without_new_identity_fields() {
    let state: ProjectRuntimeState = serde_json::from_value(json!({
        "projectId": "legacy-project",
        "revision": 1,
        "appRoot": "project",
        "templateKey": "astro-website",
        "templateVersion": "astro-website@runtime-p3",
        "framework": "astro",
        "packageManager": "npm",
        "lockfile": "package-lock.json",
        "registry": "https://registry.internal.example/npm/",
        "updatedAt": "2026-07-11T00:00:00Z"
    }))
    .unwrap();

    assert_eq!(state.template_manifest_sha256, None);
    assert_eq!(state.sandbox_execution_profile_id, None);
    assert_eq!(state.sandbox_execution_profile_version, None);
    let serialized = serde_json::to_value(state).unwrap();
    assert!(serialized.get("templateManifestSha256").is_none());
    assert!(serialized.get("sandboxExecutionProfileId").is_none());
}

#[tokio::test]
async fn availability_distinguishes_unknown_disabled_and_unready_templates() {
    let astro = TemplateId::parse("astro-website").unwrap();
    let docs = TemplateId::parse("fumadocs-docs").unwrap();
    let service = BuiltInTemplateAvailabilityService::new(
        std::sync::Arc::new(BuiltInTemplateRegistry::built_in()),
        std::sync::Arc::new(BuiltInSandboxExecutionProfileRegistry::built_in()),
        std::sync::Arc::new(NeverReady),
        [astro.clone()],
    );

    assert_eq!(
        service
            .resolve_for_init(&docs)
            .await
            .unwrap_err()
            .error_kind(),
        "template.disabled"
    );
    assert_eq!(
        service
            .resolve_for_init(&TemplateId::parse("nextjs-website").unwrap())
            .await
            .unwrap_err()
            .error_kind(),
        "template.unsupported"
    );
    assert_eq!(
        service
            .resolve_for_init(&astro)
            .await
            .unwrap_err()
            .error_kind(),
        "template.execution_profile_unavailable"
    );
}

#[tokio::test]
async fn disabling_new_initialization_preserves_existing_project_version_resolution() {
    let registry = std::sync::Arc::new(BuiltInTemplateRegistry::built_in());
    let profiles = std::sync::Arc::new(BuiltInSandboxExecutionProfileRegistry::built_in());
    let id = TemplateId::parse("astro-website").unwrap();
    let existing = registry.current(&id).unwrap();
    let service = BuiltInTemplateAvailabilityService::new(
        registry.clone(),
        profiles,
        std::sync::Arc::new(anydesign_runtime::project::StaticSandboxExecutionProfileReadiness),
        [],
    );

    assert_eq!(
        service
            .resolve_for_init(&id)
            .await
            .unwrap_err()
            .error_kind(),
        "template.disabled"
    );
    assert_eq!(
        registry
            .resolve_version(&id, &existing.version, &existing.manifest_sha256)
            .unwrap(),
        existing
    );
}

#[test]
fn built_in_templates_resolve_explicit_warm_pool_profiles() {
    let templates = BuiltInTemplateRegistry::built_in();
    let profiles = BuiltInSandboxExecutionProfileRegistry::built_in();
    for (template, expected_pool) in [
        ("astro-website", "anydesign-astro-website-pool"),
        ("fumadocs-docs", "anydesign-fumadocs-docs-pool"),
    ] {
        let spec = templates
            .current(&TemplateId::parse(template).unwrap())
            .unwrap();
        assert_eq!(
            profiles
                .resolve(&spec.sandbox_execution_profile)
                .unwrap()
                .warm_pool_name,
            expected_pool
        );
    }
}

#[tokio::test]
async fn built_in_manifest_hashes_match_current_project_init_output() {
    let registry = BuiltInTemplateRegistry::built_in();
    for template in ["astro-website", "fumadocs-docs"] {
        let actual = initialize_and_hash(template).await;
        let expected = registry
            .current(&TemplateId::parse(template).unwrap())
            .unwrap()
            .manifest_sha256
            .clone();
        assert_eq!(actual, expected, "{template} manifest drifted");
    }
}

#[test]
fn fumadocs_template_pins_a_resolvable_yuku_analyzer_graph() {
    let registry = BuiltInTemplateRegistry::built_in();
    let template_id = TemplateId::parse("fumadocs-docs").unwrap();
    let spec = registry.current(&template_id).unwrap();
    assert_eq!(spec.version.as_str(), "fumadocs-docs@runtime-p5");
    let package_json = spec
        .files
        .iter()
        .find(|file| file.path == "package.json")
        .expect("fumadocs package.json");
    let package_lock = spec
        .files
        .iter()
        .find(|file| file.path == "package-lock.json")
        .expect("fumadocs package-lock.json");
    let manifest: serde_json::Value = serde_json::from_str(package_json.content).unwrap();
    let lock: serde_json::Value = serde_json::from_str(package_lock.content).unwrap();

    assert_eq!(
        manifest["overrides"]["yuku-analyzer"],
        serde_json::Value::String("0.6.5".to_string())
    );
    assert_eq!(
        lock["packages"]["node_modules/yuku-analyzer"]["version"],
        serde_json::Value::String("0.6.5".to_string())
    );
    assert!(lock["packages"].get("node_modules/yuku-ast").is_none());

    let mdx_components = spec
        .files
        .iter()
        .find(|file| file.path == "components/mdx.jsx")
        .expect("fumadocs MDX components");
    for compatibility_export in [
        "Steps.Step = Step",
        "Tabs.Tab = Tab",
        "CompatibleAccordions.Accordion = Accordion",
    ] {
        assert!(
            mdx_components.content.contains(compatibility_export),
            "missing compound MDX compatibility: {compatibility_export}"
        );
    }

    let legacy = registry
        .resolve_version(
            &template_id,
            &TemplateVersion::parse("fumadocs-docs@runtime-p3").unwrap(),
            &ManifestHash::parse(
                "753ce62ea481258e9620bafe2d5e53e31da2db7c037945f6266490cc0d1336e4",
            )
            .unwrap(),
        )
        .expect("runtime-p3 projects must remain resolvable");
    let legacy_manifest: serde_json::Value = serde_json::from_str(
        legacy
            .files
            .iter()
            .find(|file| file.path == "package.json")
            .expect("legacy fumadocs package.json")
            .content,
    )
    .unwrap();
    assert!(legacy_manifest.get("overrides").is_none());

    registry
        .resolve_version(
            &template_id,
            &TemplateVersion::parse("fumadocs-docs@runtime-p4").unwrap(),
            &ManifestHash::parse(
                "3fb0f309bb3ce8cc7044d21981bc72bde1938f95f904e2589b75c443f7143cd3",
            )
            .unwrap(),
        )
        .expect("runtime-p4 projects must remain resolvable");
}

async fn initialize_and_hash(template: &str) -> ManifestHash {
    let workspace = unique_temp_dir(template);
    for directory in ["project", "inputs", "state", "outputs"] {
        fs::create_dir_all(workspace.join(directory)).unwrap();
    }
    let store = RuntimeStore::new();
    let run = store
        .create_run(
            format!("template-contract-{template}"),
            AgentPhase::Build,
            "build".to_string(),
            "internal-balanced".to_string(),
            vec![],
        )
        .await;
    let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
        sandbox_tools(),
        Default::default(),
        &workspace,
    ));
    let results = executor
        .execute_calls(
            store,
            &run.id,
            vec![ToolCall::new(
                "init",
                "project.init",
                json!({ "template": template }),
            )],
        )
        .await;
    assert_eq!(results.len(), 1);
    assert!(
        !results[0].result.is_error,
        "{}",
        tool_result_error_text(&results[0].result)
    );

    let mut inventory = Vec::new();
    collect_files(
        &workspace.join("project"),
        &workspace.join("project"),
        &mut inventory,
    );
    inventory.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    for (path, bytes) in inventory {
        digest.update(path.as_bytes());
        digest.update([0]);
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(bytes);
    }
    fs::remove_dir_all(workspace).unwrap();
    ManifestHash::parse(format!("{:x}", digest.finalize())).unwrap()
}

fn collect_files(root: &Path, current: &Path, inventory: &mut Vec<(String, Vec<u8>)>) {
    for entry in fs::read_dir(current).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            collect_files(root, &entry.path(), inventory);
        } else {
            inventory.push((
                entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                fs::read(entry.path()).unwrap(),
            ));
        }
    }
}

fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "anydesign-template-registry-{prefix}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ))
}
