use super::*;
use crate::project::{
    built_in_template_availability, TemplateAvailabilityError, TemplateAvailabilityService,
};

pub(super) struct ProjectInitRequest {
    pub template: String,
    pub path: String,
}

pub(super) struct ProjectInitOutcome {
    pub app_root: String,
    pub template: String,
    pub initial_token_changes: Vec<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProjectInitStep {
    TransactionPrepared,
    ConflictingFilesCleaned,
    TemplateFilesWritten,
    DesignTokensApplied,
    StyleContractWritten,
    WorkspaceCommitted,
    RuntimeStatePublished,
    RunSnapshotPublished,
    WorkspaceHintWritten,
}

#[async_trait]
pub(super) trait ProjectInitFaultInjector: Send + Sync {
    async fn checkpoint(&self, _step: ProjectInitStep) -> Result<(), ProjectInitError> {
        Ok(())
    }
}

struct NoProjectInitFaults;

#[async_trait]
impl ProjectInitFaultInjector for NoProjectInitFaults {}

#[derive(Debug)]
pub(super) enum ProjectInitError {
    Availability(TemplateAvailabilityError),
    Tool(ToolError),
    Transaction(WorkspaceTransactionError),
    RecoveryRequired(String),
    #[cfg(test)]
    SimulatedInterruption(ProjectInitStep),
}

impl ProjectInitError {
    pub(super) fn into_tool_error(self) -> ToolError {
        match self {
            Self::Availability(error) => ToolError::typed_recoverable(
                error.to_string(),
                error.error_kind(),
                json!({
                    "suggestedAction": "Select a template that is registered, enabled, and backed by a ready execution profile."
                }),
            ),
            Self::Tool(error) => error,
            Self::Transaction(error) => ToolError::typed_recoverable(
                error.to_string(),
                error.error_kind,
                json!({
                    "suggestedAction": "Inspect the project.init transaction journal, recover the workspace if required, and retry initialization."
                }),
            ),
            Self::RecoveryRequired(message) => ToolError::typed_recoverable(
                message,
                "project.init_recovery_required",
                json!({
                    "suggestedAction": "Do not continue mutating the project. Recover RuntimeStore and workspace state from the project.init transaction journal, then retry."
                }),
            ),
            #[cfg(test)]
            Self::SimulatedInterruption(step) => ToolError::typed_recoverable(
                format!("project.init interrupted after {step:?}"),
                "project.init_recovery_required",
                json!({
                    "interruptedAfter": format!("{step:?}"),
                    "suggestedAction": "Run startup recovery before allowing another project mutation."
                }),
            ),
        }
    }
}

impl From<ToolError> for ProjectInitError {
    fn from(error: ToolError) -> Self {
        Self::Tool(error)
    }
}

impl From<WorkspaceTransactionError> for ProjectInitError {
    fn from(error: WorkspaceTransactionError) -> Self {
        Self::Transaction(error)
    }
}

pub(super) struct ProjectInitializer {
    workspace: Arc<dyn WorkspaceBackend>,
    availability: Arc<dyn TemplateAvailabilityService>,
    fault_injector: Arc<dyn ProjectInitFaultInjector>,
}

impl ProjectInitializer {
    pub(super) fn built_in(workspace: Arc<dyn WorkspaceBackend>) -> Self {
        Self {
            workspace,
            availability: built_in_template_availability(),
            fault_injector: Arc::new(NoProjectInitFaults),
        }
    }

    #[cfg(test)]
    fn with_fault_injector(
        workspace: Arc<dyn WorkspaceBackend>,
        fault_injector: Arc<dyn ProjectInitFaultInjector>,
    ) -> Self {
        Self {
            workspace,
            availability: built_in_template_availability(),
            fault_injector,
        }
    }

    async fn checkpoint(&self, step: ProjectInitStep) -> Result<(), ProjectInitError> {
        self.fault_injector.checkpoint(step).await
    }

    pub(super) async fn resolve_template(
        &self,
        template: &str,
    ) -> Result<Arc<TemplateSpec>, TemplateAvailabilityError> {
        let id = TemplateId::parse(template)
            .map_err(|_| TemplateAvailabilityError::InvalidId(template.to_string()))?;
        self.availability.resolve_for_init(&id).await
    }

    pub(super) async fn initialize(
        &self,
        ctx: &ToolContext,
        request: ProjectInitRequest,
    ) -> Result<ProjectInitOutcome, ProjectInitError> {
        let template_spec = self
            .resolve_template(&request.template)
            .await
            .map_err(ProjectInitError::Availability)?;
        let app_root_relative = normalize_workspace_relative_path(&request.path)?;
        let app_root =
            check_context_workspace_path(&ctx.workspace_root.join(&app_root_relative), ctx)
                .map_err(|error| {
                    ProjectInitError::Tool(ToolError::PermissionDenied(format!("{error:?}")))
                })?;

        let mut transaction = ProjectInitWorkspaceTransaction::begin(
            &*self.workspace,
            ctx,
            &app_root,
            &request.template,
        )
        .await?;
        self.checkpoint(ProjectInitStep::TransactionPrepared)
            .await?;

        let workspace_result = async {
            cleanup_conflicting_template_files(&*self.workspace, ctx, &app_root, &template_spec)
                .await?;
            self.checkpoint(ProjectInitStep::ConflictingFilesCleaned)
                .await?;
            write_project_template_files(&*self.workspace, ctx, &app_root, &template_spec).await?;
            self.checkpoint(ProjectInitStep::TemplateFilesWritten)
                .await?;
            let style_contract = template_spec
                .style
                .render(&template_spec.id, &app_root_relative);
            let initial_token_changes =
                apply_design_profile_initial_tokens(&*self.workspace, ctx, &style_contract).await?;
            self.checkpoint(ProjectInitStep::DesignTokensApplied)
                .await?;
            write_workspace_json(
                &*self.workspace,
                ctx,
                "state/style-contract.json",
                &style_contract,
            )
            .await?;
            self.checkpoint(ProjectInitStep::StyleContractWritten)
                .await?;
            Ok::<_, ProjectInitError>(initial_token_changes)
        }
        .await;
        let initial_token_changes = match workspace_result {
            Ok(result) => result,
            #[cfg(test)]
            Err(error @ ProjectInitError::SimulatedInterruption(_)) => return Err(error),
            Err(error) => {
                return Err(match transaction.rollback().await {
                    Ok(()) => error,
                    Err(rollback_error) => ProjectInitError::RecoveryRequired(format!(
                        "{error:?}; project.init rollback also failed: {rollback_error}"
                    )),
                });
            }
        };

        let app_root_value = app_root_relative.to_string_lossy().replace('\\', "/");
        let pending_state = json!({
            "projectId": ctx.project_id,
            "runId": ctx.run.id,
            "appRoot": app_root_value,
            "templateKey": request.template,
            "templateVersion": template_spec.version.to_string(),
            "templateManifestSha256": template_spec.manifest_sha256.to_string(),
            "framework": template_spec.framework.to_string(),
            "sandboxExecutionProfileId": template_spec.sandbox_execution_profile.id.to_string(),
            "sandboxExecutionProfileVersion": template_spec.sandbox_execution_profile.version.to_string(),
            "packageManager": "npm",
            "lockfile": "package-lock.json",
            "registry": ctx.npm_registry,
        });
        if let Err(error) = transaction.mark_workspace_committed(pending_state).await {
            return Err(match transaction.rollback().await {
                Ok(()) => ProjectInitError::Transaction(error),
                Err(rollback_error) => ProjectInitError::RecoveryRequired(format!(
                    "project.init could not persist its commit journal: {error}; rollback failed: {rollback_error}"
                )),
            });
        }
        self.checkpoint(ProjectInitStep::WorkspaceCommitted).await?;
        let runtime_state = ctx
            .store
            .upsert_project_runtime_state_with_template_identity(
                &ctx.project_id,
                app_root_value,
                request.template.clone(),
                template_spec.version.to_string(),
                Some(template_spec.manifest_sha256.to_string()),
                template_spec.framework.to_string(),
                Some(template_spec.sandbox_execution_profile.id.to_string()),
                Some(template_spec.sandbox_execution_profile.version.to_string()),
                "npm".to_string(),
                "package-lock.json".to_string(),
                ctx.npm_registry.clone(),
            )
            .await
            .map_err(|error| {
                ProjectInitError::RecoveryRequired(format!(
                    "workspace commit succeeded but RuntimeStore update failed: {error}; recovery journal: {}",
                    transaction.journal_path().display()
                ))
            })?;
        self.checkpoint(ProjectInitStep::RuntimeStatePublished)
            .await?;
        ctx.store
            .set_run_project_state_snapshot(&ctx.run.id, runtime_state.clone())
            .await
            .map_err(|error| {
                ProjectInitError::RecoveryRequired(format!(
                    "project state update succeeded but run snapshot update failed: {error}; recovery journal: {}",
                    transaction.journal_path().display()
                ))
            })?;
        self.checkpoint(ProjectInitStep::RunSnapshotPublished)
            .await?;
        let mut state = serde_json::to_value(&runtime_state).map_err(|error| {
            ProjectInitError::RecoveryRequired(format!(
                "RuntimeStore commit succeeded but workspace state hint serialization failed: {error}; recovery journal: {}",
                transaction.journal_path().display()
            ))
        })?;
        state["template"] = json!(request.template);
        state["initializedAt"] = json!(runtime_state.updated_at.to_rfc3339());
        write_workspace_json(&*self.workspace, ctx, "state/project.json", &state)
            .await
            .map_err(|error| {
                ProjectInitError::RecoveryRequired(format!(
                    "RuntimeStore commit succeeded but workspace state hint update failed: {error:?}; recovery journal: {}",
                    transaction.journal_path().display()
                ))
            })?;
        self.checkpoint(ProjectInitStep::WorkspaceHintWritten)
            .await?;
        transaction.complete().await.map_err(|error| {
            ProjectInitError::RecoveryRequired(format!(
                "project.init state commit succeeded but transaction cleanup failed: {error}"
            ))
        })?;

        Ok(ProjectInitOutcome {
            app_root: display_workspace_path(&app_root, ctx),
            template: request.template,
            initial_token_changes,
        })
    }
}

// remote-fs-boundary: allow-begin project-initializer-test-fixtures
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        conversation::RuntimeStore,
        project::ProjectInitRecoveryOutcome,
        tools::sandbox::{
            JsonWorkspaceChannelBackend, LocalWorkspaceBackend, WebSocketWorkspaceChannelTransport,
            WorkspaceChannelRequest, WorkspaceChannelTransport,
        },
        types::AgentPhase,
    };
    use base64::{engine::general_purpose::STANDARD, Engine};
    use std::{
        fs, io,
        net::TcpListener as StdTcpListener,
        path::{Path, PathBuf},
        process::Stdio,
    };
    use tokio::{net::TcpStream, process::Command, time::Duration};

    struct InterruptAfter(ProjectInitStep);

    #[async_trait]
    impl ProjectInitFaultInjector for InterruptAfter {
        async fn checkpoint(&self, step: ProjectInitStep) -> Result<(), ProjectInitError> {
            if step == self.0 {
                return Err(ProjectInitError::SimulatedInterruption(step));
            }
            Ok(())
        }
    }

    async fn context(step: ProjectInitStep, workspace_root: PathBuf) -> ToolContext {
        let store = RuntimeStore::new();
        let run = store
            .create_run(
                format!("project-init-fault-{step:?}"),
                AgentPhase::Build,
                "build".to_string(),
                "fault-injection".to_string(),
                vec![],
            )
            .await;
        fs::create_dir_all(&workspace_root).unwrap();
        ToolContext::new(store, run, workspace_root)
    }

    async fn assert_checkpoint_matrix(workspace: Arc<dyn WorkspaceBackend>, workspace_root: &Path) {
        let prepared_steps = [
            ProjectInitStep::TransactionPrepared,
            ProjectInitStep::ConflictingFilesCleaned,
            ProjectInitStep::TemplateFilesWritten,
            ProjectInitStep::DesignTokensApplied,
            ProjectInitStep::StyleContractWritten,
        ];
        let committed_steps = [
            ProjectInitStep::WorkspaceCommitted,
            ProjectInitStep::RuntimeStatePublished,
            ProjectInitStep::RunSnapshotPublished,
            ProjectInitStep::WorkspaceHintWritten,
        ];

        for step in prepared_steps.iter().chain(&committed_steps).copied() {
            if workspace_root.exists() {
                fs::remove_dir_all(workspace_root).unwrap();
            }
            fs::create_dir_all(workspace_root).unwrap();
            let ctx = context(step, workspace_root.to_path_buf()).await;
            ProjectInitializer::built_in(workspace.clone())
                .initialize(
                    &ctx,
                    ProjectInitRequest {
                        template: "astro-website".to_string(),
                        path: "project".to_string(),
                    },
                )
                .await
                .unwrap();

            let interrupted = ProjectInitializer::with_fault_injector(
                workspace.clone(),
                Arc::new(InterruptAfter(step)),
            )
            .initialize(
                &ctx,
                ProjectInitRequest {
                    template: "fumadocs-docs".to_string(),
                    path: "project".to_string(),
                },
            )
            .await;
            assert!(
                matches!(
                    interrupted,
                    Err(ProjectInitError::SimulatedInterruption(found)) if found == step
                ),
                "missing interruption at {step:?}"
            );
            assert!(ProjectInitWorkspaceTransaction::journal_path_for(&ctx).exists());

            let recovery = ProjectInitWorkspaceTransaction::recover_pending(&*workspace, &ctx)
                .await
                .unwrap();
            let expected_template = if prepared_steps.contains(&step) {
                assert_eq!(recovery, ProjectInitRecoveryOutcome::RolledBackPrepared);
                "astro-website"
            } else {
                assert_eq!(recovery, ProjectInitRecoveryOutcome::CompletedCommitted);
                "fumadocs-docs"
            };
            assert_eq!(
                ctx.store
                    .get_project_runtime_state(&ctx.project_id)
                    .await
                    .unwrap()
                    .template_key,
                expected_template
            );
            let hint: Value = serde_json::from_str(
                &fs::read_to_string(ctx.workspace_root.join("state/project.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(hint["templateKey"], expected_template);
            assert!(!ProjectInitWorkspaceTransaction::journal_path_for(&ctx).exists());
        }
    }

    fn test_root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "project-initializer-{label}-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ))
    }

    #[tokio::test]
    async fn local_backend_recovers_every_mutation_checkpoint() {
        let root = test_root("local-faults");
        let workspace_root = root.join("workspace");
        assert_checkpoint_matrix(Arc::new(LocalWorkspaceBackend), &workspace_root).await;
        fs::remove_dir_all(root).unwrap();
    }

    #[derive(Clone)]
    struct LocalChannelTransport {
        root: Arc<PathBuf>,
    }

    impl LocalChannelTransport {
        fn path(&self, path: &str) -> PathBuf {
            self.root.join(path.trim_start_matches("/workspace/"))
        }
    }

    #[async_trait]
    impl WorkspaceChannelTransport for LocalChannelTransport {
        async fn request(&self, request: WorkspaceChannelRequest) -> io::Result<Value> {
            let path = self.path(&request.path);
            match request.op {
                "fs.read" => Ok(json!({ "text": fs::read_to_string(path)? })),
                "fs.readBytes" => Ok(json!({ "base64": STANDARD.encode(fs::read(path)?) })),
                "fs.write" => {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(
                        path,
                        request.payload["text"].as_str().ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidData, "missing text")
                        })?,
                    )?;
                    Ok(json!({ "written": true }))
                }
                "fs.writeBytes" => {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    let encoded = request.payload["base64"].as_str().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "missing base64")
                    })?;
                    fs::write(
                        path,
                        STANDARD
                            .decode(encoded)
                            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
                    )?;
                    Ok(json!({ "written": true }))
                }
                "fs.stat" => {
                    let metadata = fs::metadata(path)?;
                    Ok(json!({ "kind": if metadata.is_dir() { "dir" } else { "file" } }))
                }
                "fs.removeFile" => {
                    fs::remove_file(path)?;
                    Ok(json!({ "deleted": true }))
                }
                "fs.removeDirAll" => {
                    fs::remove_dir_all(path)?;
                    Ok(json!({ "deleted": true }))
                }
                "fs.copyDir" => {
                    let target = self.path(request.payload["to"].as_str().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "missing copy target")
                    })?);
                    let skip = request.payload["skipDirNames"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>();
                    copy_dir(&path, &target, &skip)?;
                    Ok(json!({ "copied": true }))
                }
                other => Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("unsupported channel operation: {other}"),
                )),
            }
        }
    }

    fn copy_dir(from: &Path, to: &Path, skip: &[&str]) -> io::Result<()> {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let source = entry.path();
            let target = to.join(entry.file_name());
            if source.is_dir() {
                if !skip
                    .iter()
                    .any(|name| entry.file_name().to_string_lossy() == *name)
                {
                    copy_dir(&source, &target, skip)?;
                }
            } else {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(source, target)?;
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn json_channel_recovers_every_mutation_checkpoint() {
        let root = test_root("json-faults");
        let workspace_root = root.join("workspace");
        fs::create_dir_all(&workspace_root).unwrap();
        let backend = JsonWorkspaceChannelBackend::new(
            LocalChannelTransport {
                root: Arc::new(workspace_root.clone()),
            },
            workspace_root.clone(),
        );
        assert_checkpoint_matrix(Arc::new(backend), &workspace_root).await;
        fs::remove_dir_all(root).unwrap();
    }

    fn free_tcp_port() -> u16 {
        StdTcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    async fn wait_for_tcp_port(port: u16) {
        for _ in 0..100 {
            if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("workspace channel did not listen on port {port}");
    }

    #[tokio::test]
    async fn websocket_channel_recovers_every_mutation_checkpoint() {
        let root = test_root("websocket-faults");
        let workspace_root = root.join("workspace");
        fs::create_dir_all(&workspace_root).unwrap();
        let port = free_tcp_port();
        let script = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../infra/agent-sandbox/base/workspace-channel-server.js");
        let mut child = Command::new("node")
            .arg(script)
            .env("WORKSPACE_ROOT", &workspace_root)
            .env("PORT", port.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("node must start workspace-channel-server.js");
        wait_for_tcp_port(port).await;
        let backend = JsonWorkspaceChannelBackend::new(
            WebSocketWorkspaceChannelTransport::new(format!("ws://127.0.0.1:{port}/workspace"))
                .with_timeout(Duration::from_secs(2)),
            workspace_root.clone(),
        );

        assert_checkpoint_matrix(Arc::new(backend), &workspace_root).await;
        child.kill().await.ok();
        fs::remove_dir_all(root).unwrap();
    }
}
// remote-fs-boundary: allow-end project-initializer-test-fixtures
