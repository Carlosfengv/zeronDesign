use super::KubernetesWorkRuntimeBackend;
use crate::publication::{DesiredWorkRuntime, FIELD_MANAGER};
use anyhow::{bail, Context, Result};
use k8s_openapi::api::{
    core::v1::{Pod, Service},
    discovery::v1::EndpointSlice,
};
use kube::{
    api::{ListParams, Patch, PatchParams},
    Api,
};
use serde_json::json;
use std::time::Duration;

impl KubernetesWorkRuntimeBackend {
    pub(super) async fn switch_stable_service(
        &self,
        desired: &DesiredWorkRuntime,
        expected_release_id: &str,
        target_release_id: &str,
    ) -> Result<Service> {
        let services: Api<Service> = Api::namespaced(self.client.clone(), &desired.namespace);
        let current = services
            .get(&desired.stable_service_name)
            .await
            .context("read stable Service before selector switch")?;
        assert_service_identity(&current, desired)?;
        let selected_release = service_release_selector(&current)?;
        if selected_release == target_release_id {
            return Ok(current);
        }
        if selected_release != expected_release_id {
            bail!(
                "stable Service selector drift: expected {expected_release_id}, observed {selected_release}"
            );
        }
        let resource_version = current
            .metadata
            .resource_version
            .clone()
            .context("stable Service is missing resourceVersion")?;
        services
            .patch(
                &desired.stable_service_name,
                &PatchParams::default(),
                &Patch::Merge(json!({
                    "metadata": {"resourceVersion": resource_version},
                    "spec": {"selector": {
                        "anydesign.dev/work": desired.work_name,
                        "anydesign.dev/release-id": target_release_id,
                    }}
                })),
            )
            .await
            .context("CAS switch stable Service release selector")
    }

    pub(super) async fn wait_endpoint_slice_release(
        &self,
        desired: &DesiredWorkRuntime,
        release_id: &str,
    ) -> Result<()> {
        let slices: Api<EndpointSlice> = Api::namespaced(self.client.clone(), &desired.namespace);
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), &desired.namespace);
        let selector = format!("kubernetes.io/service-name={}", desired.stable_service_name);
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            let mut ready_target_count = 0usize;
            let mut converged = true;
            for slice in slices
                .list(&ListParams::default().labels(&selector))
                .await?
            {
                for endpoint in slice.endpoints.into_iter().flatten() {
                    let Some(target) = endpoint.target_ref else {
                        converged = false;
                        continue;
                    };
                    if target.kind.as_deref() != Some("Pod") {
                        converged = false;
                        continue;
                    }
                    let Some(target_name) = target.name.as_deref() else {
                        converged = false;
                        continue;
                    };
                    let pod = pods.get(target_name).await?;
                    if pod
                        .metadata
                        .labels
                        .as_ref()
                        .and_then(|labels| labels.get("anydesign.dev/release-id"))
                        .map(String::as_str)
                        != Some(release_id)
                    {
                        converged = false;
                    } else if endpoint
                        .conditions
                        .as_ref()
                        .and_then(|conditions| conditions.ready)
                        != Some(false)
                    {
                        ready_target_count += 1;
                    }
                }
            }
            if converged && ready_target_count > 0 {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("EndpointSlice did not converge exclusively to release {release_id}");
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
}

fn assert_service_identity(service: &Service, desired: &DesiredWorkRuntime) -> Result<()> {
    if desired
        .expected_service_uid
        .as_deref()
        .is_some_and(|expected| service.metadata.uid.as_deref() != Some(expected))
    {
        bail!("stable Service UID drift detected");
    }
    let labels = service.metadata.labels.as_ref();
    if labels
        .and_then(|labels| labels.get("app.kubernetes.io/managed-by"))
        .map(String::as_str)
        != Some(FIELD_MANAGER)
        || labels
            .and_then(|labels| labels.get("anydesign.dev/owner-record-id"))
            .map(String::as_str)
            != Some(desired.owner_record_id.as_str())
    {
        bail!("stable Service ownership metadata drift detected");
    }
    let spec = service
        .spec
        .as_ref()
        .context("stable Service is missing spec")?;
    if spec.type_.as_deref() != Some("ClusterIP")
        || spec
            .selector
            .as_ref()
            .and_then(|selector| selector.get("anydesign.dev/work"))
            .map(String::as_str)
            != Some(desired.work_name.as_str())
        || spec.ports.as_ref().is_none_or(|ports| {
            ports.len() != 1 || ports[0].name.as_deref() != Some("http") || ports[0].port != 80
        })
    {
        bail!("stable Service controlled fields drift detected");
    }
    Ok(())
}

fn service_release_selector(service: &Service) -> Result<&str> {
    service
        .spec
        .as_ref()
        .and_then(|spec| spec.selector.as_ref())
        .and_then(|selector| selector.get("anydesign.dev/release-id"))
        .map(String::as_str)
        .context("stable Service is missing release selector")
}
