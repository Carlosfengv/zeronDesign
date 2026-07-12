use super::*;

pub(super) fn validate_style_token_value(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err("token values must be non-empty".to_string());
    }
    if trimmed.len() > 256 {
        return Err("token values must be 256 characters or fewer".to_string());
    }
    if trimmed
        .chars()
        .any(|ch| matches!(ch, ';' | '{' | '}' | '\n' | '\r'))
    {
        return Err("token values may not contain ';', braces, or newlines".to_string());
    }
    Ok(())
}

pub(super) fn replace_css_variable_value(
    content: &str,
    css_variable: &str,
    new_value: &str,
    ctx: &ToolContext,
    token_path: &Path,
) -> Result<(String, String), ToolError> {
    let marker = format!("{css_variable}:");
    let count = content.matches(&marker).count();
    if count == 0 {
        return Err(style_typed_recoverable(
            format!("style.update_tokens could not find CSS variable {css_variable} in token file"),
            "style.token_variable_missing",
            json!({
                "cssVariable": css_variable,
                "tokenFile": display_workspace_path(token_path, ctx),
                "suggestedAction": "Repair the token CSS file or regenerate it from the runtime template before retrying style.update_tokens."
            }),
        ));
    }
    if count > 1 {
        return Err(style_typed_recoverable(
            format!("style.update_tokens found CSS variable {css_variable} multiple times in token file"),
            "style.token_variable_ambiguous",
            json!({
                "cssVariable": css_variable,
                "tokenFile": display_workspace_path(token_path, ctx),
                "matchCount": count,
                "suggestedAction": "Keep one canonical CSS variable declaration in the runtime token file before retrying."
            }),
        ));
    }
    let start = content.find(&marker).expect("count checked above");
    let value_start = start + marker.len();
    let semicolon_offset = content[value_start..].find(';').ok_or_else(|| {
        style_typed_recoverable(
            format!("style.update_tokens CSS variable {css_variable} is missing a semicolon"),
            "style.token_file_invalid",
            json!({
                "cssVariable": css_variable,
                "tokenFile": display_workspace_path(token_path, ctx),
                "suggestedAction": "Fix the CSS variable declaration so it ends with a semicolon, then retry."
            }),
        )
    })?;
    let value_end = value_start + semicolon_offset;
    let old_value = content[value_start..value_end].trim().to_string();
    let updated = format!(
        "{} {}{}",
        &content[..value_start],
        new_value.trim(),
        &content[value_end..]
    );
    Ok((updated, old_value))
}
