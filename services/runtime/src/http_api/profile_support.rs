use super::*;

pub(in crate::http_api) use crate::design_profile::{
    design_profile_candidate_issues, parse_design_profile_source,
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
