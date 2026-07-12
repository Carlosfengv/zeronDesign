mod cancel;
mod continue_run;
mod permission;
pub(in crate::http_api) mod start;

use super::super::AppState;
use axum::Router;

pub(in crate::http_api) fn router() -> Router<AppState> {
    Router::new()
        .merge(start::router())
        .merge(continue_run::router())
        .merge(cancel::router())
        .merge(permission::router())
}
