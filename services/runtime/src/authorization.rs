use crate::{conversation::RuntimeStore, public_principal::PublicPrincipal};
use std::{error::Error, fmt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedPrincipal {
    pub principal_id: String,
    pub project_id: String,
}

impl From<PublicPrincipal> for AuthenticatedPrincipal {
    fn from(principal: PublicPrincipal) -> Self {
        Self {
            principal_id: principal.principal_id,
            project_id: principal.project_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizationPolicyError {
    Forbidden,
    Conflict(String),
}

impl fmt::Display for AuthorizationPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Forbidden => formatter.write_str("public_auth.project_forbidden"),
            Self::Conflict(message) => formatter.write_str(message),
        }
    }
}

impl Error for AuthorizationPolicyError {}

#[derive(Clone)]
pub struct ApplicationAuthorizationPolicy {
    store: RuntimeStore,
}

impl ApplicationAuthorizationPolicy {
    pub fn new(store: RuntimeStore) -> Self {
        Self { store }
    }

    pub async fn authorize_project_owner(
        &self,
        principal: &AuthenticatedPrincipal,
        project_id: &str,
    ) -> Result<(), AuthorizationPolicyError> {
        if principal.project_id != project_id {
            return Err(AuthorizationPolicyError::Forbidden);
        }
        let access = self
            .store
            .get_project_access(project_id)
            .await
            .ok_or(AuthorizationPolicyError::Forbidden)?;
        if access.project_id != project_id {
            return Err(AuthorizationPolicyError::Conflict(
                "project access identity drift detected".to_string(),
            ));
        }
        if access.owner_principal_id != principal.principal_id {
            return Err(AuthorizationPolicyError::Forbidden);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn project_owner_policy_fails_closed_without_matching_access() {
        let store = RuntimeStore::new();
        let policy = ApplicationAuthorizationPolicy::new(store.clone());
        let owner = AuthenticatedPrincipal {
            principal_id: "owner-1".to_string(),
            project_id: "project-1".to_string(),
        };
        assert_eq!(
            policy.authorize_project_owner(&owner, "project-1").await,
            Err(AuthorizationPolicyError::Forbidden)
        );

        store
            .upsert_project_access("project-1", "owner-1".to_string(), None, None)
            .await
            .unwrap();
        assert_eq!(
            policy.authorize_project_owner(&owner, "project-1").await,
            Ok(())
        );

        let cross_project = AuthenticatedPrincipal {
            principal_id: "owner-1".to_string(),
            project_id: "project-2".to_string(),
        };
        assert_eq!(
            policy
                .authorize_project_owner(&cross_project, "project-1")
                .await,
            Err(AuthorizationPolicyError::Forbidden)
        );
    }
}
