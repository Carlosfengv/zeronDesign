use super::super::AppState;
use super::previews::{
    candidate_capture_file, candidate_capture_host_file, candidate_capture_host_root,
    candidate_capture_root,
};
use axum::{routing::get, Router};

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .route("/preview-captures/{lease_id}", get(candidate_capture_root))
        .route("/preview-captures/{lease_id}/", get(candidate_capture_root))
        .route(
            "/preview-captures/{lease_id}/{*preview_path}",
            get(candidate_capture_file),
        )
        .route("/", get(candidate_capture_host_root))
        .route("/{*preview_path}", get(candidate_capture_host_file))
}
