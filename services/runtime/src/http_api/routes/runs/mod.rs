mod cancel;
mod continue_run;
mod design_context;
mod permission;
pub(in crate::http_api) mod start;

use super::super::AppState;
use axum::Router;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .merge(start::router())
        .merge(continue_run::router())
        .merge(cancel::router())
        .merge(design_context::router())
        .merge(permission::router())
}
