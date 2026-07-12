use super::{diff::diff_profiles, store_error, DesignProfileServiceError, ProfileDiffChange};
use crate::{
    conversation::RuntimeStore,
    design_profile::{
        design_profile_candidate_issues, normalize_component_roles, scope_with_project_id,
    },
    project::resolve_built_in_template_for_init,
    types::{
        DesignProfile, DesignProfileConversionReport, DESIGN_PROFILE_SCHEMA_V1,
        DESIGN_PROFILE_SCHEMA_V2,
    },
};
use chrono::Utc;
use serde_json::{json, Map, Value};
use std::collections::HashSet;

#[derive(Clone)]
pub struct DesignProfileService {
    pub(super) store: RuntimeStore,
}

pub struct CreateProfileCommand {
    pub project_id: Option<String>,
    pub name: String,
    pub payload: Map<String, Value>,
}

pub struct UpdateProfileCommand {
    pub design_profile_id: String,
    pub expected_version: Option<u32>,
    pub name: String,
    pub profile: Value,
}

pub struct ListProfilesQuery {
    pub project_id: Option<String>,
    pub workspace_id: Option<String>,
    pub organization_id: Option<String>,
    pub include_archived: bool,
}

impl DesignProfileService {
    pub fn new(store: RuntimeStore) -> Self {
        Self { store }
    }

    pub async fn create(
        &self,
        command: CreateProfileCommand,
    ) -> Result<DesignProfile, DesignProfileServiceError> {
        required("name", &command.name)?;
        let now = Utc::now();
        let payload = command.payload;
        let mut profile = DesignProfile {
            id: self.store.next_id("design-profile"),
            schema_version: payload
                .get("schemaVersion")
                .and_then(Value::as_str)
                .unwrap_or(DESIGN_PROFILE_SCHEMA_V1)
                .to_string(),
            name: command.name,
            status: payload_string(&payload, "status")?,
            version: 1,
            scope: scope_with_project_id(
                payload_value(&payload, "scope").unwrap_or(Value::Null),
                command.project_id.as_deref(),
            ),
            source: payload_value(&payload, "source")
                .unwrap_or_else(|| json!({ "kind": "manual" })),
            product: required_value(&payload, "product")?,
            brand: required_value(&payload, "brand")?,
            visual: required_value(&payload, "visual")?,
            tokens: required_value(&payload, "tokens")?,
            runtime_token_mapping: required_value(&payload, "runtimeTokenMapping")?,
            extended_token_mapping: payload_value(&payload, "extendedTokenMapping")
                .unwrap_or_else(|| json!({})),
            components: required_value(&payload, "components")?,
            content: required_value(&payload, "content")?,
            accessibility: required_value(&payload, "accessibility")?,
            technical: required_value(&payload, "technical")?,
            governance: required_value(&payload, "governance")?,
            signature_rules: payload
                .get("signatureRules")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            overrides: payload_value(&payload, "overrides").unwrap_or_else(|| json!({})),
            created_at: now,
            updated_at: now,
        };
        normalize_component_roles(&mut profile.components)
            .map_err(DesignProfileServiceError::InvalidRequest)?;
        validate_templates(&profile).await?;
        self.validate_source_reference(&profile).await?;
        self.store
            .create_design_profile(profile)
            .await
            .map_err(store_error)
    }

    pub async fn list(&self, query: ListProfilesQuery) -> Vec<Value> {
        let active = self
            .store
            .list_design_profiles(
                query.project_id.as_deref(),
                query.workspace_id.as_deref(),
                query.organization_id.as_deref(),
                query.include_archived,
            )
            .await;
        let drafts = self
            .store
            .list_design_profile_drafts(
                query.project_id.as_deref(),
                query.workspace_id.as_deref(),
                query.organization_id.as_deref(),
            )
            .await;
        let active_ids = active
            .iter()
            .map(|profile| profile.id.clone())
            .collect::<HashSet<_>>();
        let mut records = active.into_iter().map(to_value).collect::<Vec<_>>();
        records.extend(
            drafts
                .into_iter()
                .filter(|draft| !active_ids.contains(&draft.id))
                .map(to_value),
        );
        records
    }

    pub async fn get(&self, id: &str) -> Result<Value, DesignProfileServiceError> {
        required("designProfileId", id)?;
        if let Some(profile) = self.store.get_design_profile(id).await {
            return Ok(to_value(profile));
        }
        self.store
            .get_design_profile_draft(id)
            .await
            .map(to_value)
            .ok_or_else(|| {
                DesignProfileServiceError::NotFound(format!("design profile not found: {id}"))
            })
    }

    pub async fn versions(&self, id: &str) -> Result<Vec<Value>, DesignProfileServiceError> {
        required("designProfileId", id)?;
        let active = self
            .store
            .design_profile_versions(id)
            .await
            .map_err(store_error)?;
        let drafts = self
            .store
            .design_profile_draft_versions(id)
            .await
            .map_err(store_error)?;
        let mut versions = active
            .into_iter()
            .map(to_value)
            .chain(drafts.into_iter().map(to_value))
            .collect::<Vec<_>>();
        versions.sort_by_key(|record| record.get("version").and_then(Value::as_u64).unwrap_or(0));
        if versions.is_empty() {
            return Err(DesignProfileServiceError::NotFound(format!(
                "design profile not found: {id}"
            )));
        }
        Ok(versions)
    }

    pub async fn diff(
        &self,
        id: &str,
        from: u32,
        to: u32,
    ) -> Result<Vec<ProfileDiffChange>, DesignProfileServiceError> {
        required("designProfileId", id)?;
        if from == 0 || to == 0 {
            return Err(DesignProfileServiceError::InvalidRequest(
                "fromVersion and toVersion must be positive".to_string(),
            ));
        }
        let versions = self
            .store
            .design_profile_versions(id)
            .await
            .map_err(store_error)?;
        if versions.is_empty() {
            return Err(DesignProfileServiceError::NotFound(format!(
                "design profile not found: {id}"
            )));
        }
        let from_profile = versions
            .iter()
            .find(|profile| profile.version == from)
            .ok_or_else(|| {
                DesignProfileServiceError::NotFound(format!(
                    "design profile version not found: {id}@{from}"
                ))
            })?;
        let to_profile = versions
            .iter()
            .find(|profile| profile.version == to)
            .ok_or_else(|| {
                DesignProfileServiceError::NotFound(format!(
                    "design profile version not found: {id}@{to}"
                ))
            })?;
        Ok(diff_profiles(from_profile, to_profile))
    }

    pub async fn archive(&self, id: &str) -> Result<DesignProfile, DesignProfileServiceError> {
        required("designProfileId", id)?;
        self.store
            .archive_design_profile(id)
            .await
            .map_err(store_error)
    }

    pub async fn activate(
        &self,
        id: &str,
        expected_version: u32,
    ) -> Result<DesignProfile, DesignProfileServiceError> {
        required("designProfileId", id)?;
        let draft = self
            .store
            .get_design_profile_draft(id)
            .await
            .ok_or_else(|| {
                DesignProfileServiceError::NotFound(format!("design profile draft not found: {id}"))
            })?;
        if draft.version != expected_version {
            return Err(DesignProfileServiceError::ActivationConflict {
                message: "design profile version conflict".to_string(),
                current_version: draft.version,
                validation_issues: draft.validation_issues,
            });
        }
        let now = Utc::now();
        let mut value = draft.candidate.clone();
        let object = value.as_object_mut().ok_or_else(|| {
            activation_error(
                "draft candidate must be an object".to_string(),
                draft.version,
                design_profile_candidate_issues(&draft.candidate, true),
            )
        })?;
        object.insert("id".to_string(), json!(draft.id));
        object.insert("schemaVersion".to_string(), json!(DESIGN_PROFILE_SCHEMA_V2));
        object.insert("name".to_string(), json!(draft.name));
        object.insert("status".to_string(), json!("active"));
        object.insert("version".to_string(), json!(draft.version + 1));
        object.insert("scope".to_string(), draft.scope.clone());
        object.insert("source".to_string(), draft.source.clone());
        object.insert("createdAt".to_string(), json!(draft.created_at));
        object.insert("updatedAt".to_string(), json!(now));
        let mut profile: DesignProfile = serde_json::from_value(value).map_err(|error| {
            activation_error(
                format!("draft activation validation failed: {error}"),
                draft.version,
                design_profile_candidate_issues(&draft.candidate, true),
            )
        })?;
        normalize_component_roles(&mut profile.components)
            .map_err(DesignProfileServiceError::InvalidRequest)?;
        if let Err(error) = profile.validate_for_runtime() {
            return Err(activation_error(
                format!("draft activation validation failed: {error}"),
                draft.version,
                vec![crate::types::DesignProfileValidationIssue {
                    path: "candidate".to_string(),
                    code: "runtime_validation".to_string(),
                    message: error,
                    blocking: true,
                }],
            ));
        }
        validate_templates(&profile).await?;
        self.validate_source_reference(&profile).await?;
        self.store
            .create_design_profile(profile)
            .await
            .map_err(store_error)
    }

    pub async fn update(
        &self,
        command: UpdateProfileCommand,
    ) -> Result<Value, DesignProfileServiceError> {
        required("designProfileId", &command.design_profile_id)?;
        required("name", &command.name)?;
        let existing = self
            .store
            .get_design_profile(&command.design_profile_id)
            .await;
        if existing.is_none() {
            self.store
                .get_design_profile_draft(&command.design_profile_id)
                .await
                .ok_or_else(|| {
                    DesignProfileServiceError::NotFound(format!(
                        "design profile not found: {}",
                        command.design_profile_id
                    ))
                })?;
            let expected = command.expected_version.ok_or_else(|| {
                DesignProfileServiceError::InvalidRequest(
                    "expectedVersion is required when updating a draft".to_string(),
                )
            })?;
            let issues = design_profile_candidate_issues(&command.profile, true);
            let draft = self
                .store
                .update_design_profile_draft(
                    &command.design_profile_id,
                    expected,
                    command.name,
                    command.profile,
                    issues,
                )
                .await
                .map_err(store_error)?;
            return Ok(to_value(draft));
        }
        let existing = existing.expect("profile checked");
        if existing.schema_version == DESIGN_PROFILE_SCHEMA_V2 {
            let expected = command.expected_version.ok_or_else(|| {
                DesignProfileServiceError::InvalidRequest(
                    "expectedVersion is required when updating a V2 profile".to_string(),
                )
            })?;
            if expected != existing.version {
                return Err(DesignProfileServiceError::Conflict(format!(
                    "design profile version conflict: expected {expected}, current {}",
                    existing.version
                )));
            }
        }
        let payload = command.profile.as_object().cloned().ok_or_else(|| {
            DesignProfileServiceError::InvalidRequest("profile must be an object".to_string())
        })?;
        let mut profile = DesignProfile {
            id: existing.id,
            schema_version: payload
                .get("schemaVersion")
                .and_then(Value::as_str)
                .unwrap_or(&existing.schema_version)
                .to_string(),
            name: command.name,
            status: payload_string(&payload, "status")?,
            version: existing.version + 1,
            scope: required_value(&payload, "scope")?,
            source: payload_value(&payload, "source").unwrap_or(existing.source),
            product: required_value(&payload, "product")?,
            brand: required_value(&payload, "brand")?,
            visual: required_value(&payload, "visual")?,
            tokens: required_value(&payload, "tokens")?,
            runtime_token_mapping: required_value(&payload, "runtimeTokenMapping")?,
            extended_token_mapping: payload_value(&payload, "extendedTokenMapping")
                .unwrap_or(existing.extended_token_mapping),
            components: required_value(&payload, "components")?,
            content: required_value(&payload, "content")?,
            accessibility: required_value(&payload, "accessibility")?,
            technical: required_value(&payload, "technical")?,
            governance: required_value(&payload, "governance")?,
            signature_rules: payload
                .get("signatureRules")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or(existing.signature_rules),
            overrides: payload_value(&payload, "overrides").unwrap_or(existing.overrides),
            created_at: existing.created_at,
            updated_at: Utc::now(),
        };
        normalize_component_roles(&mut profile.components)
            .map_err(DesignProfileServiceError::InvalidRequest)?;
        validate_templates(&profile).await?;
        self.validate_source_reference(&profile).await?;
        self.store
            .create_design_profile(profile)
            .await
            .map(to_value)
            .map_err(store_error)
    }

    pub async fn bind_project(
        &self,
        project_id: &str,
        profile_id: &str,
    ) -> Result<DesignProfile, DesignProfileServiceError> {
        required("projectId", project_id)?;
        required("designProfileId", profile_id)?;
        if self.store.get_design_profile(profile_id).await.is_none()
            && self
                .store
                .get_design_profile_draft(profile_id)
                .await
                .is_some()
        {
            return Err(DesignProfileServiceError::Conflict(
                "draft design profile cannot be bound to a project".to_string(),
            ));
        }
        if let Some(profile) = self.store.get_design_profile(profile_id).await {
            validate_templates(&profile).await?;
        }
        self.store
            .bind_project_design_profile(project_id, profile_id)
            .await
            .map_err(store_error)
    }

    pub async fn project_profile(
        &self,
        project_id: &str,
    ) -> Result<Option<DesignProfile>, DesignProfileServiceError> {
        required("projectId", project_id)?;
        Ok(self.store.project_design_profile(project_id).await)
    }

    pub async fn conversion_report(
        &self,
        id: &str,
        version: Option<u32>,
    ) -> Result<DesignProfileConversionReport, DesignProfileServiceError> {
        required("designProfileId", id)?;
        if version == Some(0) {
            return Err(DesignProfileServiceError::InvalidRequest(
                "version must be positive".to_string(),
            ));
        }
        self.store
            .design_profile_conversion_report(id, version)
            .await
            .map_err(store_error)?
            .ok_or_else(|| {
                let suffix = version
                    .map(|version| format!("@{version}"))
                    .unwrap_or_default();
                DesignProfileServiceError::NotFound(format!(
                    "design profile conversion report not found: {id}{suffix}"
                ))
            })
    }

    async fn validate_source_reference(
        &self,
        profile: &DesignProfile,
    ) -> Result<(), DesignProfileServiceError> {
        let Some(artifact_id) = profile
            .source
            .get("primarySourceArtifactId")
            .and_then(Value::as_str)
        else {
            return Ok(());
        };
        required("profile.source.primarySourceArtifactId", artifact_id)?;
        let artifact = self
            .store
            .get_design_source_artifact(artifact_id)
            .await
            .ok_or_else(|| {
                DesignProfileServiceError::NotFound(format!(
                    "design source artifact not found: {artifact_id}"
                ))
            })?;
        if artifact.scope != profile.scope {
            return Err(DesignProfileServiceError::InvalidRequest(
                "profile source artifact scope must exactly match profile scope".to_string(),
            ));
        }
        let source_hash = profile
            .source
            .get("sourceHash")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                DesignProfileServiceError::InvalidRequest(
                    "profile.source.sourceHash is required with primarySourceArtifactId"
                        .to_string(),
                )
            })?;
        if !artifact.sha256.eq_ignore_ascii_case(source_hash) {
            return Err(DesignProfileServiceError::InvalidRequest(
                "profile.source.sourceHash does not match the referenced artifact".to_string(),
            ));
        }
        self.store
            .read_design_source_artifact_content(artifact_id)
            .await
            .map_err(|error| {
                let message = error.to_string();
                if message.contains("design source artifact not found") {
                    DesignProfileServiceError::NotFound(message)
                } else if message.contains("invalid design source artifact") {
                    DesignProfileServiceError::InvalidRequest(message)
                } else {
                    DesignProfileServiceError::Internal(message)
                }
            })?;
        Ok(())
    }
}

async fn validate_templates(profile: &DesignProfile) -> Result<(), DesignProfileServiceError> {
    let allowed = profile
        .technical
        .get("allowedTemplates")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            DesignProfileServiceError::Conflict(
                "technical.allowedTemplates is required".to_string(),
            )
        })?;
    for template in allowed {
        let template = template.as_str().ok_or_else(|| {
            DesignProfileServiceError::Conflict(
                "technical.allowedTemplates must contain strings".to_string(),
            )
        })?;
        resolve_built_in_template_for_init(template)
            .await
            .map_err(|error| {
                DesignProfileServiceError::Conflict(format!("{}: {error}", error.error_kind()))
            })?;
    }
    Ok(())
}

fn required(field: &str, value: &str) -> Result<(), DesignProfileServiceError> {
    if value.trim().is_empty() {
        Err(DesignProfileServiceError::InvalidRequest(format!(
            "{field} must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn payload_string(
    payload: &Map<String, Value>,
    field: &str,
) -> Result<String, DesignProfileServiceError> {
    let value = payload.get(field).and_then(Value::as_str).ok_or_else(|| {
        DesignProfileServiceError::InvalidRequest(format!("profile.{field} must be a string"))
    })?;
    required(&format!("profile.{field}"), value)?;
    Ok(value.to_string())
}

fn required_value(
    payload: &Map<String, Value>,
    field: &str,
) -> Result<Value, DesignProfileServiceError> {
    payload.get(field).cloned().ok_or_else(|| {
        DesignProfileServiceError::InvalidRequest(format!("profile.{field} is required"))
    })
}

fn payload_value(payload: &Map<String, Value>, field: &str) -> Option<Value> {
    payload.get(field).cloned()
}
fn to_value(value: impl serde::Serialize) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}
fn activation_error(
    message: String,
    current_version: u32,
    validation_issues: Vec<crate::types::DesignProfileValidationIssue>,
) -> DesignProfileServiceError {
    DesignProfileServiceError::ActivationConflict {
        message,
        current_version,
        validation_issues,
    }
}
