use crate::tools::runtime::{ProgressSink, ToolContext};
use async_trait::async_trait;
use std::{
    io,
    path::Path,
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::AsyncReadExt,
    process::{Child, Command},
    sync::Mutex,
    time,
};

use super::super::ports::{SandboxCommandBackend, SandboxCommandOutput};

#[derive(Debug, Clone, Default)]
pub struct LocalCommandBackend;

fn local_command_argv(ctx: &ToolContext, argv: &[String]) -> Vec<String> {
    argv.iter().map(|arg| local_command_arg(ctx, arg)).collect()
}

fn local_command_arg(ctx: &ToolContext, arg: &str) -> String {
    if arg == "/workspace" || arg == "workspace" {
        return ctx.workspace_root.to_string_lossy().to_string();
    }
    if let Some(relative) = arg
        .strip_prefix("/workspace/")
        .or_else(|| arg.strip_prefix("workspace/"))
    {
        return ctx
            .workspace_root
            .join(relative)
            .to_string_lossy()
            .to_string();
    }
    arg.to_string()
}

#[async_trait]
impl SandboxCommandBackend for LocalCommandBackend {
    async fn run(
        &self,
        ctx: &ToolContext,
        argv: &[String],
        cwd: &Path,
        timeout_ms: u64,
    ) -> io::Result<SandboxCommandOutput> {
        run_local_command(ctx, argv, cwd, timeout_ms, None).await
    }

    async fn run_with_output_events(
        &self,
        ctx: &ToolContext,
        argv: &[String],
        cwd: &Path,
        timeout_ms: u64,
        progress: Option<ProgressSink>,
        tool_name: &str,
    ) -> io::Result<SandboxCommandOutput> {
        run_local_command(
            ctx,
            argv,
            cwd,
            timeout_ms,
            progress.map(|sink| (sink, tool_name)),
        )
        .await
    }
}

async fn run_local_command(
    ctx: &ToolContext,
    argv: &[String],
    cwd: &Path,
    timeout_ms: u64,
    progress: Option<(ProgressSink, &str)>,
) -> io::Result<SandboxCommandOutput> {
    let argv = local_command_argv(ctx, argv);
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let stderr = Arc::new(Mutex::new(Vec::new()));
    let stdout_events = progress
        .as_ref()
        .map(|(sink, tool_name)| (sink.clone(), (*tool_name).to_string(), "stdout".to_string()));
    let stderr_events =
        progress.map(|(sink, tool_name)| (sink, tool_name.to_string(), "stderr".to_string()));
    let stdout_task = take_output_reader(&mut child, true, stdout.clone(), stdout_events);
    let stderr_task = take_output_reader(&mut child, false, stderr.clone(), stderr_events);
    let started = Instant::now();
    let mut last_len = 0usize;
    let mut last_change = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    let prompt_grace = Duration::from_millis(750);

    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            child.kill().await.ok();
            wait_output_reader(stdout_task).await;
            wait_output_reader(stderr_task).await;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "shell.run timed out",
            ));
        }

        let current_stdout = stdout.lock().await.clone();
        let current_stderr = stderr.lock().await.clone();
        let current_len = current_stdout.len() + current_stderr.len();
        if current_len != last_len {
            last_len = current_len;
            last_change = Instant::now();
        } else if last_change.elapsed() >= prompt_grace
            && output_tail_looks_interactive(&current_stdout, &current_stderr)
        {
            child.kill().await.ok();
            wait_output_reader(stdout_task).await;
            wait_output_reader(stderr_task).await;
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "interactive prompt detected; rerun non-interactively with --yes/--no or use project.init/package.install plus fs.* edits",
            ));
        }
        time::sleep(Duration::from_millis(100)).await;
    };

    wait_output_reader(stdout_task).await;
    wait_output_reader(stderr_task).await;
    let stdout = stdout.lock().await.clone();
    let stderr = stderr.lock().await.clone();
    Ok(SandboxCommandOutput {
        status: status.code(),
        success: status.success(),
        stdout: String::from_utf8_lossy(&stdout).to_string(),
        stderr: String::from_utf8_lossy(&stderr).to_string(),
    })
}

fn take_output_reader(
    child: &mut Child,
    stdout_stream: bool,
    buffer: Arc<Mutex<Vec<u8>>>,
    output_events: Option<(ProgressSink, String, String)>,
) -> tokio::task::JoinHandle<()> {
    let reader =
        if stdout_stream {
            child.stdout.take().map(|stream| {
                Box::pin(stream) as std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>
            })
        } else {
            child.stderr.take().map(|stream| {
                Box::pin(stream) as std::pin::Pin<Box<dyn tokio::io::AsyncRead + Send>>
            })
        };
    tokio::spawn(async move {
        let Some(mut reader) = reader else {
            return;
        };
        let mut chunk = [0u8; 1024];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    buffer.lock().await.extend_from_slice(&chunk[..n]);
                    if let Some((progress, tool_name, stream)) = &output_events {
                        progress
                            .emit_tool_output(
                                tool_name.clone(),
                                stream.clone(),
                                String::from_utf8_lossy(&chunk[..n]).to_string(),
                            )
                            .await;
                    }
                }
            }
        }
    })
}

async fn wait_output_reader(handle: tokio::task::JoinHandle<()>) {
    handle.await.ok();
}

fn output_tail_looks_interactive(stdout: &[u8], stderr: &[u8]) -> bool {
    let mut combined = Vec::new();
    combined.extend_from_slice(stdout);
    combined.extend_from_slice(stderr);
    let tail_start = combined.len().saturating_sub(2048);
    let tail = String::from_utf8_lossy(&combined[tail_start..]).to_lowercase();
    [
        "continue?",
        "proceed?",
        "yes/no",
        "(y/n)",
        "[y/n]",
        "press enter",
        "would you like",
        "do you want",
        "install dependencies?",
        "need to install",
    ]
    .iter()
    .any(|pattern| tail.contains(pattern))
}
