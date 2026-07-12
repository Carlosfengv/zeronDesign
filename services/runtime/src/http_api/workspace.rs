use super::*;

pub(in crate::http_api) fn project_workspace_root(
    config: &RuntimeConfig,
    project_id: &str,
) -> PathBuf {
    let safe = project_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    config.workspace_root.join(safe)
}

pub(in crate::http_api) fn effective_workspace_root(
    config: &RuntimeConfig,
    project_id: &str,
) -> PathBuf {
    match config.sandbox_backend_mode {
        SandboxBackendMode::PhaseAContract => project_workspace_root(config, project_id),
        SandboxBackendMode::Kubernetes => config.workspace_root.clone(),
    }
}

pub(in crate::http_api) fn project_state_roots(
    config: &RuntimeConfig,
    project_id: &str,
) -> Vec<PathBuf> {
    vec![
        project_workspace_root(config, project_id),
        config.workspace_root.clone(),
    ]
}

pub(in crate::http_api) fn read_first_json_file(
    roots: &[PathBuf],
    relative: &str,
) -> Option<Value> {
    roots
        .iter()
        .find_map(|root| read_json_file(&root.join(relative)))
}

// remote-fs-boundary: allow-begin phase-a-runtime-state-fallback
pub(in crate::http_api) fn read_json_file(path: &FsPath) -> Option<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}
// remote-fs-boundary: allow-end phase-a-runtime-state-fallback

pub(in crate::http_api) async fn read_runtime_state_json(
    state: &AppState,
    project_id: &str,
    run: Option<&AgentRun>,
    sandbox_binding_id: Option<&str>,
    relative: &str,
) -> Option<Value> {
    if state.config.sandbox_backend_mode == SandboxBackendMode::Kubernetes {
        if let (Some(run), Some(sandbox_binding_id)) = (run, sandbox_binding_id) {
            let mut run = run.clone();
            run.sandbox_id = Some(sandbox_binding_id.to_string());
            let ctx = ToolContext::new(
                state.store.clone(),
                run,
                state.config.workspace_root.clone(),
            );
            let backend = SandboxChannelWorkspaceBackend::new();
            if let Ok(text) = backend
                .read_to_string(&ctx, &state.config.workspace_root.join(relative))
                .await
            {
                if let Ok(value) = serde_json::from_str(&text) {
                    return Some(value);
                }
            }
        }
    }

    let state_roots = project_state_roots(&state.config, project_id);
    read_first_json_file(&state_roots, relative)
}
