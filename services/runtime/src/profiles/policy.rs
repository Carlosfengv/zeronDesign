use crate::types::{AgentPhase, PermissionMode, RunProfileSnapshot, TranscriptMode};

pub fn snapshot_for_profile(
    phase: AgentPhase,
    agent_profile: &str,
    source_checkpoint_id: Option<String>,
) -> RunProfileSnapshot {
    match (phase, agent_profile) {
        (AgentPhase::Review, _) | (_, "visual-review") | (_, "review") => {
            review_snapshot(source_checkpoint_id)
        }
        (AgentPhase::Repair, _) | (_, "repair") => repair_snapshot(source_checkpoint_id),
        _ => default_snapshot(source_checkpoint_id),
    }
}

pub fn tool_allowed(snapshot: &RunProfileSnapshot, tool_name: &str) -> bool {
    if matches_any(&snapshot.denied_tools, tool_name) {
        return false;
    }
    if snapshot.allowed_tools.is_empty() {
        return true;
    }
    matches_any(&snapshot.allowed_tools, tool_name)
}

pub fn denial_reason(snapshot: &RunProfileSnapshot, tool_name: &str) -> String {
    if matches_any(&snapshot.denied_tools, tool_name) {
        return format!("tool {tool_name} denied by frozen run profile policy");
    }
    format!("tool {tool_name} is outside frozen run profile allowedTools")
}

fn default_snapshot(source_checkpoint_id: Option<String>) -> RunProfileSnapshot {
    RunProfileSnapshot {
        allowed_tools: vec![],
        denied_tools: vec![],
        permission_mode: PermissionMode::Normal,
        transcript_mode: TranscriptMode::Main,
        source_checkpoint_id,
        mcp_server_names: vec!["figma".to_string()],
    }
}

fn review_snapshot(source_checkpoint_id: Option<String>) -> RunProfileSnapshot {
    RunProfileSnapshot {
        allowed_tools: vec![
            "fs.read".to_string(),
            "fs.list".to_string(),
            "fs.search".to_string(),
            "design_source.read_sections".to_string(),
            "preview.status".to_string(),
            "review.report_finding".to_string(),
            "browser.*".to_string(),
            "diagnostics.*".to_string(),
            "project.inspect".to_string(),
            "run.*".to_string(),
        ],
        denied_tools: vec![
            "fs.write".to_string(),
            "fs.patch".to_string(),
            "fs.multi_patch".to_string(),
            "style.update_tokens".to_string(),
            "fs.delete".to_string(),
            "shell.*".to_string(),
            "package.*".to_string(),
            "project.ensure_dependencies".to_string(),
            "preview.start".to_string(),
            "preview.stop".to_string(),
            "preview.report_candidate".to_string(),
            "preview.publish".to_string(),
            "mcp__*".to_string(),
        ],
        permission_mode: PermissionMode::ReadOnly,
        transcript_mode: TranscriptMode::Sidechain,
        source_checkpoint_id,
        mcp_server_names: vec![],
    }
}

fn repair_snapshot(source_checkpoint_id: Option<String>) -> RunProfileSnapshot {
    RunProfileSnapshot {
        allowed_tools: vec![
            "fs.read".to_string(),
            "fs.list".to_string(),
            "fs.search".to_string(),
            "design_source.read_sections".to_string(),
            "fs.write".to_string(),
            "fs.write_chunk".to_string(),
            "fs.commit_chunks".to_string(),
            "fs.patch".to_string(),
            "fs.multi_patch".to_string(),
            "style.update_tokens".to_string(),
            "shell.run".to_string(),
            "package.install".to_string(),
            "project.inspect".to_string(),
            "project.ensure_dependencies".to_string(),
            "preview.*".to_string(),
            "repair.report_attempt".to_string(),
            "browser.*".to_string(),
            "diagnostics.*".to_string(),
            "run.*".to_string(),
        ],
        denied_tools: vec!["preview.report_candidate".to_string(), "mcp__*".to_string()],
        permission_mode: PermissionMode::ScopedRepair,
        transcript_mode: TranscriptMode::Sidechain,
        source_checkpoint_id,
        mcp_server_names: vec![],
    }
}

fn matches_any(patterns: &[String], tool_name: &str) -> bool {
    patterns
        .iter()
        .any(|pattern| matches_pattern(pattern, tool_name))
}

fn matches_pattern(pattern: &str, tool_name: &str) -> bool {
    if pattern == "*" || pattern == tool_name {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return tool_name.starts_with(prefix);
    }
    false
}
