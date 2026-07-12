use crate::templates::{BuiltInTemplateRegistry, TemplateId, TemplateRegistry, TemplateSpec};
use serde_json::Value;
use std::{collections::HashSet, sync::Arc};

pub fn signature_rule_applies_to_surface(rule: &Value, surface: &str) -> bool {
    match rule.get("appliesTo") {
        Some(Value::String(value)) => value == "all",
        Some(Value::Array(values)) => values.iter().any(|value| value.as_str() == Some(surface)),
        _ => false,
    }
}

pub fn unsupported_extended_tokens_for_template(mapping: &Value, template: &str) -> Vec<String> {
    let supported = registered_template_spec(template)
        .map(|spec| {
            spec.style
                .tokens
                .iter()
                .map(|token| token.name)
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let mut unsupported = mapping
        .as_object()
        .map(|tokens| {
            tokens
                .keys()
                .filter(|token| !supported.contains(token.as_str()))
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    unsupported.sort();
    unsupported
}

pub fn registered_template_spec(template: &str) -> Option<Arc<TemplateSpec>> {
    let id = TemplateId::parse(template).ok()?;
    BuiltInTemplateRegistry::built_in().current(&id).ok()
}
