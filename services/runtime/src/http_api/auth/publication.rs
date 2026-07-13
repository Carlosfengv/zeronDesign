use super::super::*;

pub(in crate::http_api) async fn authorize_publication(
    state: &AppState,
    policy: &ApplicationAuthorizationPolicy,
    headers: &HeaderMap,
    project_id: &str,
    required_operation: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(state, policy, headers, project_id, required_operation).await?;
    Ok(())
}
