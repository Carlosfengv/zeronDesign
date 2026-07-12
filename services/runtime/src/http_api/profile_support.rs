use super::*;

pub(in crate::http_api) use crate::design_profile::{
    design_profile_candidate_issues, parse_design_profile_source, registered_template_spec,
    scope_with_project_id, signature_rule_applies_to_surface,
    unsupported_extended_tokens_for_template,
};

pub(in crate::http_api) fn design_profile_payload_from_request(
    request: &CreateDesignProfileRequest,
) -> Result<Map<String, Value>, (StatusCode, Json<ErrorResponse>)> {
    if let Some(profile) = request.profile.as_ref() {
        return profile
            .as_object()
            .cloned()
            .ok_or_else(|| bad_request("profile must be an object".to_string()));
    }
    Ok(request.legacy_profile.clone())
}

pub(in crate::http_api) fn payload_string(
    payload: &Map<String, Value>,
    field: &str,
) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    let value = payload
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| bad_request(format!("profile.{field} must be a string")))?;
    validate_required_string(&format!("profile.{field}"), value)?;
    Ok(value.to_string())
}

pub(in crate::http_api) fn payload_required_value(
    payload: &Map<String, Value>,
    field: &str,
) -> Result<Value, (StatusCode, Json<ErrorResponse>)> {
    payload
        .get(field)
        .cloned()
        .ok_or_else(|| bad_request(format!("profile.{field} is required")))
}

pub(in crate::http_api) fn payload_value(
    payload: &Map<String, Value>,
    field: &str,
) -> Option<Value> {
    payload.get(field).cloned()
}

pub(in crate::http_api) fn normalize_design_profile_component_roles(
    components: &mut Value,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    crate::design_profile::normalize_component_roles(components).map_err(bad_request)
}
