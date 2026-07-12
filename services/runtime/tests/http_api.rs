use anydesign_runtime::{http_api, types::AgentPhase};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use suite::phase_a_contract_config;
use tower::ServiceExt;

#[path = "http_api/contract_manifest.rs"]
mod contract_manifest;
#[path = "http_api/routes/mod.rs"]
mod route_families;
#[path = "http_api/suite/mod.rs"]
mod suite;
