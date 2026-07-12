use std::{
    collections::HashMap,
    error::Error,
    fmt,
    future::Future,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{sync::watch, task::JoinHandle, time::timeout};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReadiness {
    pub recovered: bool,
    pub shutting_down: bool,
    pub fatal_failure: Option<String>,
    pub active_tasks: Vec<String>,
}

impl RuntimeReadiness {
    pub fn is_ready(&self) -> bool {
        self.recovered && !self.shutting_down && self.fatal_failure.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShutdownEvidence {
    pub requested_tasks: usize,
    pub completed_tasks: usize,
    pub aborted_tasks: Vec<String>,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorError {
    DuplicateTask(String),
    ShuttingDown,
}

impl fmt::Display for SupervisorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateTask(name) => {
                write!(formatter, "runtime task already registered: {name}")
            }
            Self::ShuttingDown => formatter.write_str("runtime supervisor is shutting down"),
        }
    }
}

impl Error for SupervisorError {}

#[derive(Default)]
struct SupervisorState {
    recovered: bool,
    shutting_down: bool,
    fatal_failure: Option<String>,
    tasks: HashMap<String, JoinHandle<()>>,
}

#[derive(Clone)]
pub struct RuntimeSupervisor {
    state: Arc<Mutex<SupervisorState>>,
    shutdown: watch::Sender<bool>,
    fatal_failure: watch::Sender<Option<String>>,
}

impl Default for RuntimeSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeSupervisor {
    pub fn new() -> Self {
        let (shutdown, _) = watch::channel(false);
        let (fatal_failure, _) = watch::channel(None);
        Self {
            state: Arc::new(Mutex::new(SupervisorState::default())),
            shutdown,
            fatal_failure,
        }
    }

    pub fn mark_recovered(&self) {
        self.state.lock().unwrap().recovered = true;
    }

    pub fn readiness(&self) -> RuntimeReadiness {
        let state = self.state.lock().unwrap();
        let mut active_tasks = state.tasks.keys().cloned().collect::<Vec<_>>();
        active_tasks.sort();
        RuntimeReadiness {
            recovered: state.recovered,
            shutting_down: state.shutting_down,
            fatal_failure: state.fatal_failure.clone(),
            active_tasks,
        }
    }

    pub fn spawn<F>(
        &self,
        name: impl Into<String>,
        fatal: bool,
        future: F,
    ) -> Result<(), SupervisorError>
    where
        F: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        self.spawn_owned(name.into(), fatal, future)
    }

    pub fn spawn_with_shutdown<T, F>(
        &self,
        name: impl Into<String>,
        fatal: bool,
        task: T,
    ) -> Result<(), SupervisorError>
    where
        T: FnOnce(watch::Receiver<bool>) -> F,
        F: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let shutdown = self.shutdown.subscribe();
        self.spawn_owned(name.into(), fatal, task(shutdown))
    }

    fn spawn_owned<F>(&self, name: String, fatal: bool, future: F) -> Result<(), SupervisorError>
    where
        F: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        let mut state = self.state.lock().unwrap();
        if state.shutting_down {
            return Err(SupervisorError::ShuttingDown);
        }
        if state.tasks.contains_key(&name) {
            return Err(SupervisorError::DuplicateTask(name));
        }

        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let shared_state = Arc::clone(&self.state);
        let fatal_failure = self.fatal_failure.clone();
        let shutdown_guard = self.shutdown.clone();
        let task_name = name.clone();
        let handle = tokio::spawn(async move {
            // Keep the shutdown channel open for the lifetime of every owned task.
            // Dropping a caller-side supervisor clone is not a shutdown request.
            let _shutdown_guard = shutdown_guard;
            let _ = start_rx.await;
            let result = future.await;
            let mut state = shared_state.lock().unwrap();
            state.tasks.remove(&task_name);
            if fatal {
                if let Err(error) = result {
                    let failure = format!("{task_name}: {error}");
                    state.fatal_failure = Some(failure.clone());
                    let _ = fatal_failure.send(Some(failure));
                }
            }
        });
        state.tasks.insert(name, handle);
        let _ = start_tx.send(());
        Ok(())
    }

    pub async fn wait_for_fatal_failure(&self) -> String {
        let mut failure = self.fatal_failure.subscribe();
        loop {
            if let Some(failure) = failure.borrow().clone() {
                return failure;
            }
            if failure.changed().await.is_err() {
                return "runtime fatal-failure channel closed".to_string();
            }
        }
    }

    pub async fn shutdown(&self, deadline: Duration) -> ShutdownEvidence {
        let started_at = Instant::now();
        let tasks = {
            let mut state = self.state.lock().unwrap();
            state.shutting_down = true;
            state.tasks.drain().collect::<Vec<_>>()
        };
        let requested_tasks = tasks.len();
        let _ = self.shutdown.send(true);

        let mut completed_tasks = 0;
        let mut aborted_tasks = Vec::new();
        for (name, mut handle) in tasks {
            let remaining = deadline.saturating_sub(started_at.elapsed());
            if remaining.is_zero() || timeout(remaining, &mut handle).await.is_err() {
                handle.abort();
                aborted_tasks.push(name);
            } else {
                completed_tasks += 1;
            }
        }
        ShutdownEvidence {
            requested_tasks,
            completed_tasks,
            aborted_tasks,
            elapsed: started_at.elapsed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::oneshot;

    #[tokio::test]
    async fn duplicate_task_owner_is_rejected() {
        let supervisor = RuntimeSupervisor::new();
        let (_tx, rx) = oneshot::channel::<()>();
        supervisor
            .spawn("session/run-1", false, async move {
                let _ = rx.await;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            supervisor.spawn("session/run-1", false, async { Ok(()) }),
            Err(SupervisorError::DuplicateTask("session/run-1".to_string()))
        );
        supervisor.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test]
    async fn owned_task_is_not_cancelled_when_caller_clone_is_dropped() {
        let supervisor = RuntimeSupervisor::new();
        let (completed_tx, completed_rx) = oneshot::channel();
        supervisor
            .spawn("session/run-1", false, async move {
                tokio::task::yield_now().await;
                let _ = completed_tx.send(());
                Ok(())
            })
            .unwrap();

        drop(supervisor);

        tokio::time::timeout(Duration::from_secs(1), completed_rx)
            .await
            .expect("owned task should remain alive")
            .expect("owned task should report completion");
    }

    #[tokio::test]
    async fn fatal_task_failure_revokes_readiness() {
        let supervisor = RuntimeSupervisor::new();
        supervisor.mark_recovered();
        supervisor
            .spawn("capture", true, async { anyhow::bail!("listener failed") })
            .unwrap();
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        let readiness = supervisor.readiness();
        assert!(!readiness.is_ready());
        assert_eq!(
            readiness.fatal_failure.as_deref(),
            Some("capture: listener failed")
        );
    }

    #[tokio::test]
    async fn shutdown_signals_owned_tasks_and_records_evidence() {
        let supervisor = RuntimeSupervisor::new();
        supervisor
            .spawn_with_shutdown("controller", true, |mut shutdown| async move {
                shutdown.changed().await?;
                Ok(())
            })
            .unwrap();
        let evidence = supervisor.shutdown(Duration::from_secs(1)).await;
        assert_eq!(evidence.requested_tasks, 1);
        assert_eq!(evidence.completed_tasks, 1);
        assert!(evidence.aborted_tasks.is_empty());
        assert!(supervisor.readiness().shutting_down);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_aborts_task_that_exceeds_deadline() {
        let supervisor = RuntimeSupervisor::new();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        supervisor
            .spawn("stuck-task", false, async move {
                let _ = started_tx.send(());
                std::thread::sleep(Duration::from_millis(100));
                Ok(())
            })
            .unwrap();
        started_rx.await.unwrap();
        let evidence = supervisor.shutdown(Duration::from_millis(5)).await;
        assert_eq!(evidence.requested_tasks, 1);
        assert_eq!(evidence.completed_tasks, 0);
        assert_eq!(evidence.aborted_tasks, vec!["stuck-task"]);
    }
}
