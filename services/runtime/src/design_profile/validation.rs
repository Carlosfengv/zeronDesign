use crate::types::DesignProfileValidationIssue;
use serde_json::Value;

pub fn normalize_component_roles(components: &mut Value) -> Result<(), String> {
    let Some(primitives) = components
        .get_mut("primitives")
        .and_then(Value::as_object_mut)
    else {
        return Ok(());
    };
    for (name, guideline) in primitives {
        let Some(guideline) = guideline.as_object_mut() else {
            continue;
        };
        let role = guideline.get("role").and_then(Value::as_str);
        let intent = guideline.get("intent").and_then(Value::as_str);
        if let (Some(role), Some(intent)) = (role, intent) {
            if role != intent {
                return Err(format!(
                    "components.primitives.{name}.role conflicts with legacy intent"
                ));
            }
        }
        let canonical_role = role.or(intent).map(ToString::to_string);
        if let Some(canonical_role) = canonical_role {
            guideline.insert("role".to_string(), Value::String(canonical_role));
            guideline.remove("intent");
        }
    }
    Ok(())
}

pub fn design_profile_candidate_issues(
    candidate: &Value,
    imported: bool,
) -> Vec<DesignProfileValidationIssue> {
    let required_fields = [
        "product",
        "brand",
        "visual",
        "tokens",
        "runtimeTokenMapping",
        "components",
        "content",
        "accessibility",
        "technical",
        "governance",
    ];
    let mut issues = Vec::new();
    let object = match candidate.as_object() {
        Some(object) => object,
        None => {
            issues.push(DesignProfileValidationIssue {
                path: "candidate".to_string(),
                code: "invalid_type".to_string(),
                message: "candidate must be an object".to_string(),
                blocking: true,
            });
            return issues;
        }
    };
    for field in required_fields {
        if !object.contains_key(field) {
            issues.push(DesignProfileValidationIssue {
                path: field.to_string(),
                code: "required".to_string(),
                message: format!("{field} is required before activation"),
                blocking: true,
            });
        }
    }
    if imported
        && object
            .get("signatureRules")
            .and_then(Value::as_array)
            .is_none_or(|rules| {
                !rules
                    .iter()
                    .any(|rule| rule.get("priority").and_then(Value::as_str) == Some("required"))
            })
    {
        issues.push(DesignProfileValidationIssue {
            path: "signatureRules".to_string(),
            code: "required_signature_rule".to_string(),
            message: "imported profile requires at least one required signature rule".to_string(),
            blocking: true,
        });
    }
    issues
}
