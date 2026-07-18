use crate::{
    authorization::{
        ApplicationAuthorizationPolicy, AuthenticatedPrincipal, AuthorizationPolicyError,
    },
    channel_manager::ChannelManager,
    conversation::RuntimeStore,
    types::{sha256_hex, PreviewLeaseStatus},
};
use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy)]
pub enum PreviewAccessContext<'a> {
    Public(Option<&'a AuthenticatedPrincipal>),
    InternalCapture,
    InternalCaptureHost,
}

pub fn internal_capture_origin(base_url: &str, lease_id: &str) -> Option<String> {
    if lease_id.is_empty()
        || !lease_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    let mut url = reqwest::Url::parse(base_url).ok()?;
    url.set_host(Some(&format!("{lease_id}.preview.local")))
        .ok()?;
    url.set_path("/");
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidatePreviewAccess {
    pub lease_id: String,
    pub project_id: String,
    pub build_id: String,
    pub candidate_manifest_hash: String,
    pub upstream_endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewAccessError {
    NotFound(String),
    Forbidden(String),
    Conflict(String),
    Internal(String),
}

impl fmt::Display for PreviewAccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(message)
            | Self::Forbidden(message)
            | Self::Conflict(message)
            | Self::Internal(message) => formatter.write_str(message),
        }
    }
}

impl Error for PreviewAccessError {}

#[derive(Clone)]
pub struct PreviewAccessService {
    store: RuntimeStore,
    authorization: ApplicationAuthorizationPolicy,
}

impl PreviewAccessService {
    pub fn new(store: RuntimeStore, authorization: ApplicationAuthorizationPolicy) -> Self {
        Self {
            store,
            authorization,
        }
    }

    pub async fn resolve_candidate(
        &self,
        lease_id: &str,
        preview_path: &str,
        context: PreviewAccessContext<'_>,
    ) -> Result<CandidatePreviewAccess, PreviewAccessError> {
        validate_preview_path(preview_path)?;
        let lease = self
            .store
            .get_preview_lease(lease_id)
            .await
            .filter(|lease| lease.status == PreviewLeaseStatus::Active)
            .ok_or_else(|| {
                PreviewAccessError::NotFound("candidate preview lease is unavailable".to_string())
            })?;
        let run = self.store.get_run(&lease.run_id).await.ok_or_else(|| {
            PreviewAccessError::Conflict(
                "candidate preview lease references a missing run".to_string(),
            )
        })?;
        if lease.project_id != run.project_id {
            return Err(PreviewAccessError::Conflict(
                "candidate preview lease project identity drift detected".to_string(),
            ));
        }
        if let PreviewAccessContext::Public(Some(principal)) = context {
            let result = self
                .authorization
                .authorize_project_owner(principal, &run.project_id)
                .await;
            let (decision, reason) = match &result {
                Ok(()) => ("allow", "project-scoped public principal authorized"),
                Err(AuthorizationPolicyError::Forbidden) => {
                    ("deny", "principal does not own the preview project")
                }
                Err(AuthorizationPolicyError::Conflict(_)) => {
                    ("deny", "project access identity drift detected")
                }
            };
            self.store
                .append_audit_record(
                    &run.project_id,
                    &run.id,
                    "public.preview.read",
                    format!(
                        "leaseId={lease_id},principalHash={}",
                        sha256_hex(principal.principal_id.as_bytes())
                    ),
                    decision,
                    reason,
                )
                .await;
            result.map_err(|error| match error {
                AuthorizationPolicyError::Forbidden => {
                    PreviewAccessError::Forbidden(error.to_string())
                }
                AuthorizationPolicyError::Conflict(message) => {
                    PreviewAccessError::Conflict(message)
                }
            })?;
        }

        let binding = self
            .store
            .get_sandbox_binding(&lease.sandbox_binding_id)
            .await
            .ok_or_else(|| {
                PreviewAccessError::NotFound("candidate preview sandbox is unavailable".to_string())
            })?;
        if binding.sandbox_name != lease.sandbox_name
            || binding.pod_uid.as_deref() != Some(lease.pod_uid.as_str())
        {
            return Err(PreviewAccessError::Conflict(
                "candidate preview sandbox identity changed".to_string(),
            ));
        }
        let upstream_endpoint = ChannelManager::shared()
            .endpoint(&self.store, &binding, &lease.run_id, 4321, "http", "")
            .await
            .map_err(|error| PreviewAccessError::Internal(error.to_string()))?;
        Ok(CandidatePreviewAccess {
            lease_id: lease.id,
            project_id: lease.project_id,
            build_id: lease.build_id,
            candidate_manifest_hash: lease.candidate_manifest_hash,
            upstream_endpoint,
        })
    }
}

fn validate_preview_path(preview_path: &str) -> Result<(), PreviewAccessError> {
    if preview_path
        .split('/')
        .any(|component| component == ".." || component.contains('\\'))
    {
        return Err(PreviewAccessError::NotFound(
            "candidate preview path is invalid".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::internal_capture_origin;

    #[test]
    fn internal_capture_origin_uses_a_lease_scoped_host_and_preserves_the_port() {
        assert_eq!(
            internal_capture_origin("http://127.0.0.1:8081", "lease-123").as_deref(),
            Some("http://lease-123.preview.local:8081/")
        );
        assert!(internal_capture_origin("http://127.0.0.1:8081", "../lease").is_none());
    }
}
