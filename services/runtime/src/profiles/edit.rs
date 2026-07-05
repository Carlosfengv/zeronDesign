use crate::{
    conversation::RuntimeStore,
    types::{AgentRun, Brief},
};
use anyhow::Result;

pub const PROFILE_NAME: &str = "edit";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditIntent {
    Compatible,
    BriefConflict { reason: String },
}

pub async fn classify_edit_intent(
    store: &RuntimeStore,
    run: &AgentRun,
    user_message: &str,
) -> Result<EditIntent> {
    let Some(brief_id) = run.brief_version.as_deref() else {
        return Ok(EditIntent::Compatible);
    };
    let Some(brief) = store.get_brief(brief_id).await else {
        return Ok(EditIntent::Compatible);
    };
    Ok(classify_against_brief(&brief, user_message))
}

pub fn classify_against_brief(brief: &Brief, user_message: &str) -> EditIntent {
    let normalized = user_message.to_lowercase();
    if conflicts_with_project_type(brief, &normalized) {
        return EditIntent::BriefConflict {
            reason: format!(
                "requested project type conflicts with confirmed Brief projectType={}",
                brief.project_type
            ),
        };
    }
    if conflicts_with_template(brief, &normalized) {
        return EditIntent::BriefConflict {
            reason: format!(
                "requested template conflicts with confirmed Brief recommendedTemplate={}",
                brief.recommended_template
            ),
        };
    }
    EditIntent::Compatible
}

fn conflicts_with_project_type(brief: &Brief, normalized_message: &str) -> bool {
    if brief.project_type == "website" {
        return contains_any(
            normalized_message,
            &["docs site", "documentation", "docs portal"],
        );
    }
    if brief.project_type == "docs" {
        return contains_any(
            normalized_message,
            &["landing page", "marketing site", "website"],
        );
    }
    false
}

fn conflicts_with_template(brief: &Brief, normalized_message: &str) -> bool {
    let mentioned_templates = [
        "astro-website",
        "fumadocs-docs",
        "nextjs-website",
        "docusaurus-docs",
    ];
    mentioned_templates.iter().any(|template| {
        *template != brief.recommended_template && normalized_message.contains(template)
    })
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}
