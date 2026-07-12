use anydesign_runtime::{
    artifact_manifest::{
        manifest_file, ArtifactDeliverySpec, ArtifactManifest, ARTIFACT_MANIFEST_FILE,
    },
    release::{
        ProcessReleasePackagingBackend, ReleasePackager, ReleasePackagingInput, ReleaseStore,
        RuntimeProfile,
    },
    types::sha256_hex,
};
use anyhow::{Context, Result};
use serde_json::json;
use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

const BASE_IMAGE_DIGEST: &str =
    "sha256:53cadfbebeffa241f12333cf8a63f3c6553eedad8b9f8296de89e32c566a5caa";

#[tokio::main]
async fn main() -> Result<()> {
    let output_root = absolute(
        env::args()
            .nth(1)
            .unwrap_or_else(|| "target/release-packaging-g4".to_string()),
    )?;
    let helper = absolute(
        env::var("ANYDESIGN_RELEASE_PACKAGER_HELPER")
            .unwrap_or_else(|_| "scripts/release-packaging-backend.mjs".to_string()),
    )?;
    let helper_hash = sha256_hex(&fs::read(&helper)?);
    let artifact_root = output_root.join("artifact");
    let packager_root = output_root.join("packager");
    let store = Arc::new(ReleaseStore::open(output_root.join("store"))?);
    write_fixture(&artifact_root)?;

    let manifest: ArtifactManifest =
        serde_json::from_slice(&fs::read(artifact_root.join(ARTIFACT_MANIFEST_FILE))?)?;
    let profile = RuntimeProfile::static_web_v1(
        BASE_IMAGE_DIGEST,
        "trusted-static-web-packager@1",
        "trivy-critical-secret-v1",
    )?;
    let input = ReleasePackagingInput {
        project_id: manifest.project_id.clone(),
        version_id: manifest.version_id.clone(),
        run_id: "run-g4-real-toolchain".to_string(),
        template_id: manifest.template_id.clone(),
        template_version: manifest.template_version.clone(),
        artifact_manifest_hash: manifest.sha256()?,
        runtime_manifest_hash: profile.manifest.sha256()?,
        source_snapshot_uri: "fixture://g4/static-web".to_string(),
        runtime_profile_id: profile.id.clone(),
        base_image_digest: profile.base_image_digest.clone(),
        packager_version: profile.packager_version.clone(),
        registry_repository: env::var("ANYDESIGN_RELEASE_REPOSITORY")
            .unwrap_or_else(|_| "localhost:5001/anydesign/work-releases".to_string()),
        scan_policy_version: profile.scan_policy_version.clone(),
    };
    let (_, packaging) = store.prepare(&input)?;
    let mut environment = BTreeMap::new();
    for name in ["PATH", "HOME", "DOCKER_HOST", "TMPDIR"] {
        if let Ok(value) = env::var(name) {
            environment.insert(name.to_string(), value);
        }
    }
    environment.insert(
        "ANYDESIGN_PACKAGER_ROOT".to_string(),
        packager_root.display().to_string(),
    );
    environment.insert(
        "ANYDESIGN_PACKAGER_TOOLS".to_string(),
        env::var("ANYDESIGN_PACKAGER_TOOLS").unwrap_or_else(|_| "/opt/homebrew/bin".to_string()),
    );
    let backend = Arc::new(ProcessReleasePackagingBackend::new(
        helper,
        helper_hash,
        environment,
        Duration::from_secs(20 * 60),
    )?);
    let release = ReleasePackager::new(store.clone(), backend)
        .reconcile(&packaging.id, artifact_root, &profile)
        .await?;
    let packaging = store
        .packaging(&packaging.id)
        .context("validated packaging record disappeared")?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "release": release,
            "packaging": packaging,
            "helperSha256": sha256_hex(&fs::read("scripts/release-packaging-backend.mjs")?),
        }))?
    );
    Ok(())
}

fn write_fixture(root: &Path) -> Result<()> {
    fs::create_dir_all(root.join("assets"))?;
    let files = [
        (
            "index.html",
            "<!doctype html><html><head><link rel=\"stylesheet\" href=\"/assets/style.css\"></head><body><main><h1>AnyDesign published work</h1><p>generic static-web-v1 fixture</p></main></body></html>\n",
        ),
        (
            "assets/style.css",
            "body{font:16px system-ui;margin:4rem;background:#f7f7f5;color:#171717}main{max-width:48rem;margin:auto}\n",
        ),
    ];
    let mut manifest_files = Vec::new();
    for (relative, content) in files {
        fs::write(root.join(relative), content)?;
        manifest_files.push(manifest_file(
            Path::new(relative),
            content.len() as u64,
            sha256_hex(content.as_bytes()),
        )?);
    }
    let manifest = ArtifactManifest::build(
        "project-g4-real-toolchain",
        "version-g4-real-toolchain",
        &"c".repeat(64),
        "generic-static-fixture",
        "1.0.0",
        ArtifactDeliverySpec::HOST_ROOT,
        manifest_files,
    )?;
    fs::write(
        root.join(ARTIFACT_MANIFEST_FILE),
        manifest.canonical_bytes()?,
    )?;
    Ok(())
}

fn absolute(path: impl Into<PathBuf>) -> Result<PathBuf> {
    let path = path.into();
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(env::current_dir()?.join(path))
    }
}
