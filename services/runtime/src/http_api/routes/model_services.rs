use super::super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelServicesQuery {
    phase: String,
    agent_profile: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableModelService {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub capabilities: Value,
    pub availability: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AvailableModelServicesResponse {
    pub items: Vec<AvailableModelService>,
}

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new().route(
        "/projects/{project_id}/model-services",
        get(list_model_services),
    )
}

async fn list_model_services(
    State(state): State<AppState>,
    Extension(policy): Extension<ApplicationAuthorizationPolicy>,
    Path(project_id): Path<String>,
    Query(query): Query<ModelServicesQuery>,
    headers: HeaderMap,
) -> Result<Json<AvailableModelServicesResponse>, (StatusCode, Json<ErrorResponse>)> {
    authorize_project_operation(
        &state,
        &policy,
        &headers,
        &project_id,
        PROJECT_READ_OPERATION,
    )
    .await?;
    if !matches!(query.phase.as_str(), "build" | "edit" | "repair")
        || query.agent_profile.trim().is_empty()
    {
        return Err(bad_request(
            "phase must be build, edit, or repair and agentProfile is required".to_string(),
        ));
    }
    let access = state.store.get_project_access(&project_id).await;
    let workspace_id = access
        .as_ref()
        .map(|access| access.workspace_namespace.clone())
        .unwrap_or_else(|| "ws-runtime-local".to_string());
    let mut url = reqwest::Url::parse(&state.config.model_gateway_url)
        .map_err(|error| internal_error(anyhow::anyhow!(error)))?;
    url.set_path("/v1/model-services");
    url.set_query(None);
    url.query_pairs_mut()
        .append_pair("workspaceId", &workspace_id)
        .append_pair("projectId", &project_id)
        .append_pair("phase", &query.phase)
        .append_pair("agentProfile", &query.agent_profile);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|error| internal_error(anyhow::anyhow!(error)))?;
    let mut request = client.get(url);
    if let Some(token) = state.config.model_gateway_auth_token.as_deref() {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(model_catalog_error)?;
    if !response.status().is_success() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Provider Gateway model catalog is unavailable".to_string(),
                error_code: Some("model_service_catalog_unavailable".to_string()),
            }),
        ));
    }
    let catalog = response
        .json::<AvailableModelServicesResponse>()
        .await
        .map_err(model_catalog_error)?;
    Ok(Json(catalog))
}

fn model_catalog_error(error: reqwest::Error) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: format!("Provider Gateway model catalog is unavailable: {error}"),
            error_code: Some("model_service_catalog_unavailable".to_string()),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{PublicPrincipalAuthMode, RuntimePolicyProfile};
    use axum::{
        body::{to_bytes, Body},
        http::Request,
        routing::get,
    };
    use serde_json::json;
    use tokio::net::TcpListener;
    use tower::ServiceExt;

    #[tokio::test]
    async fn catalog_is_forwarded_with_project_scope_and_service_auth() {
        let upstream = Router::new().route(
            "/v1/model-services",
            get(
                |headers: HeaderMap,
                 Query(query): Query<std::collections::HashMap<String, String>>| async move {
                    assert_eq!(headers["authorization"], "Bearer gateway-token");
                    assert_eq!(
                        query.get("projectId").map(String::as_str),
                        Some("project-1")
                    );
                    assert_eq!(query.get("phase").map(String::as_str), Some("build"));
                    Json(json!({
                        "items": [{
                            "id": "model-service-1",
                            "displayName": "Balanced Model",
                            "description": "General generation model",
                            "capabilities": {},
                            "availability": "available"
                        }]
                    }))
                },
            ),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });

        let mut config = crate::RuntimeConfig::from_env();
        config.public_principal_auth_mode = PublicPrincipalAuthMode::Disabled;
        config.policy_profile = RuntimePolicyProfile::LocalE2e;
        config.model_gateway_url = format!("http://{address}");
        config.model_gateway_auth_token = Some("gateway-token".to_string());
        let response = crate::http_api::router(config)
            .oneshot(
                Request::builder()
                    .uri("/projects/project-1/model-services?phase=build&agentProfile=build")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 16 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(body["items"][0]["id"], "model-service-1");
        assert!(body["items"][0].get("providerId").is_none());
        server.abort();
    }
}
