use super::*;

pub(super) fn validate_style_token_value(value: &str) -> Result<(), String> {
    crate::style_contract::validate_token_value(value)
}

pub(super) fn replace_css_variable_value(
    content: &str,
    css_variable: &str,
    new_value: &str,
    ctx: &ToolContext,
    token_path: &Path,
) -> Result<(String, String), ToolError> {
    crate::style_contract::replace_css_variable_value(content, css_variable, new_value).map_err(
        |error| match error {
            crate::style_contract::CssVariableError::Missing => style_typed_recoverable(
                format!("style.update_tokens could not find CSS variable {css_variable} in token file"),
                "style.token_variable_missing",
                json!({
                    "cssVariable": css_variable,
                    "tokenFile": display_workspace_path(token_path, ctx),
                    "suggestedAction": "Repair the token CSS file or regenerate it from the runtime template before retrying style.update_tokens."
                }),
            ),
            crate::style_contract::CssVariableError::Ambiguous { match_count } => {
                style_typed_recoverable(
                    format!("style.update_tokens found CSS variable {css_variable} multiple times in token file"),
                    "style.token_variable_ambiguous",
                    json!({
                        "cssVariable": css_variable,
                        "tokenFile": display_workspace_path(token_path, ctx),
                        "matchCount": match_count,
                        "suggestedAction": "Keep one canonical CSS variable declaration in the runtime token file before retrying."
                    }),
                )
            }
            crate::style_contract::CssVariableError::MissingSemicolon => style_typed_recoverable(
                format!("style.update_tokens CSS variable {css_variable} is missing a semicolon"),
                "style.token_file_invalid",
                json!({
                    "cssVariable": css_variable,
                    "tokenFile": display_workspace_path(token_path, ctx),
                    "suggestedAction": "Fix the CSS variable declaration so it ends with a semicolon, then retry."
                }),
            ),
            crate::style_contract::CssVariableError::InvalidValue(message) => {
                style_typed_recoverable(
                    format!("style.update_tokens {message}"),
                    "style.token_value_invalid",
                    json!({
                        "cssVariable": css_variable,
                        "tokenFile": display_workspace_path(token_path, ctx),
                        "suggestedAction": "Use a simple CSS token value without semicolons, braces, or newlines."
                    }),
                )
            }
        },
    )
}
