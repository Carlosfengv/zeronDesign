use crate::{artifact_routes::ArtifactRouteContract, types::canonical_json_hash};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const GENERATION_CONTRACT_SCHEMA_V1: &str = "generation-contract@1";
pub const GENERATION_CONTRACT_SCHEMA: &str = "generation-contract@2";
pub const VALIDATION_REPORT_SCHEMA: &str = "validation-report@2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactType {
    Website,
    Docs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildContract {
    pub command: Vec<String>,
    pub output_directory: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationContract {
    pub schema_version: String,
    pub artifact_type: ArtifactType,
    pub template_key: String,
    pub build: BuildContract,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_contract: Option<ArtifactRouteContract>,
    pub required_checks: Vec<String>,
}

impl GenerationContract {
    pub fn website(template_key: impl Into<String>, output_directory: impl Into<String>) -> Self {
        Self {
            schema_version: GENERATION_CONTRACT_SCHEMA.to_string(),
            artifact_type: ArtifactType::Website,
            template_key: template_key.into(),
            build: BuildContract {
                command: vec!["npm".to_string(), "run".to_string(), "build".to_string()],
                output_directory: output_directory.into(),
            },
            route_contract: Some(ArtifactRouteContract::website()),
            required_checks: required_checks(&[
                "build",
                "artifact-integrity",
                "desktop-render",
                "mobile-render",
                "font-coverage",
                "accessibility",
                "responsive-layout",
                "link-integrity",
                "console-errors",
                "metadata",
            ]),
        }
    }

    pub fn docs(template_key: impl Into<String>, output_directory: impl Into<String>) -> Self {
        Self {
            schema_version: GENERATION_CONTRACT_SCHEMA.to_string(),
            artifact_type: ArtifactType::Docs,
            template_key: template_key.into(),
            build: BuildContract {
                command: vec!["npm".to_string(), "run".to_string(), "build".to_string()],
                output_directory: output_directory.into(),
            },
            route_contract: Some(ArtifactRouteContract::docs()),
            required_checks: required_checks(&[
                "build",
                "artifact-integrity",
                "mdx-compile",
                "navigation",
                "duplicate-slugs",
                "internal-links",
                "heading-anchors",
                "code-blocks",
                "search-index",
                "desktop-render",
                "mobile-render",
                "font-coverage",
                "accessibility",
                "console-errors",
            ]),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !matches!(
            self.schema_version.as_str(),
            GENERATION_CONTRACT_SCHEMA | GENERATION_CONTRACT_SCHEMA_V1
        ) {
            return Err(format!(
                "unsupported generation contract schema: {}",
                self.schema_version
            ));
        }
        if self.schema_version == GENERATION_CONTRACT_SCHEMA && self.route_contract.is_none() {
            return Err("generation-contract@2 requires routeContract".to_string());
        }
        if let Some(route_contract) = &self.route_contract {
            route_contract
                .validate()
                .map_err(|error| error.to_string())?;
        }
        if self.template_key.trim().is_empty() {
            return Err("templateKey is required".to_string());
        }
        if self.build.command.is_empty()
            || self.build.command.iter().any(|part| part.trim().is_empty())
        {
            return Err("build command must contain non-empty arguments".to_string());
        }
        if self.build.output_directory.trim().is_empty() {
            return Err("build outputDirectory is required".to_string());
        }
        if self.required_checks.is_empty() {
            return Err("requiredChecks must not be empty".to_string());
        }
        let mut seen = HashSet::new();
        for check in &self.required_checks {
            if check.trim().is_empty() {
                return Err("requiredChecks must not contain empty ids".to_string());
            }
            if !seen.insert(check) {
                return Err(format!("duplicate required check: {check}"));
            }
        }
        Ok(())
    }

    pub fn effective_route_contract(&self) -> Result<ArtifactRouteContract, String> {
        self.validate()?;
        Ok(self
            .route_contract
            .clone()
            .unwrap_or_else(|| match self.artifact_type {
                ArtifactType::Website => ArtifactRouteContract::website(),
                ArtifactType::Docs => ArtifactRouteContract::docs(),
            }))
    }

    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|error| error.to_string())?;
        Ok(canonical_json_hash(&value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationCheckStatus {
    Passed,
    Failed,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationCheckResult {
    pub id: String,
    pub status: ValidationCheckStatus,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidationReport {
    pub schema_version: String,
    pub run_id: String,
    pub candidate_version_id: String,
    pub candidate_manifest_hash: String,
    pub artifact_manifest_hash: String,
    pub generation_contract_digest: String,
    pub template_version: String,
    pub checks: Vec<ValidationCheckResult>,
    #[serde(default)]
    pub evidence: serde_json::Value,
}

impl ValidationReport {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != VALIDATION_REPORT_SCHEMA {
            return Err(format!(
                "unsupported validation report schema: {}",
                self.schema_version
            ));
        }
        if self.run_id.trim().is_empty() {
            return Err("runId is required".to_string());
        }
        if self.candidate_version_id.trim().is_empty() {
            return Err("candidateVersionId is required".to_string());
        }
        if !is_sha256(&self.candidate_manifest_hash)
            || !is_sha256(&self.artifact_manifest_hash)
            || !is_sha256(&self.generation_contract_digest)
        {
            return Err("validation report integrity digest is invalid".to_string());
        }
        if self.template_version.trim().is_empty() {
            return Err("templateVersion is required".to_string());
        }
        if self.checks.is_empty() {
            return Err("checks must not be empty".to_string());
        }

        let mut seen = HashSet::new();
        for check in &self.checks {
            if check.id.trim().is_empty() {
                return Err("checks must not contain empty ids".to_string());
            }
            if !seen.insert(check.id.as_str()) {
                return Err(format!("duplicate validation check: {}", check.id));
            }
            match check.status {
                ValidationCheckStatus::Passed if check.evidence.is_empty() => {
                    return Err(format!(
                        "passed validation check {} must include evidence",
                        check.id
                    ));
                }
                ValidationCheckStatus::Failed | ValidationCheckStatus::Unavailable
                    if check
                        .message
                        .as_deref()
                        .is_none_or(|message| message.trim().is_empty()) =>
                {
                    return Err(format!(
                        "non-passing validation check {} must include a message",
                        check.id
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn promotion_blockers(&self, contract: &GenerationContract) -> Vec<ValidationBlocker> {
        let results = self
            .checks
            .iter()
            .map(|check| (check.id.as_str(), check))
            .collect::<HashMap<_, _>>();
        contract
            .required_checks
            .iter()
            .filter_map(|required| match results.get(required.as_str()) {
                Some(result) if result.status == ValidationCheckStatus::Passed => None,
                Some(result) => Some(ValidationBlocker {
                    check_id: required.clone(),
                    status: result.status,
                    message: result.message.clone(),
                }),
                None => Some(ValidationBlocker {
                    check_id: required.clone(),
                    status: ValidationCheckStatus::Unavailable,
                    message: Some("required validation result is missing".to_string()),
                }),
            })
            .collect()
    }

    pub fn can_promote(&self, contract: &GenerationContract) -> bool {
        self.validate().is_ok()
            && contract
                .digest()
                .is_ok_and(|digest| digest == self.generation_contract_digest)
            && self.promotion_blockers(contract).is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationBlocker {
    pub check_id: String,
    pub status: ValidationCheckStatus,
    pub message: Option<String>,
}

fn required_checks(checks: &[&str]) -> Vec<String> {
    checks.iter().map(|check| (*check).to_string()).collect()
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing_report(contract: &GenerationContract) -> ValidationReport {
        ValidationReport {
            schema_version: VALIDATION_REPORT_SCHEMA.to_string(),
            run_id: "run-1".to_string(),
            candidate_version_id: "version-1".to_string(),
            candidate_manifest_hash: "a".repeat(64),
            artifact_manifest_hash: "b".repeat(64),
            generation_contract_digest: contract.digest().unwrap(),
            template_version: "test-template@1".to_string(),
            checks: contract
                .required_checks
                .iter()
                .map(|id| ValidationCheckResult {
                    id: id.clone(),
                    status: ValidationCheckStatus::Passed,
                    message: None,
                    evidence: vec![format!("runtime://validation/{id}")],
                })
                .collect(),
            evidence: serde_json::json!({ "source": "test" }),
        }
    }

    #[test]
    fn website_and_docs_contracts_share_platform_checks() {
        let website = GenerationContract::website("next-app", "dist");
        let docs = GenerationContract::docs("fumadocs-docs", "out");
        for check in [
            "build",
            "artifact-integrity",
            "desktop-render",
            "mobile-render",
            "font-coverage",
            "accessibility",
            "console-errors",
        ] {
            assert!(website.required_checks.contains(&check.to_string()));
            assert!(docs.required_checks.contains(&check.to_string()));
        }
        assert!(docs.required_checks.contains(&"mdx-compile".to_string()));
        assert!(website
            .required_checks
            .contains(&"responsive-layout".to_string()));
    }

    #[test]
    fn legacy_v1_contract_infers_the_frozen_route_contract_without_rewriting_digest_input() {
        let contract: GenerationContract = serde_json::from_value(serde_json::json!({
            "schemaVersion": "generation-contract@1",
            "artifactType": "docs",
            "templateKey": "fumadocs-docs",
            "build": {
                "command": ["npm", "run", "build"],
                "outputDirectory": "out"
            },
            "requiredChecks": ["build"]
        }))
        .unwrap();

        assert!(contract.validate().is_ok());
        assert_eq!(
            contract.effective_route_contract().unwrap(),
            ArtifactRouteContract::docs()
        );
        let serialized = serde_json::to_value(&contract).unwrap();
        assert!(serialized.get("routeContract").is_none());
        assert_eq!(contract.digest().unwrap(), canonical_json_hash(&serialized));
    }

    #[test]
    fn v2_contract_requires_an_explicit_route_contract() {
        let mut contract = GenerationContract::website("next-app", "dist");
        contract.route_contract = None;

        assert_eq!(
            contract.validate().unwrap_err(),
            "generation-contract@2 requires routeContract"
        );
    }

    #[test]
    fn required_failed_or_unavailable_checks_block_promotion() {
        let contract = GenerationContract::website("next-app", "dist");
        for status in [
            ValidationCheckStatus::Failed,
            ValidationCheckStatus::Unavailable,
        ] {
            let mut report = passing_report(&contract);
            report.checks[0].status = status;
            assert!(!report.can_promote(&contract));
            assert_eq!(report.promotion_blockers(&contract)[0].status, status);
        }
    }

    #[test]
    fn missing_required_check_is_unavailable_and_blocks_promotion() {
        let contract = GenerationContract::docs("fumadocs-docs", "out");
        let mut report = passing_report(&contract);
        let missing = report.checks.pop().unwrap().id;
        assert!(!report.can_promote(&contract));
        assert_eq!(
            report.promotion_blockers(&contract),
            vec![ValidationBlocker {
                check_id: missing,
                status: ValidationCheckStatus::Unavailable,
                message: Some("required validation result is missing".to_string()),
            }]
        );
    }

    #[test]
    fn all_required_checks_must_pass_before_promotion() {
        let contract = GenerationContract::website("next-app", "dist");
        let report = passing_report(&contract);
        assert!(report.can_promote(&contract));
    }

    #[test]
    fn generation_contract_digest_changes_with_release_relevant_fields() {
        let original = GenerationContract::website("next-app", "dist");
        let mut changed_output = original.clone();
        changed_output.build.output_directory = "out".to_string();
        let mut changed_checks = original.clone();
        changed_checks
            .required_checks
            .push("new-required-check".to_string());

        assert_ne!(original.digest().unwrap(), changed_output.digest().unwrap());
        assert_ne!(original.digest().unwrap(), changed_checks.digest().unwrap());
        let report = passing_report(&original);
        assert!(!report.can_promote(&changed_output));
        assert!(!report.can_promote(&changed_checks));
    }

    #[test]
    fn duplicate_checks_or_unproven_successes_cannot_promote() {
        let contract = GenerationContract::website("next-app", "dist");

        let mut duplicate = passing_report(&contract);
        duplicate.checks.push(duplicate.checks[0].clone());
        assert!(!duplicate.can_promote(&contract));
        assert!(duplicate.validate().unwrap_err().contains("duplicate"));

        let mut missing_evidence = passing_report(&contract);
        missing_evidence.checks[0].evidence.clear();
        assert!(!missing_evidence.can_promote(&contract));
        assert!(missing_evidence
            .validate()
            .unwrap_err()
            .contains("must include evidence"));
    }
}
