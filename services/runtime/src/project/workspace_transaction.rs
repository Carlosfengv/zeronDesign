use crate::{
    tools::{
        runtime::ToolContext,
        sandbox::{WorkspaceBackend, WorkspacePathKind},
    },
    types::sha256_hex,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    error::Error,
    fmt, io,
    path::{Path, PathBuf},
};

const SNAPSHOT_SKIP_DIRS: &[&str] = &["node_modules", "dist", "out", ".next", ".astro", ".source"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TransactionStatus {
    Prepared,
    WorkspaceCommitted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectInitJournal {
    operation_id: String,
    project_id: String,
    run_id: String,
    template_id: String,
    app_root: String,
    app_root_existed: bool,
    style_contract_existed: bool,
    project_state_existed: bool,
    status: TransactionStatus,
    pending_state: Option<Value>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingProjectState {
    project_id: String,
    run_id: String,
    app_root: String,
    template_key: String,
    template_version: String,
    template_manifest_sha256: String,
    framework: String,
    sandbox_execution_profile_id: String,
    sandbox_execution_profile_version: String,
    package_manager: String,
    lockfile: String,
    registry: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectInitRecoveryOutcome {
    NothingToRecover,
    RolledBackPrepared,
    CompletedCommitted,
}

#[derive(Debug)]
pub struct WorkspaceTransactionError {
    pub error_kind: &'static str,
    pub message: String,
}

impl WorkspaceTransactionError {
    fn io(action: &str, error: io::Error) -> Self {
        Self {
            error_kind: "project.init_transaction_failed",
            message: format!("project init transaction {action} failed: {error}"),
        }
    }

    fn recovery_required(message: impl Into<String>) -> Self {
        Self {
            error_kind: "project.init_recovery_required",
            message: message.into(),
        }
    }
}

impl fmt::Display for WorkspaceTransactionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for WorkspaceTransactionError {}

pub struct ProjectInitWorkspaceTransaction<'a> {
    workspace: &'a dyn WorkspaceBackend,
    ctx: &'a ToolContext,
    app_root: PathBuf,
    transaction_root: PathBuf,
    journal_path: PathBuf,
    backup_app_root: PathBuf,
    backup_style_contract: PathBuf,
    backup_project_state: PathBuf,
    journal: ProjectInitJournal,
}

impl<'a> ProjectInitWorkspaceTransaction<'a> {
    pub fn journal_path_for(ctx: &ToolContext) -> PathBuf {
        ctx.workspace_root
            .join("state/project-init-transactions")
            .join(safe_segment(&ctx.project_id))
            .join("journal.json")
    }

    pub async fn begin(
        workspace: &'a dyn WorkspaceBackend,
        ctx: &'a ToolContext,
        app_root: &Path,
        template_id: &str,
    ) -> Result<Self, WorkspaceTransactionError> {
        let operation_id =
            sha256_hex(format!("{}:{}:{}", ctx.project_id, ctx.run.id, template_id).as_bytes());
        let transaction_root = ctx
            .workspace_root
            .join("state/project-init-transactions")
            .join(safe_segment(&ctx.project_id));
        let journal_path = transaction_root.join("journal.json");
        let backup_root = transaction_root.join("backup");
        let backup_app_root = backup_root.join("app");
        let backup_style_contract = backup_root.join("style-contract.json");
        let backup_project_state = backup_root.join("project.json");

        Self::recover_pending(workspace, ctx).await?;

        let app_root_existed = snapshot_directory(
            workspace,
            ctx,
            app_root,
            &backup_app_root,
            SNAPSHOT_SKIP_DIRS,
        )
        .await?;
        let style_contract = ctx.workspace_root.join("state/style-contract.json");
        let project_state = ctx.workspace_root.join("state/project.json");
        let style_contract_existed =
            snapshot_file(workspace, ctx, &style_contract, &backup_style_contract).await?;
        let project_state_existed =
            snapshot_file(workspace, ctx, &project_state, &backup_project_state).await?;
        let now = Utc::now().to_rfc3339();
        let journal = ProjectInitJournal {
            operation_id,
            project_id: ctx.project_id.clone(),
            run_id: ctx.run.id.clone(),
            template_id: template_id.to_string(),
            app_root: app_root
                .strip_prefix(&ctx.workspace_root)
                .unwrap_or(app_root)
                .to_string_lossy()
                .replace('\\', "/"),
            app_root_existed,
            style_contract_existed,
            project_state_existed,
            status: TransactionStatus::Prepared,
            pending_state: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let transaction = Self {
            workspace,
            ctx,
            app_root: app_root.to_path_buf(),
            transaction_root,
            journal_path,
            backup_app_root,
            backup_style_contract,
            backup_project_state,
            journal,
        };
        transaction.write_journal().await?;
        Ok(transaction)
    }

    pub async fn recover_pending(
        workspace: &'a dyn WorkspaceBackend,
        ctx: &'a ToolContext,
    ) -> Result<ProjectInitRecoveryOutcome, WorkspaceTransactionError> {
        let transaction_root = ctx
            .workspace_root
            .join("state/project-init-transactions")
            .join(safe_segment(&ctx.project_id));
        let journal_path = transaction_root.join("journal.json");
        let existing = match workspace.read_to_string(ctx, &journal_path).await {
            Ok(existing) => existing,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(ProjectInitRecoveryOutcome::NothingToRecover)
            }
            Err(error) => return Err(WorkspaceTransactionError::io("read journal", error)),
        };
        let journal: ProjectInitJournal = serde_json::from_str(&existing).map_err(|error| {
            WorkspaceTransactionError::recovery_required(format!(
                "project init journal is unreadable at {}: {error}",
                journal_path.display()
            ))
        })?;
        if journal.project_id != ctx.project_id {
            return Err(WorkspaceTransactionError::recovery_required(format!(
                "project init journal project mismatch: expected {}, found {}",
                ctx.project_id, journal.project_id
            )));
        }
        let backup_root = transaction_root.join("backup");
        let transaction = Self {
            workspace,
            ctx,
            app_root: ctx.workspace_root.join(&journal.app_root),
            transaction_root,
            journal_path,
            backup_app_root: backup_root.join("app"),
            backup_style_contract: backup_root.join("style-contract.json"),
            backup_project_state: backup_root.join("project.json"),
            journal,
        };
        if transaction.journal.status == TransactionStatus::Prepared {
            transaction.rollback().await?;
            return Ok(ProjectInitRecoveryOutcome::RolledBackPrepared);
        }
        transaction.complete_committed_recovery().await?;
        Ok(ProjectInitRecoveryOutcome::CompletedCommitted)
    }

    pub async fn mark_workspace_committed(
        &mut self,
        pending_state: Value,
    ) -> Result<(), WorkspaceTransactionError> {
        self.journal.status = TransactionStatus::WorkspaceCommitted;
        self.journal.pending_state = Some(pending_state);
        self.journal.updated_at = Utc::now().to_rfc3339();
        self.write_journal().await
    }

    pub async fn rollback(self) -> Result<(), WorkspaceTransactionError> {
        remove_if_exists(self.workspace, self.ctx, &self.app_root).await?;
        if self.journal.app_root_existed {
            self.workspace
                .copy_dir_all(self.ctx, &self.backup_app_root, &self.app_root, &[])
                .await
                .map_err(|error| WorkspaceTransactionError::io("restore app root", error))?;
        }
        restore_file(
            self.workspace,
            self.ctx,
            &self.backup_style_contract,
            &self.ctx.workspace_root.join("state/style-contract.json"),
            self.journal.style_contract_existed,
        )
        .await?;
        restore_file(
            self.workspace,
            self.ctx,
            &self.backup_project_state,
            &self.ctx.workspace_root.join("state/project.json"),
            self.journal.project_state_existed,
        )
        .await?;
        remove_if_exists(self.workspace, self.ctx, &self.transaction_root).await
    }

    pub async fn complete(self) -> Result<(), WorkspaceTransactionError> {
        remove_if_exists(self.workspace, self.ctx, &self.transaction_root).await
    }

    pub fn journal_path(&self) -> &Path {
        &self.journal_path
    }

    async fn write_journal(&self) -> Result<(), WorkspaceTransactionError> {
        let text = serde_json::to_string_pretty(&self.journal).map_err(|error| {
            WorkspaceTransactionError {
                error_kind: "project.init_transaction_failed",
                message: format!("project init journal serialization failed: {error}"),
            }
        })?;
        self.workspace
            .write_string(self.ctx, &self.journal_path, &format!("{text}\n"))
            .await
            .map_err(|error| WorkspaceTransactionError::io("write journal", error))
    }

    async fn complete_committed_recovery(self) -> Result<(), WorkspaceTransactionError> {
        let pending: PendingProjectState =
            serde_json::from_value(self.journal.pending_state.clone().ok_or_else(|| {
                WorkspaceTransactionError::recovery_required(
                    "committed project init journal is missing pending state",
                )
            })?)
            .map_err(|error| {
                WorkspaceTransactionError::recovery_required(format!(
                    "committed project init journal has invalid pending state: {error}"
                ))
            })?;
        if pending.project_id != self.ctx.project_id || pending.run_id != self.journal.run_id {
            return Err(WorkspaceTransactionError::recovery_required(
                "committed project init pending state identity does not match its journal",
            ));
        }
        let runtime_state = match self
            .ctx
            .store
            .get_project_runtime_state(&pending.project_id)
            .await
            .filter(|state| pending.matches(state))
        {
            Some(state) => state,
            None => self
                .ctx
                .store
                .upsert_project_runtime_state_with_template_identity(
                    &pending.project_id,
                    pending.app_root.clone(),
                    pending.template_key.clone(),
                    pending.template_version.clone(),
                    Some(pending.template_manifest_sha256.clone()),
                    pending.framework.clone(),
                    Some(pending.sandbox_execution_profile_id.clone()),
                    Some(pending.sandbox_execution_profile_version.clone()),
                    pending.package_manager.clone(),
                    pending.lockfile.clone(),
                    pending.registry.clone(),
                )
                .await
                .map_err(|error| {
                    WorkspaceTransactionError::recovery_required(format!(
                        "RuntimeStore project state recovery failed: {error}"
                    ))
                })?,
        };
        if self.ctx.store.get_run(&pending.run_id).await.is_some() {
            self.ctx
                .store
                .set_run_project_state_snapshot(&pending.run_id, runtime_state.clone())
                .await
                .map_err(|error| {
                    WorkspaceTransactionError::recovery_required(format!(
                        "run project state recovery failed: {error}"
                    ))
                })?;
        }
        let mut state = serde_json::to_value(&runtime_state).map_err(|error| {
            WorkspaceTransactionError::recovery_required(format!(
                "workspace project state recovery serialization failed: {error}"
            ))
        })?;
        state["template"] = Value::String(pending.template_key);
        state["initializedAt"] = Value::String(runtime_state.updated_at.to_rfc3339());
        let text = serde_json::to_string_pretty(&state).map_err(|error| {
            WorkspaceTransactionError::recovery_required(format!(
                "workspace project state recovery serialization failed: {error}"
            ))
        })?;
        self.workspace
            .write_string(
                self.ctx,
                &self.ctx.workspace_root.join("state/project.json"),
                &format!("{text}\n"),
            )
            .await
            .map_err(|error| {
                WorkspaceTransactionError::recovery_required(format!(
                    "workspace project state recovery write failed: {error}"
                ))
            })?;
        self.complete().await
    }
}

impl PendingProjectState {
    fn matches(&self, state: &crate::types::ProjectRuntimeState) -> bool {
        state.project_id == self.project_id
            && state.app_root == self.app_root
            && state.template_key == self.template_key
            && state.template_version == self.template_version
            && state.template_manifest_sha256.as_deref()
                == Some(self.template_manifest_sha256.as_str())
            && state.framework == self.framework
            && state.sandbox_execution_profile_id.as_deref()
                == Some(self.sandbox_execution_profile_id.as_str())
            && state.sandbox_execution_profile_version.as_deref()
                == Some(self.sandbox_execution_profile_version.as_str())
            && state.package_manager == self.package_manager
            && state.lockfile == self.lockfile
            && state.registry == self.registry
    }
}

async fn snapshot_directory(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    from: &Path,
    to: &Path,
    skip: &[&str],
) -> Result<bool, WorkspaceTransactionError> {
    match workspace.path_kind(ctx, from).await {
        Ok(WorkspacePathKind::Dir) => {
            workspace
                .copy_dir_all(
                    ctx,
                    from,
                    to,
                    &skip
                        .iter()
                        .map(|value| (*value).to_string())
                        .collect::<Vec<_>>(),
                )
                .await
                .map_err(|error| WorkspaceTransactionError::io("snapshot app root", error))?;
            Ok(true)
        }
        Ok(WorkspacePathKind::File) => Err(WorkspaceTransactionError {
            error_kind: "project.init_transaction_failed",
            message: format!("project app root is a file: {}", from.display()),
        }),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(WorkspaceTransactionError::io("inspect app root", error)),
    }
}

async fn snapshot_file(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    from: &Path,
    to: &Path,
) -> Result<bool, WorkspaceTransactionError> {
    match workspace.read_bytes(ctx, from).await {
        Ok(bytes) => {
            workspace
                .write_bytes(ctx, to, &bytes)
                .await
                .map_err(|error| WorkspaceTransactionError::io("snapshot state file", error))?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(WorkspaceTransactionError::io("read state file", error)),
    }
}

async fn restore_file(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    backup: &Path,
    target: &Path,
    existed: bool,
) -> Result<(), WorkspaceTransactionError> {
    remove_if_exists(workspace, ctx, target).await?;
    if existed {
        let bytes = workspace
            .read_bytes(ctx, backup)
            .await
            .map_err(|error| WorkspaceTransactionError::io("read state backup", error))?;
        workspace
            .write_bytes(ctx, target, &bytes)
            .await
            .map_err(|error| WorkspaceTransactionError::io("restore state file", error))?;
    }
    Ok(())
}

async fn remove_if_exists(
    workspace: &dyn WorkspaceBackend,
    ctx: &ToolContext,
    path: &Path,
) -> Result<(), WorkspaceTransactionError> {
    match workspace.path_kind(ctx, path).await {
        Ok(WorkspacePathKind::Dir) => workspace
            .remove_dir_all(ctx, path)
            .await
            .map_err(|error| WorkspaceTransactionError::io("remove directory", error)),
        Ok(WorkspacePathKind::File) => workspace
            .remove_file(ctx, path)
            .await
            .map_err(|error| WorkspaceTransactionError::io("remove file", error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(WorkspaceTransactionError::io("inspect path", error)),
    }
}

fn safe_segment(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        conversation::RuntimeStore,
        tools::sandbox::{
            JsonWorkspaceChannelBackend, LocalWorkspaceBackend, WorkspaceChannelRequest,
            WorkspaceChannelTransport,
        },
        types::AgentPhase,
    };
    use async_trait::async_trait;
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde_json::json;
    use std::{fs, sync::Arc};

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
                    let text = request.payload["text"].as_str().ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidData, "missing text")
                    })?;
                    fs::write(path, text)?;
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
                if skip
                    .iter()
                    .any(|name| entry.file_name().to_string_lossy() == *name)
                {
                    continue;
                }
                copy_dir(&source, &target, skip)?;
            } else {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(source, target)?;
            }
        }
        Ok(())
    }

    async fn context(name: &str) -> (ToolContext, PathBuf) {
        let store = RuntimeStore::new();
        let run = store
            .create_run(
                format!("project-{name}"),
                AgentPhase::Build,
                "build".to_string(),
                "fixture".to_string(),
                vec![],
            )
            .await;
        let root = std::env::temp_dir().join(format!(
            "project-init-transaction-{name}-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let workspace_root = root.join("workspace");
        fs::create_dir_all(&workspace_root).unwrap();
        (ToolContext::new(store, run, workspace_root), root)
    }

    #[tokio::test]
    async fn rollback_restores_project_and_state_after_injected_failure() {
        let (ctx, root) = context("rollback").await;
        let workspace = LocalWorkspaceBackend;
        let app_root = ctx.workspace_root.join("project");
        fs::create_dir_all(app_root.join("src")).unwrap();
        fs::create_dir_all(ctx.workspace_root.join("state")).unwrap();
        fs::write(app_root.join("src/original.txt"), "original app").unwrap();
        fs::create_dir_all(app_root.join("node_modules")).unwrap();
        fs::write(app_root.join("node_modules/skipped.txt"), "generated").unwrap();
        fs::write(
            ctx.workspace_root.join("state/style-contract.json"),
            "original style",
        )
        .unwrap();
        fs::write(
            ctx.workspace_root.join("state/project.json"),
            "original state",
        )
        .unwrap();

        let transaction =
            ProjectInitWorkspaceTransaction::begin(&workspace, &ctx, &app_root, "astro-website")
                .await
                .unwrap();

        // Simulate a template writer failing after destructive mutations.
        fs::remove_dir_all(&app_root).unwrap();
        fs::create_dir_all(&app_root).unwrap();
        fs::write(app_root.join("partial.txt"), "partial template").unwrap();
        fs::write(
            ctx.workspace_root.join("state/style-contract.json"),
            "partial style",
        )
        .unwrap();
        fs::remove_file(ctx.workspace_root.join("state/project.json")).unwrap();
        transaction.rollback().await.unwrap();

        assert_eq!(
            fs::read_to_string(app_root.join("src/original.txt")).unwrap(),
            "original app"
        );
        assert!(!app_root.join("partial.txt").exists());
        assert!(!app_root.join("node_modules").exists());
        assert_eq!(
            fs::read_to_string(ctx.workspace_root.join("state/style-contract.json")).unwrap(),
            "original style"
        );
        assert_eq!(
            fs::read_to_string(ctx.workspace_root.join("state/project.json")).unwrap(),
            "original state"
        );
        assert!(!ProjectInitWorkspaceTransaction::journal_path_for(&ctx).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn committed_workspace_is_replayed_into_runtime_store_and_completed() {
        let (ctx, root) = context("committed").await;
        let workspace = LocalWorkspaceBackend;
        let app_root = ctx.workspace_root.join("project");
        let mut transaction =
            ProjectInitWorkspaceTransaction::begin(&workspace, &ctx, &app_root, "astro-website")
                .await
                .unwrap();
        transaction
            .mark_workspace_committed(serde_json::json!({
                "projectId": ctx.project_id,
                "runId": ctx.run.id,
                "appRoot": "project",
                "templateKey": "astro-website",
                "templateVersion": "astro-website@runtime-p3",
                "templateManifestSha256": "7374f4f493c49752bbcbdad49992b02d089f79c1f01784c42fa7224668136e3f",
                "framework": "astro",
                "sandboxExecutionProfileId": "astro-website",
                "sandboxExecutionProfileVersion": "0.1.0",
                "packageManager": "npm",
                "lockfile": "package-lock.json",
                "registry": ctx.npm_registry,
            }))
            .await
            .unwrap();

        let outcome = ProjectInitWorkspaceTransaction::recover_pending(&workspace, &ctx)
            .await
            .unwrap();
        assert_eq!(outcome, ProjectInitRecoveryOutcome::CompletedCommitted);
        let state = ctx
            .store
            .get_project_runtime_state(&ctx.project_id)
            .await
            .unwrap();
        assert_eq!(state.template_key, "astro-website");
        assert_eq!(state.revision, 1);
        assert!(!ProjectInitWorkspaceTransaction::journal_path_for(&ctx).exists());
        assert_eq!(
            ProjectInitWorkspaceTransaction::recover_pending(&workspace, &ctx)
                .await
                .unwrap(),
            ProjectInitRecoveryOutcome::NothingToRecover
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn json_channel_replays_committed_workspace_transaction() {
        let (ctx, root) = context("json-channel-committed").await;
        let transport = LocalChannelTransport {
            root: Arc::new(ctx.workspace_root.clone()),
        };
        let workspace = JsonWorkspaceChannelBackend::new(transport, ctx.workspace_root.clone());
        let app_root = ctx.workspace_root.join("project");
        let mut transaction =
            ProjectInitWorkspaceTransaction::begin(&workspace, &ctx, &app_root, "fumadocs-docs")
                .await
                .unwrap();
        transaction
            .mark_workspace_committed(json!({
                "projectId": ctx.project_id,
                "runId": ctx.run.id,
                "appRoot": "project",
                "templateKey": "fumadocs-docs",
                "templateVersion": "fumadocs-docs@runtime-p3",
                "templateManifestSha256": "2bad43ae3a97dd2a2472779e45206cf8a95f380176cb6214602a2a3868c5a494",
                "framework": "fumadocs",
                "sandboxExecutionProfileId": "fumadocs-docs",
                "sandboxExecutionProfileVersion": "0.1.0",
                "packageManager": "npm",
                "lockfile": "package-lock.json",
                "registry": ctx.npm_registry,
            }))
            .await
            .unwrap();

        assert_eq!(
            ProjectInitWorkspaceTransaction::recover_pending(&workspace, &ctx)
                .await
                .unwrap(),
            ProjectInitRecoveryOutcome::CompletedCommitted
        );
        assert_eq!(
            ctx.store
                .get_project_runtime_state(&ctx.project_id)
                .await
                .unwrap()
                .template_key,
            "fumadocs-docs"
        );
        assert!(!ProjectInitWorkspaceTransaction::journal_path_for(&ctx).exists());

        workspace
            .write_string(&ctx, &app_root.join("original.txt"), "original")
            .await
            .unwrap();
        let _prepared =
            ProjectInitWorkspaceTransaction::begin(&workspace, &ctx, &app_root, "fumadocs-docs")
                .await
                .unwrap();
        workspace.remove_dir_all(&ctx, &app_root).await.unwrap();
        workspace
            .write_string(&ctx, &app_root.join("partial.txt"), "partial")
            .await
            .unwrap();
        assert_eq!(
            ProjectInitWorkspaceTransaction::recover_pending(&workspace, &ctx)
                .await
                .unwrap(),
            ProjectInitRecoveryOutcome::RolledBackPrepared
        );
        assert_eq!(
            workspace
                .read_to_string(&ctx, &app_root.join("original.txt"))
                .await
                .unwrap(),
            "original"
        );
        assert!(workspace
            .path_kind(&ctx, &app_root.join("partial.txt"))
            .await
            .is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
