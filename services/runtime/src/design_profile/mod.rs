mod capability;
mod source;
mod validation;

#[cfg(test)]
mod tests;

pub use capability::{
    registered_template_spec, signature_rule_applies_to_surface,
    unsupported_extended_tokens_for_template,
};
pub use source::{parse_design_profile_source, ParsedDesignProfileSource};
pub use validation::{design_profile_candidate_issues, normalize_component_roles};

pub fn scope_with_project_id(
    scope: serde_json::Value,
    project_id: Option<&str>,
) -> serde_json::Value {
    let mut object = match scope {
        serde_json::Value::Object(object) => object,
        _ => serde_json::Map::new(),
    };
    if let Some(project_id) = project_id {
        object
            .entry("projectId".to_string())
            .or_insert_with(|| serde_json::Value::String(project_id.to_string()));
    }
    serde_json::Value::Object(object)
}
