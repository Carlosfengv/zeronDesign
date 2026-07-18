use crate::{artifact_publisher::safe_segment, generation_contract::ValidationReport};
use anyhow::{anyhow, Context, Result};
use std::{fs, path::PathBuf};

#[derive(Debug, Clone)]
pub struct FileValidationReportStore {
    runtime_storage_dir: PathBuf,
}

impl FileValidationReportStore {
    pub fn new(runtime_storage_dir: impl Into<PathBuf>) -> Self {
        Self {
            runtime_storage_dir: runtime_storage_dir.into(),
        }
    }

    pub fn uri(project_id: &str, run_id: &str, candidate_version_id: &str) -> String {
        format!(
            "runtime://validation-reports/{}/{}/{}.json",
            safe_segment(project_id),
            safe_segment(run_id),
            safe_segment(candidate_version_id)
        )
    }

    pub fn write(&self, project_id: &str, report: &ValidationReport) -> Result<String> {
        report
            .validate()
            .map_err(|error| anyhow!("invalid validation report: {error}"))?;
        let target = self.path(project_id, &report.run_id, &report.candidate_version_id);
        let parent = target
            .parent()
            .ok_or_else(|| anyhow!("validation report path has no parent"))?;
        fs::create_dir_all(parent)?;
        let temporary = target.with_extension("json.tmp");
        fs::write(
            &temporary,
            serde_json::to_vec_pretty(report).context("failed to serialize validation report")?,
        )?;
        fs::rename(&temporary, &target)?;
        Ok(Self::uri(
            project_id,
            &report.run_id,
            &report.candidate_version_id,
        ))
    }

    pub fn read(
        &self,
        project_id: &str,
        run_id: &str,
        candidate_version_id: &str,
    ) -> Result<ValidationReport> {
        let path = self.path(project_id, run_id, candidate_version_id);
        let report: ValidationReport = serde_json::from_slice(
            &fs::read(&path)
                .with_context(|| format!("failed to read validation report {}", path.display()))?,
        )
        .context("failed to deserialize validation report")?;
        report
            .validate()
            .map_err(|error| anyhow!("invalid persisted validation report: {error}"))?;
        Ok(report)
    }

    fn path(&self, project_id: &str, run_id: &str, candidate_version_id: &str) -> PathBuf {
        self.runtime_storage_dir
            .join("validation-reports")
            .join(safe_segment(project_id))
            .join(safe_segment(run_id))
            .join(format!("{}.json", safe_segment(candidate_version_id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation_contract::{
        ValidationCheckResult, ValidationCheckStatus, VALIDATION_REPORT_SCHEMA,
    };

    #[test]
    fn validation_report_round_trips_and_identifiers_cannot_escape_root() {
        let root = std::env::temp_dir().join(format!(
            "runtime-validation-store-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let report = ValidationReport {
            schema_version: VALIDATION_REPORT_SCHEMA.to_string(),
            run_id: "run/../outside".to_string(),
            candidate_version_id: "version/../outside".to_string(),
            candidate_manifest_hash: "a".repeat(64),
            artifact_manifest_hash: "b".repeat(64),
            generation_contract_digest: "c".repeat(64),
            template_version: "template@1".to_string(),
            checks: vec![ValidationCheckResult {
                id: "build".to_string(),
                status: ValidationCheckStatus::Passed,
                message: None,
                evidence: vec!["runtime://build/one".to_string()],
            }],
            evidence: serde_json::json!({ "build": { "success": true } }),
        };
        let store = FileValidationReportStore::new(&root);

        let uri = store.write("project/../outside", &report).unwrap();
        let actual = store
            .read("project/../outside", "run/../outside", "version/../outside")
            .unwrap();

        assert_eq!(actual, report);
        assert!(uri.starts_with("runtime://validation-reports/"));
        let report_root = root.join("validation-reports");
        assert_eq!(
            fs::read_dir(&report_root).unwrap().count(),
            1,
            "unsafe identifiers must not create sibling roots"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_reports_are_not_persisted() {
        let root = std::env::temp_dir().join(format!(
            "runtime-validation-store-invalid-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let report = ValidationReport {
            schema_version: VALIDATION_REPORT_SCHEMA.to_string(),
            run_id: "run-1".to_string(),
            candidate_version_id: "version-1".to_string(),
            candidate_manifest_hash: "a".repeat(64),
            artifact_manifest_hash: "b".repeat(64),
            generation_contract_digest: "c".repeat(64),
            template_version: "template@1".to_string(),
            checks: vec![ValidationCheckResult {
                id: "build".to_string(),
                status: ValidationCheckStatus::Passed,
                message: None,
                evidence: vec![],
            }],
            evidence: serde_json::json!({}),
        };

        assert!(FileValidationReportStore::new(&root)
            .write("project-1", &report)
            .is_err());
        assert!(!root.exists());
    }
}
