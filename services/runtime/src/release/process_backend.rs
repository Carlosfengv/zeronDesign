use super::{
    BuiltReleaseImage, PackagingEvidence, PackagingScanEvidence, ReleaseImageBuildRequest,
    ReleaseSignatureEvidence, TrustedReleasePackagingBackend,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, process::Command, time::timeout};

const MAX_HELPER_OUTPUT_BYTES: usize = 1024 * 1024;
const ALLOWED_ENVIRONMENT: &[&str] = &[
    "ANYDESIGN_PACKAGER_ROOT",
    "ANYDESIGN_PACKAGER_TOOLS",
    "DOCKER_HOST",
    "HOME",
    "PATH",
    "TMPDIR",
    "XDG_CONFIG_HOME",
];

/// Runs the release packager through one hash-pinned executable and a bounded JSON protocol.
///
/// This adapter intentionally does not accept shell fragments, inherit the Runtime process
/// environment, or trust an executable only because it has a configured path.
pub struct ProcessReleasePackagingBackend {
    program: PathBuf,
    expected_sha256: String,
    environment: BTreeMap<String, String>,
    deadline: Duration,
}

impl ProcessReleasePackagingBackend {
    pub fn new(
        program: PathBuf,
        expected_sha256: String,
        environment: BTreeMap<String, String>,
        deadline: Duration,
    ) -> Result<Self> {
        if !program.is_absolute() {
            return Err(anyhow!("release packaging helper path must be absolute"));
        }
        validate_sha256(&expected_sha256)?;
        for (name, value) in &environment {
            if !ALLOWED_ENVIRONMENT.contains(&name.as_str()) || value.contains('\0') {
                return Err(anyhow!("release packaging helper environment is invalid"));
            }
        }
        if deadline.is_zero() {
            return Err(anyhow!(
                "release packaging helper deadline must be positive"
            ));
        }
        let backend = Self {
            program,
            expected_sha256,
            environment,
            deadline,
        };
        backend.verify_program()?;
        Ok(backend)
    }

    fn verify_program(&self) -> Result<()> {
        let metadata = fs::metadata(&self.program).with_context(|| {
            format!(
                "release packaging helper is unavailable: {}",
                self.program.display()
            )
        })?;
        if !metadata.is_file() {
            return Err(anyhow!("release packaging helper is not a regular file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o111 == 0 {
                return Err(anyhow!("release packaging helper is not executable"));
            }
        }
        let actual = hex_sha256(&self.program)?;
        if actual != self.expected_sha256 {
            return Err(anyhow!("release packaging helper digest mismatch"));
        }
        Ok(())
    }

    async fn call<I, O>(&self, operation: &str, input: &I) -> Result<O>
    where
        I: Serialize + ?Sized,
        O: DeserializeOwned,
    {
        self.verify_program()?;
        let request = serde_json::to_vec(&json!({
            "protocolVersion": "release-packager-process@1",
            "operation": operation,
            "input": input,
        }))?;
        let mut command = Command::new(&self.program);
        command
            .env_clear()
            .envs(&self.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .context("failed to start release packaging helper")?;
        child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("release packaging helper stdin is unavailable"))?
            .write_all(&request)
            .await?;
        let output = timeout(self.deadline, child.wait_with_output())
            .await
            .map_err(|_| anyhow!("release packaging helper exceeded its deadline"))??;
        if output.stdout.len() > MAX_HELPER_OUTPUT_BYTES
            || output.stderr.len() > MAX_HELPER_OUTPUT_BYTES
        {
            return Err(anyhow!(
                "release packaging helper output exceeded its limit"
            ));
        }
        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "release packaging helper failed for {operation}: {}",
                detail.trim()
            ));
        }
        let envelope: HelperResponse<O> = serde_json::from_slice(&output.stdout)
            .context("release packaging helper returned invalid JSON")?;
        if envelope.protocol_version != "release-packager-process@1" {
            return Err(anyhow!("release packaging helper protocol mismatch"));
        }
        Ok(envelope.output)
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HelperResponse<T> {
    protocol_version: String,
    output: T,
}

#[async_trait]
impl TrustedReleasePackagingBackend for ProcessReleasePackagingBackend {
    async fn build(&self, request: &ReleaseImageBuildRequest) -> Result<BuiltReleaseImage> {
        self.call("build", request).await
    }

    async fn registry_digest(
        &self,
        image_repository: &str,
        release_id: &str,
    ) -> Result<Option<String>> {
        self.call(
            "registryDigest",
            &json!({"imageRepository": image_repository, "releaseId": release_id}),
        )
        .await
    }

    async fn push(
        &self,
        request: &ReleaseImageBuildRequest,
        image: &BuiltReleaseImage,
    ) -> Result<String> {
        self.call("push", &json!({"request": request, "image": image}))
            .await
    }

    async fn generate_evidence(
        &self,
        request: &ReleaseImageBuildRequest,
        image_digest: &str,
    ) -> Result<PackagingEvidence> {
        self.call(
            "generateEvidence",
            &json!({"request": request, "imageDigest": image_digest}),
        )
        .await
    }

    async fn scan(
        &self,
        image_digest: &str,
        evidence: &PackagingEvidence,
        policy_version: &str,
    ) -> Result<PackagingScanEvidence> {
        self.call(
            "scan",
            &json!({
                "imageDigest": image_digest,
                "evidence": evidence,
                "policyVersion": policy_version,
            }),
        )
        .await
    }

    async fn sign(
        &self,
        image_digest: &str,
        provenance_digest: &str,
    ) -> Result<ReleaseSignatureEvidence> {
        self.call(
            "sign",
            &json!({
                "imageDigest": image_digest,
                "provenanceDigest": provenance_digest,
            }),
        )
        .await
    }
}

fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(anyhow!("release packaging helper digest is invalid"));
    }
    Ok(())
}

fn hex_sha256(path: &Path) -> Result<String> {
    let mut digest = Sha256::new();
    digest.update(fs::read(path)?);
    Ok(format!("{:x}", digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn rejects_relative_or_mutated_helpers() {
        assert!(ProcessReleasePackagingBackend::new(
            PathBuf::from("helper"),
            "a".repeat(64),
            BTreeMap::new(),
            Duration::from_secs(1),
        )
        .is_err());

        let root =
            std::env::temp_dir().join(format!("anydesign-process-backend-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let helper = root.join("helper");
        fs::write(&helper, b"#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
        let expected = hex_sha256(&helper).unwrap();
        let backend = ProcessReleasePackagingBackend::new(
            helper.clone(),
            expected,
            BTreeMap::new(),
            Duration::from_secs(1),
        )
        .unwrap();
        fs::write(&helper, b"#!/bin/sh\nexit 1\n").unwrap();
        assert!(backend.verify_program().is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_unapproved_environment() {
        let mut environment = BTreeMap::new();
        environment.insert("REGISTRY_PASSWORD".to_string(), "secret".to_string());
        assert!(ProcessReleasePackagingBackend::new(
            PathBuf::from("/does/not/matter"),
            "a".repeat(64),
            environment,
            Duration::from_secs(1),
        )
        .is_err());
    }
}
