use serde_json::{json, Value};
use std::collections::BTreeMap;

pub fn style_contract_identity(contract: &Value) -> Value {
    json!({
        "version": contract.get("version").cloned().unwrap_or(Value::Null),
        "template": contract.get("template").cloned().unwrap_or(Value::Null),
        "appRoot": contract.get("appRoot").cloned().unwrap_or(Value::Null),
        "tokens": contract.get("tokens").cloned().unwrap_or(Value::Null),
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CssVariableError {
    Missing,
    Ambiguous { match_count: usize },
    MissingSemicolon,
    InvalidValue(String),
}

impl std::fmt::Display for CssVariableError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(formatter, "CSS variable is missing from token file"),
            Self::Ambiguous { match_count } => {
                write!(
                    formatter,
                    "CSS variable is ambiguous in token file ({match_count} matches)"
                )
            }
            Self::MissingSemicolon => write!(formatter, "CSS variable is missing a semicolon"),
            Self::InvalidValue(message) => write!(formatter, "invalid token value: {message}"),
        }
    }
}

pub fn validate_token_value(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("token values must be non-empty".to_string());
    }
    if trimmed.len() > 256 {
        return Err("token values must be 256 characters or fewer".to_string());
    }
    if trimmed
        .chars()
        .any(|character| matches!(character, ';' | '{' | '}' | '\n' | '\r'))
    {
        return Err("token values may not contain ';', braces, or newlines".to_string());
    }
    Ok(())
}

pub fn read_contract_token_values(
    contract: &Value,
    token_file_content: &str,
) -> Result<BTreeMap<String, String>, String> {
    let tokens = contract
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or_else(|| "style contract is missing tokens map".to_string())?;
    let mut values = BTreeMap::new();
    for (token_name, css_variable) in tokens {
        let css_variable = css_variable
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                format!("style contract token {token_name} must map to a CSS variable")
            })?;
        let value = read_css_variable_value(token_file_content, css_variable)
            .map_err(|error| format!("token {token_name} {error}"))?;
        validate_token_value(&value)
            .map_err(|error| format!("token {token_name} has invalid current value: {error}"))?;
        values.insert(token_name.clone(), value);
    }
    Ok(values)
}

pub fn read_css_variable_value(
    content: &str,
    css_variable: &str,
) -> Result<String, CssVariableError> {
    let (_, value_start, value_end) = css_variable_value_bounds(content, css_variable)?;
    Ok(content[value_start..value_end].trim().to_string())
}

pub fn replace_css_variable_value(
    content: &str,
    css_variable: &str,
    new_value: &str,
) -> Result<(String, String), CssVariableError> {
    validate_token_value(new_value).map_err(CssVariableError::InvalidValue)?;
    let (_, value_start, value_end) = css_variable_value_bounds(content, css_variable)?;
    let old_value = content[value_start..value_end].trim().to_string();
    let updated = format!(
        "{} {}{}",
        &content[..value_start],
        new_value.trim(),
        &content[value_end..]
    );
    Ok((updated, old_value))
}

fn css_variable_value_bounds(
    content: &str,
    css_variable: &str,
) -> Result<(usize, usize, usize), CssVariableError> {
    let marker = format!("{css_variable}:");
    let count = content.matches(&marker).count();
    if count == 0 {
        return Err(CssVariableError::Missing);
    }
    if count > 1 {
        return Err(CssVariableError::Ambiguous { match_count: count });
    }
    let start = content.find(&marker).expect("count checked") + marker.len();
    let end = start
        + content[start..]
            .find(';')
            .ok_or(CssVariableError::MissingSemicolon)?;
    Ok((start - marker.len(), start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_current_values_from_contract_mappings_not_token_names() {
        let values = read_contract_token_values(
            &json!({ "tokens": { "color.primary": "--color-primary" } }),
            ":root { --color-primary: #3456aa; }",
        )
        .unwrap();
        assert_eq!(values["color.primary"], "#3456aa");
    }

    #[test]
    fn refuses_ambiguous_css_variable_values() {
        let error = read_css_variable_value(
            ":root { --color-primary: #111; --color-primary: #222; }",
            "--color-primary",
        )
        .unwrap_err();
        assert_eq!(error, CssVariableError::Ambiguous { match_count: 2 });
    }

    #[test]
    fn replaces_exactly_one_contract_variable() {
        let (updated, old_value) = replace_css_variable_value(
            ":root { --color-primary: #111; --color-secondary: #222; }",
            "--color-primary",
            "#333",
        )
        .unwrap();
        assert_eq!(old_value, "#111");
        assert_eq!(
            updated,
            ":root { --color-primary: #333; --color-secondary: #222; }"
        );
    }

    #[test]
    fn identity_ignores_runtime_only_contract_fields() {
        let expected = json!({
            "version": 1,
            "template": "astro-website",
            "appRoot": "project",
            "tokens": { "color.primary": "--color-primary" },
        });
        let actual = json!({
            "version": 1,
            "template": "astro-website",
            "appRoot": "project",
            "tokens": { "color.primary": "--color-primary" },
            "generatedAt": "runtime-only",
        });
        assert_eq!(
            style_contract_identity(&expected),
            style_contract_identity(&actual)
        );
    }
}
