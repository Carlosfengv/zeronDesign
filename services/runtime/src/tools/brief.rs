use crate::{
    conversation::RuntimeStore,
    types::{AgentEvent, Brief},
};
use anyhow::{anyhow, Result};
use chrono::Utc;
use serde_json::{json, Map, Value};

pub async fn write_draft(store: &RuntimeStore, run_id: &str, input: Value) -> Result<Value> {
    let input = normalize_draft_input(input);
    let brief: Brief = serde_json::from_value(input)
        .map_err(|err| anyhow!("brief.write_draft received invalid brief JSON: {err}"))?;
    brief
        .validate_for_runtime()
        .map_err(|err| anyhow!("brief.write_draft validation failed: {err}"))?;
    let brief_id = store.write_brief_draft(run_id, brief).await?;
    Ok(json!({ "briefId": brief_id }))
}

pub async fn request_confirmation(
    store: &RuntimeStore,
    run_id: &str,
    project_id: &str,
    message: Option<&str>,
) -> Result<Value> {
    let text = message.unwrap_or("Brief is ready for confirmation.");
    store
        .append_conversation_item(
            project_id,
            Some(run_id),
            "approval_request",
            Some("assistant"),
            text,
            None,
        )
        .await;
    store
        .append_event(AgentEvent::AgentMessage {
            run_id: run_id.to_string(),
            text: text.to_string(),
            timestamp: Utc::now(),
        })
        .await;
    Ok(json!({ "status": "needs_user_input", "message": text }))
}

pub fn normalize_draft_input(input: Value) -> Value {
    let Value::Object(mut object) = input else {
        return input;
    };

    move_alias(
        &mut object,
        "projectType",
        &["project_type", "project", "type"],
    );
    move_alias(&mut object, "contentHierarchy", &["content_hierarchy"]);
    move_alias(&mut object, "pageStructure", &["page_structure", "pages"]);
    move_alias(
        &mut object,
        "visualDirection",
        &["visual_direction", "style"],
    );
    move_alias(
        &mut object,
        "recommendedTemplate",
        &["recommended_template", "template"],
    );
    move_alias(&mut object, "missingInformation", &["missing_information"]);

    if let Some(project_type) = object.get("projectType").and_then(Value::as_str) {
        let normalized = normalize_project_type(project_type);
        object.insert("projectType".to_string(), json!(normalized));
    }

    if let Some(template) = object
        .get("recommendedTemplate")
        .and_then(Value::as_str)
        .map(normalize_template)
    {
        object.insert("recommendedTemplate".to_string(), json!(template));
    } else if let Some(project_type) = object.get("projectType").and_then(Value::as_str) {
        object.insert(
            "recommendedTemplate".to_string(),
            json!(match project_type {
                "docs" => "fumadocs-docs",
                _ => "astro-website",
            }),
        );
    }

    Value::Object(object)
}

fn move_alias(object: &mut Map<String, Value>, canonical: &str, aliases: &[&str]) {
    if object.contains_key(canonical) {
        return;
    }
    for alias in aliases {
        if let Some(value) = object.remove(*alias) {
            object.insert(canonical.to_string(), value);
            return;
        }
    }
}

fn normalize_project_type(project_type: &str) -> &'static str {
    let normalized = project_type.trim().to_ascii_lowercase();
    if normalized.contains("doc") || normalized.contains("文档") {
        "docs"
    } else {
        "website"
    }
}

fn normalize_template(template: &str) -> &'static str {
    match template.trim().to_ascii_lowercase().as_str() {
        "fumadocs" | "fumadocs-docs" | "docs" | "doc" => "fumadocs-docs",
        "docusaurus" | "docusaurus-docs" => "docusaurus-docs",
        "nextjs" | "nextjs-website" => "nextjs-website",
        "astro" | "astro-website" | "website" => "astro-website",
        _ => "astro-website",
    }
}
