use crate::{acceptance_contract::AcceptanceReport, artifact_publisher::safe_segment};
use anyhow::{anyhow, Context, Result};
use std::{fs, path::PathBuf};

#[derive(Debug, Clone)]
pub struct FileAcceptanceReportStore {
    runtime_storage_dir: PathBuf,
}

impl FileAcceptanceReportStore {
    pub fn new(runtime_storage_dir: impl Into<PathBuf>) -> Self {
        Self {
            runtime_storage_dir: runtime_storage_dir.into(),
        }
    }

    pub fn uri(project_id: &str, run_id: &str, candidate_version_id: &str) -> String {
        format!(
            "runtime://acceptance-reports/{}/{}/{}.json",
            safe_segment(project_id),
            safe_segment(run_id),
            safe_segment(candidate_version_id),
        )
    }

    pub fn write(&self, project_id: &str, report: &AcceptanceReport) -> Result<String> {
        report
            .validate()
            .map_err(|error| anyhow!("invalid acceptance report: {error}"))?;
        let target = self.path(project_id, &report.run_id, &report.candidate_version_id);
        let parent = target
            .parent()
            .ok_or_else(|| anyhow!("acceptance report path has no parent"))?;
        fs::create_dir_all(parent)?;
        let temporary = target.with_extension("json.tmp");
        fs::write(
            &temporary,
            serde_json::to_vec_pretty(report).context("failed to serialize acceptance report")?,
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
    ) -> Result<AcceptanceReport> {
        let path = self.path(project_id, run_id, candidate_version_id);
        let report: AcceptanceReport = serde_json::from_slice(
            &fs::read(&path)
                .with_context(|| format!("failed to read acceptance report {}", path.display()))?,
        )
        .context("failed to deserialize acceptance report")?;
        report
            .validate()
            .map_err(|error| anyhow!("invalid persisted acceptance report: {error}"))?;
        Ok(report)
    }

    pub fn failed_report_count(&self, project_id: &str, run_id: &str) -> Result<u32> {
        let directory = self
            .runtime_storage_dir
            .join("acceptance-reports")
            .join(safe_segment(project_id))
            .join(safe_segment(run_id));
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.into()),
        };
        let mut failed = 0u32;
        for entry in entries {
            let path = entry?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let report: AcceptanceReport =
                serde_json::from_slice(&fs::read(&path).with_context(|| {
                    format!("failed to read acceptance report {}", path.display())
                })?)
                .with_context(|| {
                    format!("failed to deserialize acceptance report {}", path.display())
                })?;
            report
                .validate()
                .map_err(|error| anyhow!("invalid persisted acceptance report: {error}"))?;
            if report.run_id == run_id && !report.passed() {
                failed = failed.saturating_add(1);
            }
        }
        Ok(failed)
    }

    fn path(&self, project_id: &str, run_id: &str, candidate_version_id: &str) -> PathBuf {
        self.runtime_storage_dir
            .join("acceptance-reports")
            .join(safe_segment(project_id))
            .join(safe_segment(run_id))
            .join(format!("{}.json", safe_segment(candidate_version_id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acceptance_contract::{
        AcceptanceCheck, AcceptanceCheckStatus, ACCEPTANCE_REPORT_SCHEMA,
    };
    use serde_json::json;

    #[test]
    fn report_round_trips_under_safe_segments() {
        let root =
            std::env::temp_dir().join(format!("acceptance-report-store-{}", rand::random::<u64>()));
        let report = AcceptanceReport {
            schema_version: ACCEPTANCE_REPORT_SCHEMA.to_string(),
            run_id: "run/../one".to_string(),
            candidate_version_id: "version/../one".to_string(),
            candidate_manifest_hash: "a".repeat(64),
            brief_id: "brief-1".to_string(),
            contract_digest: "b".repeat(64),
            status: AcceptanceCheckStatus::Passed,
            checks: vec![AcceptanceCheck {
                id: "route:/".to_string(),
                status: AcceptanceCheckStatus::Passed,
                message: None,
                evidence: vec!["index.html".to_string()],
            }],
            evidence: json!({}),
        };
        let store = FileAcceptanceReportStore::new(&root);
        let uri = store.write("project/../one", &report).unwrap();
        assert_eq!(
            store
                .read("project/../one", "run/../one", "version/../one")
                .unwrap(),
            report
        );
        assert!(uri.starts_with("runtime://acceptance-reports/"));
        assert_eq!(
            store
                .failed_report_count("project/../one", "run/../one")
                .unwrap(),
            0
        );
        assert_eq!(
            fs::read_dir(root.join("acceptance-reports"))
                .unwrap()
                .count(),
            1
        );
        let _ = fs::remove_dir_all(root);
    }
}
