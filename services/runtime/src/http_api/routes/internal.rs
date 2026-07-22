mod design_context_canary;
mod design_context_enforcement;
mod generation_context_metrics;
mod preview_promotion;
mod project_access;
mod release_evidence;
mod sandbox_release;
mod template_build;
mod visual_artifact;

use super::super::*;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .merge(template_build::router())
        .merge(preview_promotion::router())
        .merge(project_access::router())
        .merge(design_context_canary::router())
        .merge(design_context_enforcement::router())
        .merge(generation_context_metrics::router())
        .merge(release_evidence::router())
        .merge(sandbox_release::router())
        .merge(visual_artifact::router())
}
