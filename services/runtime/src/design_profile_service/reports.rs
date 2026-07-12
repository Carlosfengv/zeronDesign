use super::{store_error, DesignProfileService, DesignProfileServiceError};
use crate::{
    design_profile::{
        registered_template_spec, render_design_profile_markdown,
        signature_rule_applies_to_surface, unsupported_extended_tokens_for_template,
    },
    types::{DesignProfile, DesignProfileFidelityReport, DESIGN_PROFILE_SCHEMA_V1},
};
use serde_json::Value;

impl DesignProfileService {
    pub async fn fidelity_report(
        &self,
        design_profile_id: &str,
        version: u32,
        surface: &str,
        template: &str,
    ) -> Result<DesignProfileFidelityReport, DesignProfileServiceError> {
        if version == 0 {
            return Err(DesignProfileServiceError::InvalidRequest(
                "version must be positive".to_string(),
            ));
        }
        let versions = self
            .store
            .design_profile_versions(design_profile_id)
            .await
            .map_err(store_error)?;
        let profile = versions
            .into_iter()
            .find(|profile| profile.version == version)
            .ok_or_else(|| {
                DesignProfileServiceError::NotFound(format!(
                    "design profile version not found: {design_profile_id}@{version}"
                ))
            })?;
        let effective = profile
            .effective_for(surface, template)
            .map_err(DesignProfileServiceError::InvalidRequest)?;
        let materialized: DesignProfile = serde_json::from_value(effective.profile.clone())
            .map_err(|error| DesignProfileServiceError::Internal(error.to_string()))?;
        let capsule = render_design_profile_markdown(&materialized)
            .map_err(|error| DesignProfileServiceError::Internal(error.to_string()))?;
        let mut required_signature_rule_ids = materialized
            .signature_rules
            .iter()
            .filter(|rule| rule.get("priority").and_then(Value::as_str) == Some("required"))
            .filter(|rule| signature_rule_applies_to_surface(rule, surface))
            .filter_map(|rule| {
                rule.get("id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
            })
            .collect::<Vec<_>>();
        required_signature_rule_ids.sort();
        let capsule_included_rule_ids = required_signature_rule_ids
            .iter()
            .filter(|id| capsule.contains(&format!("[{id}]")))
            .cloned()
            .collect::<Vec<_>>();
        let capsule_missing_rule_ids = required_signature_rule_ids
            .iter()
            .filter(|id| !capsule_included_rule_ids.contains(id))
            .cloned()
            .collect::<Vec<_>>();
        let unsupported_extended_tokens = unsupported_extended_tokens_for_template(
            &materialized.extended_token_mapping,
            template,
        );
        let source_integrity = profile
            .source
            .get("integrity")
            .and_then(Value::as_str)
            .unwrap_or(if profile.schema_version == DESIGN_PROFILE_SCHEMA_V1 {
                "unverified"
            } else {
                "missing"
            })
            .to_string();
        let source_hash_matches = if let Some(artifact_id) = profile
            .source
            .get("primarySourceArtifactId")
            .and_then(Value::as_str)
        {
            match self.store.get_design_source_artifact(artifact_id).await {
                Some(artifact) => Some(
                    profile.source.get("sourceHash").and_then(Value::as_str)
                        == Some(artifact.sha256.as_str())
                        && self
                            .store
                            .read_design_source_artifact_content(artifact_id)
                            .await
                            .is_ok(),
                ),
                None => Some(false),
            }
        } else {
            None
        };
        let mut warnings = Vec::new();
        if source_hash_matches == Some(false) {
            warnings.push("source artifact integrity verification failed".to_string());
        }
        if !unsupported_extended_tokens.is_empty() {
            warnings.push(format!(
                "template does not support extended tokens: {}",
                unsupported_extended_tokens.join(", ")
            ));
        }
        if !capsule_missing_rule_ids.is_empty() {
            warnings.push("Design Capsule is missing required signature rules".to_string());
        }
        Ok(DesignProfileFidelityReport {
            design_profile_id: design_profile_id.to_string(),
            version,
            schema_version: profile.schema_version,
            surface: surface.to_string(),
            template: template.to_string(),
            style_contract_version: registered_template_spec(template)
                .map(|spec| spec.style.version.to_string())
                .unwrap_or_else(|| "runtime-style-contract@p2".to_string()),
            effective_profile_hash: effective.effective_profile_hash,
            source_integrity,
            source_hash_matches,
            required_signature_rule_ids,
            capsule_included_rule_ids,
            capsule_missing_rule_ids,
            unsupported_extended_tokens,
            warnings,
        })
    }
}
