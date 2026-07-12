use super::{DesignProfileService, DesignProfileServiceError};
use crate::{
    project::resolve_built_in_template_for_init,
    types::{AgentPhase, AgentRun, DesignProfile},
};
use serde_json::Value;

pub struct RunProfileContextQuery<'a> {
    pub project_id: &'a str,
    pub workspace_id: Option<&'a str>,
    pub organization_id: Option<&'a str>,
    pub explicit_profile_id: Option<&'a str>,
    pub phase: AgentPhase,
    pub brief_id: Option<&'a str>,
}

#[derive(Debug)]
pub struct PreparedRunProfile {
    pub profile: Option<DesignProfile>,
    pub execution_target: Option<(String, String)>,
    pub conflict: Option<String>,
}

impl DesignProfileService {
    pub async fn prepare_run_context(
        &self,
        query: RunProfileContextQuery<'_>,
    ) -> Result<PreparedRunProfile, DesignProfileServiceError> {
        let profile = self
            .store
            .resolve_design_profile(
                query.project_id,
                query.workspace_id,
                query.organization_id,
                query.explicit_profile_id,
            )
            .await
            .map_err(super::store_error)?;
        if query.phase != AgentPhase::Build {
            return Ok(PreparedRunProfile {
                profile,
                execution_target: None,
                conflict: None,
            });
        }
        let Some(brief_id) = query.brief_id else {
            return Ok(PreparedRunProfile {
                profile,
                execution_target: None,
                conflict: None,
            });
        };
        let brief = self.store.get_brief(brief_id).await.ok_or_else(|| {
            DesignProfileServiceError::NotFound(format!("brief not found: {brief_id}"))
        })?;
        let template = resolve_built_in_template_for_init(&brief.recommended_template)
            .await
            .map_err(|error| DesignProfileServiceError::InvalidRequest(error.to_string()))?;
        let conflict = profile.as_ref().and_then(|profile| {
            let allowed = profile
                .technical
                .get("allowedTemplates")
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(Value::as_str).collect::<Vec<_>>())
                .unwrap_or_default();
            (!allowed.is_empty() && !allowed.contains(&brief.recommended_template.as_str())).then(
                || {
                    format!(
                        "Brief recommendedTemplate={} is not allowed by DesignProfile {}",
                        brief.recommended_template, profile.id
                    )
                },
            )
        });
        Ok(PreparedRunProfile {
            profile,
            execution_target: Some((template.surface.to_string(), brief.recommended_template)),
            conflict,
        })
    }

    pub async fn prebuild_failure(
        &self,
        run: &AgentRun,
        profile: &DesignProfile,
    ) -> Option<(String, String)> {
        if run.phase != AgentPhase::Build {
            return None;
        }
        if profile.status != "active" {
            return Some((
                "needs_user_input:design_profile_integrity_failed".to_string(),
                "DesignProfile must be active before Build.".to_string(),
            ));
        }
        if run.design_profile_hash.as_deref() != Some(profile.stable_hash().as_str()) {
            return Some((
                "needs_user_input:design_profile_integrity_failed".to_string(),
                "DesignProfile hash no longer matches the run snapshot.".to_string(),
            ));
        }
        if let (Some(surface), Some(template), Some(expected_hash)) = (
            run.design_profile_surface.as_deref(),
            run.design_profile_template.as_deref(),
            run.design_profile_effective_hash.as_deref(),
        ) {
            match profile.effective_for(surface, template) {
                Ok(effective) if effective.effective_profile_hash == expected_hash => {}
                _ => {
                    return Some((
                        "needs_user_input:design_profile_integrity_failed".to_string(),
                        "Effective DesignProfile hash or template resolution changed.".to_string(),
                    ))
                }
            }
        }
        if profile.schema_version == crate::types::DESIGN_PROFILE_SCHEMA_V1 {
            self.store
                .append_audit_record(
                    &run.project_id,
                    &run.id,
                    "design_profile.legacy_source",
                    "schemaVersion=design-profile@1",
                    "allow",
                    "legacy-warning: source artifact verification unavailable",
                )
                .await;
            return None;
        }
        if profile.source.get("kind").and_then(Value::as_str) != Some("imported") {
            return None;
        }
        if profile.source.get("integrity").and_then(Value::as_str) != Some("verified") {
            return Some((
                "needs_user_input:design_profile_integrity_failed".to_string(),
                "Imported DesignProfile source integrity is not verified.".to_string(),
            ));
        }
        let Some(artifact_id) = run.design_source_artifact_id.as_deref() else {
            return Some((
                "needs_user_input:design_profile_source_missing".to_string(),
                "Imported DesignProfile source artifact is missing from the run snapshot."
                    .to_string(),
            ));
        };
        let Some(artifact) = self.store.get_design_source_artifact(artifact_id).await else {
            return Some((
                "needs_user_input:design_profile_source_missing".to_string(),
                "Imported DesignProfile source artifact metadata is missing.".to_string(),
            ));
        };
        if run.design_source_hash.as_deref() != Some(artifact.sha256.as_str())
            || profile.source.get("sourceHash").and_then(Value::as_str)
                != Some(artifact.sha256.as_str())
        {
            return Some((
                "needs_user_input:design_profile_integrity_failed".to_string(),
                "Imported DesignProfile source hash does not match the immutable artifact."
                    .to_string(),
            ));
        }
        if self
            .store
            .read_design_source_artifact_content(artifact_id)
            .await
            .is_err()
        {
            return Some((
                "needs_user_input:design_profile_integrity_failed".to_string(),
                "Imported DesignProfile source bytes failed integrity verification.".to_string(),
            ));
        }
        None
    }
}
