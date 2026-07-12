use crate::types::{sha256_hex, DesignProfileUnmappedItem};
use serde_json::{Map, Value};

pub struct ParsedDesignProfileSource {
    pub headings: Vec<String>,
    pub tokens: Map<String, Value>,
    pub extracted_token_count: usize,
    pub extracted_component_count: usize,
    pub unmapped_items: Vec<DesignProfileUnmappedItem>,
    pub warnings: Vec<String>,
}

pub fn parse_design_profile_source(source: &str) -> ParsedDesignProfileSource {
    let mut headings = Vec::new();
    let mut tokens = Map::new();
    let mut extracted_component_count = 0usize;
    let mut unmapped_items = Vec::new();
    let mut offset = 0usize;
    let mut operational_instruction_detected = false;

    for raw_line in source.split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim();
        let start_byte = offset;
        let end_byte = offset + raw_line.len();
        offset = end_byte;
        if trimmed.is_empty() || trimmed.starts_with("```") {
            continue;
        }
        let normalized = trimmed.to_ascii_lowercase();
        if [
            "ignore system",
            "ignore previous",
            "call the tool",
            "call tool",
            "change permission",
            "read /",
            "upload data",
            "exfiltrate",
        ]
        .iter()
        .any(|pattern| normalized.contains(pattern))
        {
            operational_instruction_detected = true;
        }

        if let Some(heading) = trimmed.strip_prefix('#') {
            let heading = heading.trim_start_matches('#').trim();
            if !heading.is_empty() {
                if heading.to_ascii_lowercase().contains("component")
                    || ["button", "input", "card", "badge"]
                        .iter()
                        .any(|name| heading.eq_ignore_ascii_case(name))
                {
                    extracted_component_count += 1;
                }
                headings.push(heading.to_string());
                continue;
            }
        }

        if let Some((name, value)) =
            parse_css_custom_property(trimmed).or_else(|| parse_markdown_token_row(trimmed))
        {
            if let Some(existing) = tokens.get(&name) {
                if existing.as_str() != Some(value.as_str()) {
                    unmapped_items.push(unmapped_source_item(
                        "token-conflict",
                        start_byte,
                        end_byte,
                        line,
                        "duplicate",
                    ));
                }
            } else {
                tokens.insert(name, Value::String(value));
            }
            continue;
        }

        unmapped_items.push(unmapped_source_item(
            headings.last().map(String::as_str).unwrap_or("document"),
            start_byte,
            end_byte,
            line,
            "unsupported-field",
        ));
    }

    let mut warnings = Vec::new();
    if headings.is_empty() {
        warnings.push("No Markdown headings were extracted".to_string());
    }
    if tokens.is_empty() {
        warnings.push("No CSS custom properties or token table rows were extracted".to_string());
    }
    if !unmapped_items.is_empty() {
        warnings.push(format!(
            "{} source items require review",
            unmapped_items.len()
        ));
    }
    if operational_instruction_detected {
        warnings.push(
            "Operational instruction detected and excluded from design semantics".to_string(),
        );
    }
    let extracted_token_count = tokens.len();
    ParsedDesignProfileSource {
        headings,
        tokens,
        extracted_token_count,
        extracted_component_count,
        unmapped_items,
        warnings,
    }
}

fn parse_css_custom_property(line: &str) -> Option<(String, String)> {
    let line = line.trim().trim_end_matches(';');
    let (name, value) = line.split_once(':')?;
    let name = name.trim();
    let value = value.trim();
    if !name.starts_with("--") || name.len() < 3 || value.is_empty() {
        return None;
    }
    Some((name.to_string(), value.to_string()))
}

fn parse_markdown_token_row(line: &str) -> Option<(String, String)> {
    if !line.starts_with('|') || !line.ends_with('|') {
        return None;
    }
    let cells = line
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect::<Vec<_>>();
    if cells.len() < 2
        || cells.iter().all(|cell| {
            cell.chars()
                .all(|character| matches!(character, '-' | ':' | ' '))
        })
    {
        return None;
    }
    let name = cells[0].trim_matches('`');
    let value = cells[1].trim_matches('`');
    let token_like_name = name.starts_with("--")
        || name.contains('.')
        || name.contains('-')
        || name.to_ascii_lowercase().contains("color");
    if !token_like_name || value.is_empty() || value.eq_ignore_ascii_case("value") {
        return None;
    }
    Some((name.to_string(), value.to_string()))
}

fn unmapped_source_item(
    source_section: &str,
    start_byte: usize,
    end_byte: usize,
    line: &str,
    reason: &str,
) -> DesignProfileUnmappedItem {
    let excerpt = line.chars().take(500).collect::<String>();
    DesignProfileUnmappedItem {
        source_section: source_section.to_string(),
        start_byte,
        end_byte,
        excerpt_hash: sha256_hex(excerpt.as_bytes()),
        excerpt,
        reason: reason.to_string(),
    }
}
