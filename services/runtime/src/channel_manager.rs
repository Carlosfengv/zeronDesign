use crate::{
    conversation::RuntimeStore,
    types::{
        sha256_hex, ChannelLeaseRecord, ChannelLeaseStatus, ChannelLeaseTransport, SandboxBinding,
    },
};
use anyhow::{anyhow, Context, Result};
use chrono::{Duration, Utc};
use std::{
    collections::HashMap,
    net::{SocketAddr, TcpListener},
    process::Stdio,
    sync::{Arc, OnceLock},
    time::Duration as StdDuration,
};
use tokio::{net::TcpStream, process::Child, sync::Mutex, time::sleep};

const CHANNEL_LEASE_TTL_SECONDS: i64 = 300;
const PORT_FORWARD_READY_TIMEOUT: StdDuration = StdDuration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransportMode {
    ServiceDns,
    PortForward,
}

#[derive(Clone)]
pub struct ChannelManager {
    runtime_epoch: Arc<String>,
    children: Arc<Mutex<HashMap<String, Child>>>,
}

impl Default for ChannelManager {
    fn default() -> Self {
        Self {
            runtime_epoch: Arc::new(sha256_hex(&rand::random::<[u8; 32]>())),
            children: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl ChannelManager {
    pub fn shared() -> Arc<Self> {
        static INSTANCE: OnceLock<Arc<ChannelManager>> = OnceLock::new();
        INSTANCE
            .get_or_init(|| Arc::new(ChannelManager::default()))
            .clone()
    }

    pub fn runtime_epoch(&self) -> &str {
        self.runtime_epoch.as_str()
    }

    pub async fn endpoint(
        &self,
        store: &RuntimeStore,
        binding: &SandboxBinding,
        run_id: &str,
        target_port: u16,
        scheme: &str,
        path: &str,
    ) -> Result<String> {
        self.endpoint_with_mode(
            store,
            binding,
            run_id,
            target_port,
            scheme,
            path,
            transport_mode(),
        )
        .await
    }

    // Keep transport selection explicit at the private boundary; production callers use endpoint().
    #[allow(clippy::too_many_arguments)]
    async fn endpoint_with_mode(
        &self,
        store: &RuntimeStore,
        binding: &SandboxBinding,
        run_id: &str,
        target_port: u16,
        scheme: &str,
        path: &str,
        mode: TransportMode,
    ) -> Result<String> {
        let pod_uid = binding
            .pod_uid
            .as_deref()
            .ok_or_else(|| anyhow!("sandbox binding has no verified pod UID"))?;
        self.reconcile_binding(store, &binding.id, pod_uid).await?;

        let leases = store.channel_leases().await?;
        if let Some(mut lease) = leases.into_iter().find(|lease| {
            lease.owner_runtime_epoch == self.runtime_epoch()
                && lease.sandbox_binding_id == binding.id
                && lease.pod_uid == pod_uid
                && lease.target_port == target_port
                && lease.status == ChannelLeaseStatus::Ready
                && lease.expires_at > Utc::now()
        }) {
            lease.heartbeat_at = Utc::now();
            lease.expires_at = Utc::now() + Duration::seconds(CHANNEL_LEASE_TTL_SECONDS);
            let lease = store.put_channel_lease(lease).await?;
            return endpoint_from_lease(&lease, scheme, path);
        }

        let lease_id = sha256_hex(
            format!(
                "{}:{}:{}:{}",
                self.runtime_epoch(),
                binding.id,
                pod_uid,
                target_port
            )
            .as_bytes(),
        );
        let now = Utc::now();
        let mut lease = ChannelLeaseRecord {
            id: lease_id.clone(),
            owner_runtime_epoch: self.runtime_epoch().to_string(),
            sandbox_binding_id: binding.id.clone(),
            sandbox_uid: binding.sandbox_uid.clone(),
            pod_uid: pod_uid.to_string(),
            project_id: binding.project_id.clone(),
            run_id: run_id.to_string(),
            transport: match mode {
                TransportMode::ServiceDns => ChannelLeaseTransport::ServiceDns,
                TransportMode::PortForward => ChannelLeaseTransport::PortForward,
            },
            target_port,
            local_port: None,
            service_endpoint: None,
            child_pid: None,
            child_started_at: None,
            status: ChannelLeaseStatus::Acquiring,
            created_at: now,
            heartbeat_at: now,
            expires_at: now + Duration::seconds(CHANNEL_LEASE_TTL_SECONDS),
        };
        store.put_channel_lease(lease.clone()).await?;

        let service_name = binding
            .channel_service_name
            .as_deref()
            .unwrap_or(&binding.sandbox_name);
        let acquisition = match mode {
            TransportMode::ServiceDns => {
                let (host, port) = debug_authority_override(target_port).unwrap_or_else(|| {
                    (
                        format!("{service_name}.{}.svc.cluster.local", binding.namespace),
                        target_port,
                    )
                });
                lease.service_endpoint = Some(format!("{host}:{port}"));
                Ok(())
            }
            TransportMode::PortForward => {
                let local_port = reserve_local_port()?;
                let mut command = tokio::process::Command::new(
                    std::env::var("KUBECTL").unwrap_or_else(|_| "kubectl".to_string()),
                );
                command
                    .arg("-n")
                    .arg(&binding.namespace)
                    .arg("port-forward")
                    .arg(format!("pod/{}", binding.sandbox_name))
                    .arg(format!("{local_port}:{target_port}"))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true);
                let mut child = command.spawn().context("spawn kubectl port-forward")?;
                let pid = child
                    .id()
                    .ok_or_else(|| anyhow!("port-forward child has no PID"))?;
                let started_at = process_start_fingerprint(pid).await?;
                if let Err(error) = wait_for_local_port(&mut child, local_port).await {
                    let _ = child.kill().await;
                    return Err(error);
                }
                lease.local_port = Some(local_port);
                lease.service_endpoint = Some(format!("127.0.0.1:{local_port}"));
                lease.child_pid = Some(pid);
                lease.child_started_at = Some(started_at);
                self.children.lock().await.insert(lease_id, child);
                Ok(())
            }
        };

        match acquisition {
            Ok(()) => lease.status = ChannelLeaseStatus::Ready,
            Err(error) => {
                lease.status = ChannelLeaseStatus::Failed;
                store.put_channel_lease(lease).await?;
                return Err(error);
            }
        }
        let lease = store.put_channel_lease(lease).await?;
        endpoint_from_lease(&lease, scheme, path)
    }

    pub async fn reconcile(&self, store: &RuntimeStore) -> Result<()> {
        for lease in store.channel_leases().await? {
            if lease.owner_runtime_epoch != self.runtime_epoch()
                && matches!(
                    lease.status,
                    ChannelLeaseStatus::Acquiring | ChannelLeaseStatus::Ready
                )
            {
                self.retire_stale_lease(store, lease).await?;
            }
        }
        Ok(())
    }

    pub async fn release_binding(&self, store: &RuntimeStore, binding_id: &str) -> Result<usize> {
        let leases = store
            .channel_leases()
            .await?
            .into_iter()
            .filter(|lease| {
                lease.sandbox_binding_id == binding_id
                    && matches!(
                        lease.status,
                        ChannelLeaseStatus::Acquiring
                            | ChannelLeaseStatus::Ready
                            | ChannelLeaseStatus::Stale
                    )
            })
            .collect::<Vec<_>>();
        for lease in leases.iter().cloned() {
            self.retire_stale_lease(store, lease).await?;
        }
        Ok(leases.len())
    }

    async fn reconcile_binding(
        &self,
        store: &RuntimeStore,
        binding_id: &str,
        pod_uid: &str,
    ) -> Result<()> {
        for lease in store.channel_leases().await? {
            let stale_epoch = lease.owner_runtime_epoch != self.runtime_epoch();
            let stale_pod = lease.sandbox_binding_id == binding_id && lease.pod_uid != pod_uid;
            let expired = lease.expires_at <= Utc::now();
            if matches!(
                lease.status,
                ChannelLeaseStatus::Acquiring | ChannelLeaseStatus::Ready
            ) && (stale_epoch || stale_pod || expired)
            {
                self.retire_stale_lease(store, lease).await?;
            }
        }
        Ok(())
    }

    async fn retire_stale_lease(
        &self,
        store: &RuntimeStore,
        mut lease: ChannelLeaseRecord,
    ) -> Result<()> {
        lease.status = ChannelLeaseStatus::Stale;
        store.put_channel_lease(lease.clone()).await?;
        if let Some(mut child) = self.children.lock().await.remove(&lease.id) {
            let _ = child.kill().await;
        } else if let (Some(pid), Some(expected)) = (lease.child_pid, &lease.child_started_at) {
            if process_start_fingerprint(pid)
                .await
                .is_ok_and(|actual| actual == *expected)
            {
                let _ = tokio::process::Command::new("kill")
                    .arg("-TERM")
                    .arg(pid.to_string())
                    .status()
                    .await;
            }
        }
        lease.status = ChannelLeaseStatus::Released;
        lease.heartbeat_at = Utc::now();
        store.put_channel_lease(lease).await?;
        Ok(())
    }
}

fn transport_mode() -> TransportMode {
    match std::env::var("SANDBOX_CHANNEL_TRANSPORT").as_deref() {
        Ok("port_forward" | "port-forward" | "desktop") => TransportMode::PortForward,
        Ok("service_dns" | "service-dns" | "cluster") => TransportMode::ServiceDns,
        _ if std::env::var_os("SANDBOX_CHANNEL_HOST_OVERRIDE").is_some()
            || std::env::var_os("SANDBOX_PREVIEW_HOST_OVERRIDE").is_some() =>
        {
            TransportMode::ServiceDns
        }
        _ if std::env::var_os("KUBERNETES_SERVICE_HOST").is_some() => TransportMode::ServiceDns,
        _ => TransportMode::PortForward,
    }
}

fn debug_authority_override(target_port: u16) -> Option<(String, u16)> {
    let (host_variable, port_variable) = if target_port == 4321 {
        (
            "SANDBOX_PREVIEW_HOST_OVERRIDE",
            "SANDBOX_PREVIEW_PORT_OVERRIDE",
        )
    } else {
        (
            "SANDBOX_CHANNEL_HOST_OVERRIDE",
            "SANDBOX_CHANNEL_PORT_OVERRIDE",
        )
    };
    let host = std::env::var(host_variable)
        .ok()
        .filter(|value| !value.trim().is_empty())?;
    let port = std::env::var(port_variable)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(target_port);
    Some((host, port))
}

fn reserve_local_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

async fn wait_for_local_port(child: &mut Child, port: u16) -> Result<()> {
    let deadline = tokio::time::Instant::now() + PORT_FORWARD_READY_TIMEOUT;
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    loop {
        if let Some(status) = child.try_wait()? {
            return Err(anyhow!(
                "kubectl port-forward exited before ready: {status}"
            ));
        }
        if TcpStream::connect(address).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(anyhow!("kubectl port-forward did not become ready"));
        }
        sleep(StdDuration::from_millis(100)).await;
    }
}

async fn process_start_fingerprint(pid: u32) -> Result<String> {
    let output = tokio::process::Command::new("ps")
        .args(["-o", "lstart=", "-p", &pid.to_string()])
        .output()
        .await?;
    if !output.status.success() {
        return Err(anyhow!("cannot inspect child PID {pid}"));
    }
    let fingerprint = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if fingerprint.is_empty() {
        return Err(anyhow!("child PID {pid} has no start fingerprint"));
    }
    Ok(fingerprint)
}

fn endpoint_from_lease(lease: &ChannelLeaseRecord, scheme: &str, path: &str) -> Result<String> {
    let authority = lease
        .service_endpoint
        .as_deref()
        .ok_or_else(|| anyhow!("channel lease has no endpoint"))?;
    Ok(format!("{scheme}://{authority}{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{SandboxBindingStatus, SandboxChannelProtocol};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn manager(epoch: &str) -> ChannelManager {
        ChannelManager {
            runtime_epoch: Arc::new(epoch.to_string()),
            children: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn temp_dir() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let path = std::env::temp_dir().join(format!(
            "channel-manager-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    async fn ready_binding(store: &RuntimeStore, pod_uid: &str) -> SandboxBinding {
        let binding = store
            .create_sandbox_binding(
                "project-1",
                "sandbox-1".to_string(),
                "claim-1".to_string(),
                "workspace-claim-1".to_string(),
                "pool-1".to_string(),
                "sandboxes".to_string(),
                SandboxChannelProtocol::Websocket,
            )
            .await
            .unwrap();
        store
            .update_sandbox_binding_status(&binding.id, SandboxBindingStatus::Ready)
            .await
            .unwrap();
        store
            .update_sandbox_binding_runtime_identity_with_uids(
                &binding.id,
                binding.sandbox_name.clone(),
                Some("sandbox-service".to_string()),
                Some("sandbox-uid-1".to_string()),
                Some(pod_uid.to_string()),
            )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn service_dns_lease_survives_store_restart_and_old_epoch_is_released() {
        let storage = temp_dir();
        let store = RuntimeStore::with_checkpoint_dir(&storage);
        let binding = ready_binding(&store, "pod-uid-1").await;
        let first = manager("epoch-1");
        let endpoint = first
            .endpoint_with_mode(
                &store,
                &binding,
                "run-1",
                3001,
                "ws",
                "/workspace",
                TransportMode::ServiceDns,
            )
            .await
            .unwrap();
        assert_eq!(
            endpoint,
            "ws://sandbox-service.sandboxes.svc.cluster.local:3001/workspace"
        );

        let restarted_store = RuntimeStore::with_checkpoint_dir(&storage);
        let second = manager("epoch-2");
        second.reconcile(&restarted_store).await.unwrap();
        let leases = restarted_store.channel_leases().await.unwrap();
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].status, ChannelLeaseStatus::Released);
    }

    #[tokio::test]
    async fn pod_uid_change_retires_ready_lease_before_reacquiring() {
        let store = RuntimeStore::with_checkpoint_dir(temp_dir());
        let binding = ready_binding(&store, "pod-uid-1").await;
        let manager = manager("epoch-1");
        manager
            .endpoint_with_mode(
                &store,
                &binding,
                "run-1",
                4321,
                "http",
                "",
                TransportMode::ServiceDns,
            )
            .await
            .unwrap();
        let changed = store
            .update_sandbox_binding_runtime_identity_with_uids(
                &binding.id,
                binding.sandbox_name.clone(),
                binding.channel_service_name.clone(),
                binding.sandbox_uid.clone(),
                Some("pod-uid-2".to_string()),
            )
            .await
            .unwrap();
        manager
            .endpoint_with_mode(
                &store,
                &changed,
                "run-1",
                4321,
                "http",
                "",
                TransportMode::ServiceDns,
            )
            .await
            .unwrap();

        let leases = store.channel_leases().await.unwrap();
        assert_eq!(leases.len(), 2);
        assert!(leases.iter().any(|lease| {
            lease.pod_uid == "pod-uid-1" && lease.status == ChannelLeaseStatus::Released
        }));
        assert!(leases.iter().any(|lease| {
            lease.pod_uid == "pod-uid-2" && lease.status == ChannelLeaseStatus::Ready
        }));
    }
}
