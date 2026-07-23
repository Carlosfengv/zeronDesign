use super::*;
use serde::Deserialize;
use std::{collections::BTreeSet, fs, path::Path};

const HTTP_API_SOURCE: &str = include_str!("../../src/http_api/mod.rs");
const ARTIFACTS_ROUTES_SOURCE: &str = include_str!("../../src/http_api/routes/artifacts.rs");
const BRIEFS_ROUTES_SOURCE: &str = include_str!("../../src/http_api/routes/briefs.rs");
const CAPTURE_ROUTES_SOURCE: &str = include_str!("../../src/http_api/routes/capture.rs");
const DESIGN_SOURCES_ROUTES_SOURCE: &str =
    include_str!("../../src/http_api/routes/design_sources.rs");
const DESIGN_PROFILES_ROUTES_SOURCE: &str =
    include_str!("../../src/http_api/routes/design_profiles.rs");
const DRAFT_PREVIEW_EVENTS_ROUTES_SOURCE: &str =
    include_str!("../../src/http_api/routes/draft_preview_events.rs");
const PROJECTS_ROUTES_SOURCE: &str = include_str!("../../src/http_api/routes/projects.rs");
const MODEL_SERVICES_ROUTES_SOURCE: &str =
    include_str!("../../src/http_api/routes/model_services.rs");
const PREVIEWS_ROUTES_SOURCE: &str = include_str!("../../src/http_api/routes/previews.rs");
const PUBLICATION_ROUTES_SOURCE: &str = include_str!("../../src/http_api/routes/publication.rs");
const RUN_EVENTS_ROUTES_SOURCE: &str = include_str!("../../src/http_api/routes/run_events.rs");
const RUN_START_ROUTES_SOURCE: &str = include_str!("../../src/http_api/routes/runs/start.rs");
const RUN_CONTINUE_ROUTES_SOURCE: &str =
    include_str!("../../src/http_api/routes/runs/continue_run.rs");
const RUN_CANCEL_ROUTES_SOURCE: &str = include_str!("../../src/http_api/routes/runs/cancel.rs");
const RUN_DESIGN_CONTEXT_ROUTES_SOURCE: &str =
    include_str!("../../src/http_api/routes/runs/design_context.rs");
const RUN_METRICS_ROUTES_SOURCE: &str = include_str!("../../src/http_api/routes/runs/metrics.rs");
const RUN_PERMISSION_ROUTES_SOURCE: &str =
    include_str!("../../src/http_api/routes/runs/permission.rs");
const SYSTEM_ROUTES_SOURCE: &str = include_str!("../../src/http_api/routes/system.rs");
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

pub(super) fn assert_manifest_entries(surface: &str, entries: &[(&str, &str)]) {
    let manifest = manifest();
    for (method, path) in entries {
        assert!(
            manifest.routes.iter().any(|route| {
                route.surface == surface
                    && route.path == *path
                    && route.methods.iter().any(|candidate| candidate == method)
            }),
            "missing {surface} {method} {path} from executable route manifest"
        );
    }
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
    let production_source = source.split("#[cfg(test)]").next().unwrap_or(source);
    route_calls(production_source)
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

fn declared_routes_in_dir(surface: &str, root: &Path) -> BTreeSet<String> {
    let mut routes = BTreeSet::new();
    for entry in fs::read_dir(root).expect("route source directory must be readable") {
        let path = entry.expect("route source entry must be readable").path();
        if path.is_dir() {
            routes.extend(declared_routes_in_dir(surface, &path));
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            let source = fs::read_to_string(&path).expect("route source must be readable");
            routes.extend(declared_routes(surface, &source));
        }
    }
    routes
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
    actual.extend(declared_routes("public", ARTIFACTS_ROUTES_SOURCE));
    actual.extend(declared_routes("public", BRIEFS_ROUTES_SOURCE));
    actual.extend(declared_routes("public", DESIGN_PROFILES_ROUTES_SOURCE));
    actual.extend(declared_routes("public", DESIGN_SOURCES_ROUTES_SOURCE));
    actual.extend(declared_routes(
        "public",
        DRAFT_PREVIEW_EVENTS_ROUTES_SOURCE,
    ));
    actual.extend(declared_routes_in_dir(
        "internal",
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/http_api/routes/internal")
            .as_path(),
    ));
    actual.extend(declared_routes("public", PROJECTS_ROUTES_SOURCE));
    actual.extend(declared_routes("public", MODEL_SERVICES_ROUTES_SOURCE));
    actual.extend(declared_routes("public", PREVIEWS_ROUTES_SOURCE));
    actual.extend(declared_routes("public", PUBLICATION_ROUTES_SOURCE));
    actual.extend(declared_routes("public", RUN_EVENTS_ROUTES_SOURCE));
    actual.extend(declared_routes("public", RUN_START_ROUTES_SOURCE));
    actual.extend(declared_routes("public", RUN_CONTINUE_ROUTES_SOURCE));
    actual.extend(declared_routes("public", RUN_CANCEL_ROUTES_SOURCE));
    actual.extend(declared_routes("public", RUN_DESIGN_CONTEXT_ROUTES_SOURCE));
    actual.extend(declared_routes("public", RUN_METRICS_ROUTES_SOURCE));
    actual.extend(declared_routes("public", RUN_PERMISSION_ROUTES_SOURCE));
    actual.extend(declared_routes("public", SYSTEM_ROUTES_SOURCE));
    actual.extend(declared_routes("capture", CAPTURE_ROUTES_SOURCE));
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
    assert_eq!(manifest.source, "src/http_api/{mod.rs,routes/**/*.rs}");
    assert_eq!(manifest.baseline_test_count, 70);
    assert_eq!(manifest.baseline_commit.len(), 40);
    assert!(manifest.routes.len() >= 40);

    let allowed_authorization = [
        "none",
        "artifact_referer",
        "artifact_referer_and_preview_principal_when_required",
        "capture_listener_only",
        "design_profile_scope",
        "internal_service",
        "project_access_in_production",
        "project_principal_read_when_required",
        "project_principal_read_write_when_required",
        "project_principal_write_when_required",
        "preview_principal_when_required",
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
