use super::super::*;

pub(in crate::http_api) fn internal_admin_authorized(
    config: &RuntimeConfig,
    headers: &HeaderMap,
) -> bool {
    let Some(expected_token) = config.internal_admin_token.as_deref() else {
        return false;
    };
    let internal = headers
        .get("x-anydesign-internal")
        .and_then(|value| value.to_str().ok())
        == Some("true");
    let token = headers
        .get("x-runtime-admin-token")
        .and_then(|value| value.to_str().ok());
    internal && token == Some(expected_token)
}
