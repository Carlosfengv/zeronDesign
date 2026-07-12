use super::{
    DesiredUnpublishRuntime, DesiredWorkRuntime, KubernetesResourceIdentity, ObservedWorkRuntime,
    PublicationReconcileDisposition, PublishCheckpoint, WorkRuntimeBackend, FIELD_MANAGER,
};
use crate::{config::WorkRuntimeExposureMode, RuntimeConfig};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::{Namespace, Pod, Service},
    networking::v1::NetworkPolicy,
};
use kube::{
    api::{DeleteParams, Patch, PatchParams},
    Api, Client, Resource, ResourceExt,
};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration;

#[path = "kubernetes_ingress.rs"]
mod ingress;
pub use ingress::KubernetesIngressExposure;
#[path = "kubernetes_switch.rs"]
mod switch;

const APPLY_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Clone)]
pub struct KubernetesWorkRuntimeBackend {
    client: Client,
    timeout: Duration,
    prober_image: String,
    exposure: Option<KubernetesIngressExposure>,
}

impl KubernetesWorkRuntimeBackend {
    pub async fn try_default() -> Result<Self> {
        Self::from_runtime_config(&RuntimeConfig::from_env()).await
    }

    pub async fn from_runtime_config(config: &RuntimeConfig) -> Result<Self> {
        let prober_image = config
            .work_runtime_prober_image
            .clone()
            .context("WORK_RUNTIME_PROBER_IMAGE is required for Kubernetes publication")?;
        if !is_digest_pinned_image(&prober_image) {
            bail!("WORK_RUNTIME_PROBER_IMAGE must be sha256-pinned");
        }
        let exposure = if config.work_runtime_exposure_mode == WorkRuntimeExposureMode::Ingress {
            Some(KubernetesIngressExposure::from_runtime_config(config)?)
        } else {
            None
        };
        Ok(Self {
            client: Client::try_default()
                .await
                .context("load Kubernetes client configuration")?,
            timeout: APPLY_TIMEOUT,
            prober_image,
            exposure,
        })
    }

    pub fn new(client: Client, timeout: Duration, prober_image: String) -> Result<Self> {
        if !is_digest_pinned_image(&prober_image) {
            bail!("Release Prober image must be sha256-pinned");
        }
        Ok(Self {
            client,
            timeout,
            prober_image,
            exposure: None,
        })
    }

    pub fn new_with_ingress(
        client: Client,
        timeout: Duration,
        prober_image: String,
        exposure: KubernetesIngressExposure,
    ) -> Result<Self> {
        let mut backend = Self::new(client, timeout, prober_image)?;
        exposure.validate()?;
        backend.exposure = Some(exposure);
        Ok(backend)
    }

    async fn restore_previous_release(
        &self,
        desired: &DesiredWorkRuntime,
        current_release_id: &str,
    ) -> Result<()> {
        self.switch_stable_service(desired, &desired.release_id, current_release_id)
            .await?;
        self.wait_endpoint_slice_release(desired, current_release_id)
            .await?;
        if let Some(exposure) = &self.exposure {
            self.verify_external_release_id(desired, exposure, current_release_id)
                .await?;
        }
        Ok(())
    }

    async fn assert_namespace(&self, desired: &DesiredWorkRuntime) -> Result<()> {
        let namespaces: Api<Namespace> = Api::all(self.client.clone());
        let namespace = namespaces
            .get(&desired.namespace)
            .await
            .context("get Published Runtime namespace")?;
        let labels = namespace.metadata.labels.unwrap_or_default();
        if labels.get("anydesign.dev/purpose").map(String::as_str) != Some("published-works") {
            bail!("Published Runtime namespace is missing its managed purpose label");
        }
        Ok(())
    }

    async fn apply_network_policy(&self, desired: &DesiredWorkRuntime) -> Result<()> {
        let policies: Api<NetworkPolicy> = Api::namespaced(self.client.clone(), &desired.namespace);
        assert_expected_identity(
            &policies,
            &desired.network_policy_name,
            None,
            &desired.owner_record_id,
        )
        .await?;
        let policy = object::<NetworkPolicy>(json!({
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": {
                "name": desired.network_policy_name,
                "namespace": desired.namespace,
                "labels": desired.labels,
            },
            "spec": {
                "podSelector": {"matchLabels": {"anydesign.dev/work": desired.work_name}},
                "policyTypes": ["Ingress", "Egress"],
                "ingress": [
                    {
                        "from": [{"podSelector": {"matchLabels": {"anydesign.dev/role": "release-prober"}}}],
                        "ports": [{"protocol": "TCP", "port": desired.container_port}]
                    },
                    {
                        "from": [{"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "anydesign-ingress"}}}],
                        "ports": [{"protocol": "TCP", "port": desired.container_port}]
                    },
                    {
                        "from": [{
                            "namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "kube-system"}},
                            "podSelector": {"matchLabels": {"app.kubernetes.io/name": "traefik"}}
                        }],
                        "ports": [{"protocol": "TCP", "port": desired.container_port}]
                    }
                ],
                "egress": []
            }
        }))?;
        policies
            .patch(
                &desired.network_policy_name,
                &apply_params(),
                &Patch::Apply(&policy),
            )
            .await
            .context("server-side apply work NetworkPolicy")?;
        Ok(())
    }

    async fn apply_deployment(&self, desired: &DesiredWorkRuntime) -> Result<Deployment> {
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &desired.namespace);
        assert_expected_identity(
            &deployments,
            &desired.deployment_name,
            desired.expected_deployment_uid.as_deref(),
            &desired.owner_record_id,
        )
        .await?;
        let deployment = object::<Deployment>(json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": {
                "name": desired.deployment_name,
                "namespace": desired.namespace,
                "labels": desired.labels,
                "annotations": trust_annotations(desired),
            },
            "spec": {
                "replicas": 1,
                "selector": {"matchLabels": {
                    "anydesign.dev/work": desired.work_name,
                    "anydesign.dev/release-id": desired.release_id,
                }},
                "strategy": {"type": "Recreate"},
                "template": {
                    "metadata": {"labels": desired.labels, "annotations": trust_annotations(desired)},
                    "spec": {
                        "automountServiceAccountToken": false,
                        "enableServiceLinks": false,
                        "securityContext": {"runAsNonRoot": true, "seccompProfile": {"type": "RuntimeDefault"}},
                        "volumes": [{"name": "tmp", "emptyDir": {"sizeLimit": "16Mi"}}],
                        "containers": [{
                            "name": "work",
                            "image": desired.image,
                            "imagePullPolicy": "IfNotPresent",
                            "ports": [{"name": "http", "containerPort": desired.container_port, "protocol": "TCP"}],
                            "readinessProbe": {"httpGet": {"path": desired.health_path, "port": "http"}, "periodSeconds": 2, "failureThreshold": 15},
                            "livenessProbe": {"httpGet": {"path": desired.health_path, "port": "http"}, "periodSeconds": 10, "failureThreshold": 3},
                            "resources": {
                                "requests": {"cpu": "10m", "memory": "16Mi"},
                                "limits": {"cpu": "250m", "memory": "128Mi"}
                            },
                            "volumeMounts": [{"name": "tmp", "mountPath": "/tmp"}],
                            "securityContext": {
                                "allowPrivilegeEscalation": false,
                                "readOnlyRootFilesystem": true,
                                "runAsNonRoot": true,
                                "capabilities": {"drop": ["ALL"]}
                            }
                        }]
                    }
                }
            }
        }))?;
        deployments
            .patch(
                &desired.deployment_name,
                &apply_params(),
                &Patch::Apply(&deployment),
            )
            .await
            .context("server-side apply release-specific Deployment")
    }

    async fn wait_ready(&self, desired: &DesiredWorkRuntime) -> Result<Deployment> {
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &desired.namespace);
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            let deployment = deployments.get(&desired.deployment_name).await?;
            let ready = deployment
                .status
                .as_ref()
                .is_some_and(|status| status.available_replicas.unwrap_or_default() >= 1);
            if ready {
                return Ok(deployment);
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("release-specific Deployment did not become Available before timeout");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn apply_service(
        &self,
        desired: &DesiredWorkRuntime,
        name: &str,
        expected_uid: Option<&str>,
        expected_resource_version: Option<&str>,
    ) -> Result<Service> {
        let services: Api<Service> = Api::namespaced(self.client.clone(), &desired.namespace);
        assert_expected_identity(&services, name, expected_uid, &desired.owner_record_id).await?;
        let mut metadata = json!({
            "name": name,
            "namespace": desired.namespace,
            "labels": desired.labels,
        });
        if let Some(resource_version) = expected_resource_version {
            metadata["resourceVersion"] = Value::String(resource_version.to_string());
        }
        let service = object::<Service>(json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": metadata,
            "spec": {
                "type": "ClusterIP",
                "selector": {
                    "anydesign.dev/work": desired.work_name,
                    "anydesign.dev/release-id": desired.release_id,
                },
                "ports": [{"name": "http", "port": 80, "targetPort": "http", "protocol": "TCP"}]
            }
        }))?;
        services
            .patch(name, &apply_params(), &Patch::Apply(&service))
            .await
            .with_context(|| format!("server-side apply Service {name}"))
    }

    async fn probe_release(&self, desired: &DesiredWorkRuntime) -> Result<()> {
        self.apply_service(desired, &desired.probe_service_name, None, None)
            .await?;
        let result = self.run_release_prober(desired).await;
        let services: Api<Service> = Api::namespaced(self.client.clone(), &desired.namespace);
        let delete_result = services
            .delete(&desired.probe_service_name, &DeleteParams::default())
            .await;
        if let Err(error) = delete_result {
            if !matches!(error, kube::Error::Api(ref response) if response.code == 404) {
                return Err(error).context("delete temporary release probe Service");
            }
        }
        result
    }

    async fn run_release_prober(&self, desired: &DesiredWorkRuntime) -> Result<()> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &desired.namespace);
        let name = format!("{}-verify", desired.probe_service_name);
        if pods.get_opt(&name).await?.is_some() {
            pods.delete(&name, &DeleteParams::default()).await?;
            wait_absent(&pods, &name, self.timeout).await?;
        }
        let script = format!(
            "attempt=0; until wget -q -T 2 -O /dev/null http://{}{} && wget -q -T 2 -O - http://{}{} | grep -F -- '{}' >/dev/null; do attempt=$((attempt+1)); [ \"$attempt\" -lt 20 ] || exit 1; sleep 1; done",
            desired.probe_service_name,
            desired.health_path,
            desired.probe_service_name,
            desired.release_path,
            desired.release_id,
        );
        let pod = object::<Pod>(json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": name,
                "namespace": desired.namespace,
                "labels": {
                    "anydesign.dev/role": "release-prober",
                    "anydesign.dev/work": desired.work_name,
                    "app.kubernetes.io/managed-by": FIELD_MANAGER
                }
            },
            "spec": {
                "restartPolicy": "Never",
                "serviceAccountName": "anydesign-release-prober",
                "automountServiceAccountToken": false,
                "enableServiceLinks": false,
                "securityContext": {"runAsNonRoot": true, "seccompProfile": {"type": "RuntimeDefault"}},
                "containers": [{
                    "name": "probe",
                    "image": self.prober_image,
                    "imagePullPolicy": "IfNotPresent",
                    "command": ["sh", "-ec", script],
                    "resources": {
                        "requests": {"cpu": "5m", "memory": "8Mi"},
                        "limits": {"cpu": "50m", "memory": "32Mi"}
                    },
                    "securityContext": {
                        "allowPrivilegeEscalation": false,
                        "readOnlyRootFilesystem": true,
                        "runAsNonRoot": true,
                        "runAsUser": 101,
                        "capabilities": {"drop": ["ALL"]}
                    }
                }]
            }
        }))?;
        pods.patch(&name, &apply_params(), &Patch::Apply(&pod))
            .await
            .context("create isolated Release Prober Pod")?;
        let deadline = tokio::time::Instant::now() + self.timeout;
        let result = loop {
            let pod = pods.get(&name).await?;
            match pod
                .status
                .as_ref()
                .and_then(|status| status.phase.as_deref())
            {
                Some("Succeeded") => break Ok(()),
                Some("Failed") => {
                    let logs = pods
                        .logs(&name, &Default::default())
                        .await
                        .unwrap_or_default();
                    break Err(anyhow::anyhow!("Release Prober failed: {logs}"));
                }
                _ if tokio::time::Instant::now() >= deadline => {
                    break Err(anyhow::anyhow!(
                        "Release Prober did not complete before timeout"
                    ));
                }
                _ => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        };
        let delete_result = pods.delete(&name, &DeleteParams::default()).await;
        if let Err(error) = delete_result {
            if !matches!(error, kube::Error::Api(ref response) if response.code == 404) {
                return Err(error).context("delete Release Prober Pod");
            }
        }
        result
    }
}

#[async_trait]
impl WorkRuntimeBackend for KubernetesWorkRuntimeBackend {
    async fn reconcile(
        &self,
        desired: &DesiredWorkRuntime,
    ) -> Result<PublicationReconcileDisposition> {
        self.assert_namespace(desired).await?;
        self.apply_network_policy(desired).await?;
        self.apply_deployment(desired).await?;
        let deployment = self.wait_ready(desired).await?;
        self.probe_release(desired).await?;
        let switching_from = desired
            .current_release_id
            .as_deref()
            .filter(|current| *current != desired.release_id);
        let (service, ingress) = if let Some(current_release_id) = switching_from {
            let ingress = if let Some(exposure) = &self.exposure {
                Some(self.apply_ingress(desired, exposure).await?)
            } else {
                None
            };
            let service = self
                .switch_stable_service(desired, current_release_id, &desired.release_id)
                .await?;
            if let Err(switch_error) = self
                .wait_endpoint_slice_release(desired, &desired.release_id)
                .await
            {
                return match self
                    .restore_previous_release(desired, current_release_id)
                    .await
                {
                    Ok(()) => Err(switch_error)
                        .context("green switch failed and stable Service was restored to blue"),
                    Err(rollback_error) => Err(anyhow::anyhow!(
                        "green switch failed: {switch_error}; selector rollback also failed: {rollback_error}"
                    )),
                };
            }
            let ingress = ingress.map(|ingress| identity(&ingress)).transpose()?;
            if desired.reconcile_checkpoint != PublishCheckpoint::TrafficSwitched {
                return Ok(PublicationReconcileDisposition::TrafficSwitched(Box::new(
                    ObservedWorkRuntime {
                        deployment: identity(&deployment)?,
                        service: identity(&service)?,
                        ingress,
                        ready: true,
                        release_identity_verified: true,
                        external_release_identity_verified: false,
                    },
                )));
            }
            if let Some(exposure) = &self.exposure {
                if let Err(switch_error) = self.verify_external_release(desired, exposure).await {
                    return match self
                        .restore_previous_release(desired, current_release_id)
                        .await
                    {
                        Ok(()) => Err(switch_error)
                            .context("green external probe failed and stable Service was restored to blue"),
                        Err(rollback_error) => Err(anyhow::anyhow!(
                            "green external probe failed: {switch_error}; selector rollback also failed: {rollback_error}"
                        )),
                    };
                }
            }
            (service, ingress)
        } else {
            let service = self
                .apply_service(
                    desired,
                    &desired.stable_service_name,
                    desired.expected_service_uid.as_deref(),
                    desired.expected_service_resource_version.as_deref(),
                )
                .await?;
            let ingress = if let Some(exposure) = &self.exposure {
                let ingress = self.apply_ingress(desired, exposure).await?;
                self.verify_external_release(desired, exposure).await?;
                Some(identity(&ingress)?)
            } else {
                None
            };
            (service, ingress)
        };
        Ok(PublicationReconcileDisposition::Applied(Box::new(
            ObservedWorkRuntime {
                deployment: identity(&deployment)?,
                service: identity(&service)?,
                ingress,
                ready: true,
                release_identity_verified: true,
                external_release_identity_verified: self.exposure.is_some(),
            },
        )))
    }

    async fn unpublish(
        &self,
        desired: &DesiredUnpublishRuntime,
    ) -> Result<PublicationReconcileDisposition> {
        self.delete_published_resources(desired).await?;
        Ok(PublicationReconcileDisposition::Unpublished)
    }
}

fn apply_params() -> PatchParams {
    PatchParams::apply(FIELD_MANAGER).force()
}

fn object<K: DeserializeOwned>(value: Value) -> Result<K> {
    serde_json::from_value(value).context("construct controlled Kubernetes resource")
}

fn trust_annotations(desired: &DesiredWorkRuntime) -> Value {
    json!({
        "anydesign.dev/image-digest": desired.image_digest,
        "anydesign.dev/signature-identity": desired.trust.signature_identity,
        "anydesign.dev/signature-digest": desired.trust.signature_digest,
        "anydesign.dev/provenance-digest": desired.trust.provenance_digest,
        "anydesign.dev/scan-policy": desired.trust.scan_policy_version,
        "anydesign.dev/scan-report-digest": desired.trust.scan_report_digest,
    })
}

async fn assert_expected_identity<K>(
    api: &Api<K>,
    name: &str,
    expected_uid: Option<&str>,
    expected_owner_record_id: &str,
) -> Result<()>
where
    K: Clone + DeserializeOwned + std::fmt::Debug + Resource<DynamicType = ()>,
{
    let current = api
        .get_opt(name)
        .await
        .with_context(|| format!("read Kubernetes resource identity {name} before mutation"))?;
    match (expected_uid, current) {
        (Some(expected), Some(current)) if current.meta().uid.as_deref() == Some(expected) => {
            Ok(())
        }
        (Some(_), _) => bail!("Kubernetes resource UID drift detected for {name}"),
        (None, None) => Ok(()),
        (None, Some(current)) => {
            let labels = current.meta().labels.as_ref();
            if labels
                .and_then(|labels| labels.get("app.kubernetes.io/managed-by"))
                .map(String::as_str)
                != Some(FIELD_MANAGER)
                || labels
                    .and_then(|labels| labels.get("anydesign.dev/owner-record-id"))
                    .map(String::as_str)
                    != Some(expected_owner_record_id)
            {
                bail!("refusing to adopt pre-existing Kubernetes resource {name}");
            }
            Ok(())
        }
    }
}

async fn assert_delete_identity<K>(
    api: &Api<K>,
    name: &str,
    expected_uid: Option<&str>,
    expected_owner_record_id: &str,
) -> Result<()>
where
    K: Clone + DeserializeOwned + std::fmt::Debug + Resource<DynamicType = ()>,
{
    let Some(current) = api.get_opt(name).await? else {
        return Ok(());
    };
    if let Some(expected) = expected_uid {
        if current.meta().uid.as_deref() != Some(expected) {
            bail!("Kubernetes resource UID drift detected for {name}");
        }
        return Ok(());
    }
    let labels = current.meta().labels.as_ref();
    if labels
        .and_then(|labels| labels.get("app.kubernetes.io/managed-by"))
        .map(String::as_str)
        != Some(FIELD_MANAGER)
        || labels
            .and_then(|labels| labels.get("anydesign.dev/owner-record-id"))
            .map(String::as_str)
            != Some(expected_owner_record_id)
    {
        bail!("refusing to delete pre-existing Kubernetes resource {name}");
    }
    Ok(())
}

fn identity<K: ResourceExt>(resource: &K) -> Result<KubernetesResourceIdentity> {
    Ok(KubernetesResourceIdentity {
        name: resource.name_any(),
        uid: resource
            .meta()
            .uid
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Kubernetes resource is missing UID"))?,
        resource_version: resource
            .meta()
            .resource_version
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Kubernetes resource is missing resourceVersion"))?,
    })
}

fn is_digest_pinned_image(image: &str) -> bool {
    image
        .rsplit_once("@sha256:")
        .is_some_and(|(repository, digest)| {
            !repository.is_empty()
                && digest.len() == 64
                && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
}

async fn wait_absent<K>(api: &Api<K>, name: &str, timeout: Duration) -> Result<()>
where
    K: Clone + DeserializeOwned + std::fmt::Debug + Resource<DynamicType = ()>,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if api.get_opt(name).await?.is_none() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("Kubernetes resource {name} was not deleted before timeout");
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn delete_if_present<K>(api: &Api<K>, name: &str) -> Result<()>
where
    K: Clone + DeserializeOwned + std::fmt::Debug + Resource<DynamicType = ()>,
{
    match api.delete(name, &DeleteParams::default()).await {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(response)) if response.code == 404 => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::is_digest_pinned_image;

    #[test]
    fn release_prober_image_is_fail_closed_and_digest_pinned() {
        assert!(is_digest_pinned_image(&format!(
            "registry.example/anydesign/release-prober@sha256:{}",
            "a".repeat(64)
        )));
        assert!(!is_digest_pinned_image(
            "registry.example/anydesign/release-prober:latest"
        ));
    }
}
