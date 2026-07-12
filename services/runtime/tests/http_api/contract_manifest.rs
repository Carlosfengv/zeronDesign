use super::*;
use serde::Deserialize;
use std::collections::BTreeSet;

const HTTP_API_SOURCE: &str = include_str!("../../src/http_api.rs");
const ROUTE_MANIFEST_SOURCE: &str = include_str!("../../contracts/http-routes.json");

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RouteManifest {
    schema_version: String,
    source: String,
    baseline_commit: String,
    baseline_test_count: usize,
    routes: Vec<RouteContract>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RouteContract {
    surface: String,
    path: String,
    methods: Vec<String>,
    authorization: String,
    body_limit: String,
    feature_flag: Option<String>,
    response: String,
}

fn manifest() -> RouteManifest {
    serde_json::from_str(ROUTE_MANIFEST_SOURCE).expect("HTTP route manifest must be valid JSON")
}

fn router_section<'a>(start: &str, end: &str) -> &'a str {
    let start = HTTP_API_SOURCE
        .find(start)
        .unwrap_or_else(|| panic!("missing router section start: {start}"));
    let tail = &HTTP_API_SOURCE[start..];
    let end = tail
        .find(end)
        .unwrap_or_else(|| panic!("missing router section end: {end}"));
    &tail[..end]
}

fn route_calls(source: &str) -> Vec<&str> {
    let mut calls = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = source[cursor..].find(".route(") {
        let start = cursor + relative_start + ".route".len();
        let mut depth = 0_u32;
        let mut in_string = false;
        let mut escaped = false;
        let mut end = None;
        for (offset, character) in source[start..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == '"' {
                    in_string = false;
                }
                continue;
            }
            match character {
                '"' => in_string = true,
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + offset + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        let end = end.expect("route declaration must have balanced parentheses");
        calls.push(&source[start..end]);
        cursor = end;
    }
    calls
}

fn declared_routes(surface: &str, source: &str) -> BTreeSet<String> {
    route_calls(source)
        .into_iter()
        .flat_map(|route| {
            let first_quote = route
                .find('"')
                .expect("route path must be a string literal")
                + 1;
            let path_end = route[first_quote..]
                .find('"')
                .expect("route path string must terminate")
                + first_quote;
            let path = &route[first_quote..path_end];
            ["GET", "POST", "PUT", "DELETE", "PATCH"]
                .into_iter()
                .filter(move |method| {
                    let rust_method = method.to_ascii_lowercase();
                    route.contains(&format!("{rust_method}("))
                })
                .map(move |method| format!("{surface}|{method}|{path}"))
        })
        .collect()
}

#[test]
fn executable_route_manifest_matches_every_router_declaration() {
    let manifest = manifest();
    let expected = manifest
        .routes
        .iter()
        .flat_map(|route| {
            route
                .methods
                .iter()
                .map(|method| format!("{}|{}|{}", route.surface, method, route.path))
        })
        .collect::<BTreeSet<_>>();
    let mut actual = declared_routes(
        "public",
        router_section(
            "pub fn router_with_state",
            "pub fn capture_router_with_state",
        ),
    );
    actual.extend(declared_routes(
        "capture",
        router_section(
            "pub fn capture_router_with_state",
            "async fn candidate_capture_root",
        ),
    ));
    actual = actual
        .into_iter()
        .map(|entry| {
            if entry.contains("|/internal/") {
                entry.replacen("public|", "internal|", 1)
            } else {
                entry
            }
        })
        .collect();

    assert_eq!(actual, expected, "route manifest drifted from Axum routers");
}

#[test]
fn route_manifest_metadata_is_complete_and_fail_closed() {
    let manifest = manifest();
    assert_eq!(manifest.schema_version, "runtime-http-routes@1");
    assert_eq!(manifest.source, "src/http_api.rs");
    assert_eq!(manifest.baseline_test_count, 70);
    assert_eq!(manifest.baseline_commit.len(), 40);
    assert!(manifest.routes.len() >= 40);

    let allowed_authorization = [
        "none",
        "artifact_referer",
        "capture_listener_only",
        "internal_service",
        "project_access_in_production",
        "public_principal_when_required",
    ];
    for route in &manifest.routes {
        assert!(route.path.starts_with('/'));
        assert!(!route.methods.is_empty());
        assert!(allowed_authorization.contains(&route.authorization.as_str()));
        assert!(!route.body_limit.is_empty());
        assert!(!route.response.is_empty());
        if route.surface == "internal" {
            assert!(route.path.starts_with("/internal/"));
            assert_eq!(route.authorization, "internal_service");
        }
        if route.feature_flag.is_some() {
            assert_eq!(route.surface, "internal");
        }
    }
}

#[tokio::test]
async fn route_contract_freezes_json_error_status_and_shape() {
    let response = http_api::router(phase_a_contract_config())
        .oneshot(
            Request::builder()
                .uri("/runs/missing-run/events")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(response.headers()["content-type"], "application/json");
    let payload: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(payload, json!({ "error": "run not found: missing-run" }));
}

#[tokio::test]
async fn route_contract_freezes_sse_content_type_and_cache_policy() {
    let config = phase_a_contract_config();
    let state = http_api::app_state(config);
    let run = state
        .store
        .create_run(
            "contract-sse-project".to_string(),
            AgentPhase::Brief,
            "brief".to_string(),
            "contract-model".to_string(),
            vec![],
        )
        .await;
    let response = http_api::router_with_state(state)
        .oneshot(
            Request::builder()
                .uri(format!("/runs/{}/events", run.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    assert_eq!(response.headers()["cache-control"], "no-cache");
}

#[tokio::test]
async fn route_contract_freezes_design_source_request_body_limit() {
    let mut config = phase_a_contract_config();
    config.internal_admin_token = Some("contract-admin".to_string());
    let response = http_api::router(config)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/design-source-artifacts")
                .header("content-type", "application/json")
                .header("x-anydesign-internal", "true")
                .header("x-runtime-admin-token", "contract-admin")
                .body(Body::from(vec![b'x'; 393_217]))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
