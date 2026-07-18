use crate::types::{canonical_json_hash, Brief, ContentSource};
use regex::Regex;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
use unicode_normalization::UnicodeNormalization;

pub const ACCEPTANCE_CONTRACT_SCHEMA: &str = "acceptance-contract@1";
pub const ACCEPTANCE_REPORT_SCHEMA: &str = "acceptance-report@1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceContractDraft {
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub required_routes: Vec<String>,
    #[serde(default)]
    pub required_text: Vec<String>,
    #[serde(default)]
    pub forbidden_text: Vec<String>,
}

impl AcceptanceContractDraft {
    pub fn validate(&self) -> Result<(), String> {
        if self.required_routes.is_empty() && self.required_text.is_empty() {
            return Err(
                "acceptanceCriteria requires at least one required route or text assertion"
                    .to_string(),
            );
        }
        if self.required_routes.len() > 32
            || self.required_text.len() > 64
            || self.forbidden_text.len() > 64
        {
            return Err("acceptanceCriteria exceeds the supported assertion count".to_string());
        }
        for route in &self.required_routes {
            if !route.starts_with('/')
                || route.contains("..")
                || route.contains(['?', '#'])
                || route.len() > 256
            {
                return Err(format!("invalid required acceptance route: {route}"));
            }
        }
        for text in self.required_text.iter().chain(&self.forbidden_text) {
            let normalized = normalize_text(text);
            if normalized.is_empty() || normalized.chars().count() > 500 {
                return Err(
                    "acceptance text assertions must contain 1..=500 normalized characters"
                        .to_string(),
                );
            }
        }
        if let Some(locale) = self.locale.as_deref() {
            if locale.trim().is_empty() || locale.len() > 32 {
                return Err("acceptance locale is invalid".to_string());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceContract {
    pub schema_version: String,
    pub brief_id: String,
    pub brief_digest: String,
    pub content_sources_digest: String,
    pub artifact_type: String,
    pub locale: Option<String>,
    pub required_routes: Vec<String>,
    pub required_text: Vec<String>,
    pub forbidden_text: Vec<String>,
    pub legacy: bool,
    pub contract_digest: String,
}

impl AcceptanceContract {
    pub fn compile(
        brief_id: &str,
        brief: &Brief,
        content_sources: &[ContentSource],
        draft: Option<AcceptanceContractDraft>,
    ) -> Result<Self, String> {
        if let Some(draft) = draft.as_ref() {
            draft.validate()?;
        }
        let legacy = draft.is_none();
        let draft = draft.unwrap_or_default();
        let brief_value = serde_json::to_value(brief).map_err(|error| error.to_string())?;
        let sources_value =
            serde_json::to_value(content_sources).map_err(|error| error.to_string())?;
        let mut contract = Self {
            schema_version: ACCEPTANCE_CONTRACT_SCHEMA.to_string(),
            brief_id: brief_id.to_string(),
            brief_digest: canonical_json_hash(&brief_value),
            content_sources_digest: canonical_json_hash(&sources_value),
            artifact_type: brief.project_type.clone(),
            locale: draft.locale.map(|value| value.trim().to_string()),
            required_routes: normalized_unique(draft.required_routes, false),
            required_text: normalized_unique(draft.required_text, true),
            forbidden_text: normalized_unique(draft.forbidden_text, true),
            legacy,
            contract_digest: String::new(),
        };
        contract.contract_digest = contract.recompute_digest()?;
        contract.validate()?;
        Ok(contract)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != ACCEPTANCE_CONTRACT_SCHEMA || self.brief_id.trim().is_empty() {
            return Err("acceptance contract identity is invalid".to_string());
        }
        if !matches!(self.artifact_type.as_str(), "website" | "docs") {
            return Err("acceptance contract artifact type is invalid".to_string());
        }
        if self.brief_digest.len() != 64
            || self.content_sources_digest.len() != 64
            || self.contract_digest.len() != 64
        {
            return Err("acceptance contract digest is invalid".to_string());
        }
        if self.recompute_digest()? != self.contract_digest {
            return Err("acceptance contract digest does not match its payload".to_string());
        }
        AcceptanceContractDraft {
            locale: self.locale.clone(),
            required_routes: self.required_routes.clone(),
            required_text: self.required_text.clone(),
            forbidden_text: self.forbidden_text.clone(),
        }
        .validate()
        .or_else(|error| if self.legacy { Ok(()) } else { Err(error) })
    }

    fn recompute_digest(&self) -> Result<String, String> {
        let value = json!({
            "schemaVersion": self.schema_version,
            "briefId": self.brief_id,
            "briefDigest": self.brief_digest,
            "contentSourcesDigest": self.content_sources_digest,
            "artifactType": self.artifact_type,
            "locale": self.locale,
            "requiredRoutes": self.required_routes,
            "requiredText": self.required_text,
            "forbiddenText": self.forbidden_text,
            "legacy": self.legacy,
        });
        Ok(canonical_json_hash(&value))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceCheckStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceCheck {
    pub id: String,
    pub status: AcceptanceCheckStatus,
    pub message: Option<String>,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceReport {
    pub schema_version: String,
    pub run_id: String,
    pub candidate_version_id: String,
    pub candidate_manifest_hash: String,
    pub brief_id: String,
    pub contract_digest: String,
    pub status: AcceptanceCheckStatus,
    pub checks: Vec<AcceptanceCheck>,
    pub evidence: Value,
}

impl AcceptanceReport {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != ACCEPTANCE_REPORT_SCHEMA
            || self.run_id.trim().is_empty()
            || self.candidate_version_id.trim().is_empty()
            || self.candidate_manifest_hash.len() != 64
            || self.brief_id.trim().is_empty()
            || self.contract_digest.len() != 64
            || self.checks.is_empty()
        {
            return Err("acceptance report identity is invalid".to_string());
        }
        let failed = self
            .checks
            .iter()
            .any(|check| check.status == AcceptanceCheckStatus::Failed);
        if failed != (self.status == AcceptanceCheckStatus::Failed) {
            return Err("acceptance report status does not match its checks".to_string());
        }
        Ok(())
    }

    pub fn passed(&self) -> bool {
        self.status == AcceptanceCheckStatus::Passed
    }
}

pub fn validate_staged_candidate(
    staged_root: &Path,
    run_id: &str,
    candidate_version_id: &str,
    candidate_manifest_hash: &str,
    contract: &AcceptanceContract,
) -> Result<AcceptanceReport, String> {
    contract.validate()?;
    let mut checks = Vec::new();
    let mut visible_documents = Vec::new();
    let routes = if contract.required_routes.is_empty() {
        vec!["/".to_string()]
    } else {
        contract.required_routes.clone()
    };
    for route in routes {
        match resolve_route(staged_root, &route) {
            Some(file) => match fs::read_to_string(&file) {
                Ok(html) => {
                    visible_documents.push(extract_visible_text(&html));
                    checks.push(AcceptanceCheck {
                        id: format!("route:{route}"),
                        status: AcceptanceCheckStatus::Passed,
                        message: None,
                        evidence: vec![file
                            .strip_prefix(staged_root)
                            .unwrap_or(&file)
                            .display()
                            .to_string()],
                    });
                }
                Err(error) => checks.push(failed_check(
                    format!("route:{route}"),
                    format!("route is not readable: {error}"),
                )),
            },
            None => checks.push(failed_check(
                format!("route:{route}"),
                "required route is missing".to_string(),
            )),
        }
    }
    let visible_text = normalize_text(&visible_documents.join(" "));
    for required in &contract.required_text {
        let normalized = normalize_text(required);
        let passed = visible_text.contains(&normalized);
        checks.push(AcceptanceCheck {
            id: format!("required-text:{}", canonical_json_hash(&json!(required))),
            status: if passed {
                AcceptanceCheckStatus::Passed
            } else {
                AcceptanceCheckStatus::Failed
            },
            message: (!passed).then(|| format!("required visible text is missing: {required}")),
            evidence: Vec::new(),
        });
    }
    for forbidden in &contract.forbidden_text {
        let normalized = normalize_text(forbidden);
        let passed = !visible_text.contains(&normalized);
        checks.push(AcceptanceCheck {
            id: format!("forbidden-text:{}", canonical_json_hash(&json!(forbidden))),
            status: if passed {
                AcceptanceCheckStatus::Passed
            } else {
                AcceptanceCheckStatus::Failed
            },
            message: (!passed).then(|| format!("forbidden visible text is present: {forbidden}")),
            evidence: Vec::new(),
        });
    }
    if checks.is_empty() {
        checks.push(AcceptanceCheck {
            id: "legacy-contract".to_string(),
            status: AcceptanceCheckStatus::Passed,
            message: Some("legacy Brief has no structured content assertions".to_string()),
            evidence: Vec::new(),
        });
    }
    let status = if checks
        .iter()
        .any(|check| check.status == AcceptanceCheckStatus::Failed)
    {
        AcceptanceCheckStatus::Failed
    } else {
        AcceptanceCheckStatus::Passed
    };
    let report = AcceptanceReport {
        schema_version: ACCEPTANCE_REPORT_SCHEMA.to_string(),
        run_id: run_id.to_string(),
        candidate_version_id: candidate_version_id.to_string(),
        candidate_manifest_hash: candidate_manifest_hash.to_string(),
        brief_id: contract.brief_id.clone(),
        contract_digest: contract.contract_digest.clone(),
        status,
        checks,
        evidence: json!({
            "visibleTextSha256": canonical_json_hash(&json!(visible_text)),
            "visibleTextCharacters": visible_text.chars().count(),
            "legacyContract": contract.legacy,
        }),
    };
    report.validate()?;
    Ok(report)
}

fn resolve_route(root: &Path, route: &str) -> Option<PathBuf> {
    let relative = route.trim_start_matches('/').trim_end_matches('/');
    let candidates = if relative.is_empty() {
        vec![root.join("index.html")]
    } else {
        vec![
            root.join(relative).join("index.html"),
            root.join(format!("{relative}.html")),
        ]
    };
    candidates
        .into_iter()
        .find(|candidate| candidate.is_file() && candidate.starts_with(root))
}

fn extract_visible_text(html: &str) -> String {
    let script = Regex::new(r"(?is)<script\b[^>]*>.*?</script>").expect("valid script regex");
    let style = Regex::new(r"(?is)<style\b[^>]*>.*?</style>").expect("valid style regex");
    let without_scripts = script.replace_all(html, " ");
    let cleaned = style.replace_all(&without_scripts, " ");
    let document = Html::parse_document(&cleaned);
    let selector = Selector::parse("body").expect("valid body selector");
    let text = document
        .select(&selector)
        .next()
        .map(|body| body.text().collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| document.root_element().text().collect::<Vec<_>>().join(" "));
    normalize_text(&text)
}

fn normalize_text(value: &str) -> String {
    value
        .nfkc()
        .flat_map(char::to_lowercase)
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalized_unique(values: Vec<String>, normalize: bool) -> Vec<String> {
    let mut output = Vec::new();
    for value in values {
        let value = if normalize {
            normalize_text(&value)
        } else {
            value.trim().to_string()
        };
        if !value.is_empty() && !output.contains(&value) {
            output.push(value);
        }
    }
    output
}

fn failed_check(id: String, message: String) -> AcceptanceCheck {
    AcceptanceCheck {
        id,
        status: AcceptanceCheckStatus::Failed,
        message: Some(message),
        evidence: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brief() -> Brief {
        Brief {
            project_type: "website".to_string(),
            audience: "operators".to_string(),
            content_hierarchy: vec!["Hero".to_string()],
            page_structure: json!([]),
            visual_direction: "clean".to_string(),
            recommended_template: "astro-website".to_string(),
            assumptions: vec![],
            missing_information: vec![],
        }
    }

    #[test]
    fn visible_dom_text_is_normalized_and_scripts_do_not_satisfy_acceptance() {
        let root =
            std::env::temp_dir().join(format!("acceptance-contract-{}", rand::random::<u64>()));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("index.html"),
            "<html><body><h1>ＺStack &amp; 企业 智能体云</h1><script>forbidden fake title</script></body></html>",
        )
        .unwrap();
        let contract = AcceptanceContract::compile(
            "brief-1",
            &brief(),
            &[],
            Some(AcceptanceContractDraft {
                locale: Some("zh-CN".to_string()),
                required_routes: vec!["/".to_string()],
                required_text: vec!["zstack & 企业 智能体云".to_string()],
                forbidden_text: vec!["fake title".to_string()],
            }),
        )
        .unwrap();
        let report =
            validate_staged_candidate(&root, "run-1", "version-1", &"a".repeat(64), &contract)
                .unwrap();
        assert!(report.passed());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_required_text_fails_closed() {
        let root = std::env::temp_dir().join(format!(
            "acceptance-contract-missing-{}",
            rand::random::<u64>()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("index.html"), "<h1>Generic cloud</h1>").unwrap();
        let contract = AcceptanceContract::compile(
            "brief-1",
            &brief(),
            &[],
            Some(AcceptanceContractDraft {
                required_routes: vec!["/".to_string()],
                required_text: vec!["ZStack Zenova 企业智能体云".to_string()],
                ..Default::default()
            }),
        )
        .unwrap();
        let report =
            validate_staged_candidate(&root, "run-1", "version-1", &"b".repeat(64), &contract)
                .unwrap();
        assert!(!report.passed());
        assert!(report
            .checks
            .iter()
            .any(|check| check.status == AcceptanceCheckStatus::Failed));
        let _ = fs::remove_dir_all(root);
    }
}
