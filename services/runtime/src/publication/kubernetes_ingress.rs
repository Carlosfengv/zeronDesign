use super::{
    apply_params, assert_delete_identity, assert_expected_identity, delete_if_present, object,
    wait_absent, KubernetesWorkRuntimeBackend,
};
use crate::publication::{DesiredUnpublishRuntime, DesiredWorkRuntime, FIELD_MANAGER};
use crate::RuntimeConfig;
use anyhow::{bail, Context, Result};
use k8s_openapi::api::{
    apps::v1::Deployment,
    core::v1::Service,
    networking::v1::{Ingress, NetworkPolicy},
};
use kube::{
    api::{ListParams, Patch},
    Api, ResourceExt,
};
use serde_json::json;
use std::{net::SocketAddr, path::PathBuf, time::Duration};

#[derive(Debug, Clone)]
pub struct KubernetesIngressExposure {
    pub base_domain: String,
    pub ingress_class: String,
    pub tls_secret_name: String,
    pub certificate_issuer_name: Option<String>,
    pub probe_scheme: String,
    pub probe_resolve: Option<SocketAddr>,
    pub probe_ca_file: Option<PathBuf>,
}

impl KubernetesIngressExposure {
    pub(super) fn from_runtime_config(config: &RuntimeConfig) -> Result<Self> {
        let value = Self {
            base_domain: config
                .works_base_domain
                .clone()
                .context("WORKS_BASE_DOMAIN is required")?,
            ingress_class: config
                .works_ingress_class
                .clone()
                .context("WORKS_INGRESS_CLASS is required")?,
            tls_secret_name: config.works_tls_secret_name.clone().unwrap_or_default(),
            certificate_issuer_name: config.works_certificate_issuer_name.clone(),
            probe_scheme: config.works_probe_scheme.clone(),
            probe_resolve: config.works_probe_resolve,
            probe_ca_file: config.works_probe_ca_file.clone(),
        };
        value.validate()?;
        Ok(value)
    }

    pub(super) fn validate(&self) -> Result<()> {
        if self.base_domain.is_empty()
            || self.base_domain.len() > 200
            || self.base_domain.starts_with('.')
            || !self.base_domain.split('.').all(valid_dns_label)
        {
            bail!("WORKS_BASE_DOMAIN is invalid");
        }
        if self.ingress_class.trim().is_empty() {
            bail!("Ingress class is required");
        }
        if self.certificate_issuer_name.is_none() && self.tls_secret_name.trim().is_empty() {
            bail!("TLS secret name is required without a certificate issuer");
        }
        if self
            .certificate_issuer_name
            .as_ref()
            .is_some_and(|issuer| !valid_dns_label(issuer))
        {
            bail!("WORKS_CERTIFICATE_ISSUER_NAME is invalid");
        }
        if self.probe_scheme != "https" {
            bail!("Published external probe must use HTTPS");
        }
        if self
            .probe_ca_file
            .as_ref()
            .is_some_and(|path| !path.is_file())
        {
            bail!("WORKS_PROBE_CA_FILE does not exist");
        }
        Ok(())
    }

    fn host(&self, host_slug: &str) -> String {
        format!("{host_slug}.{}", self.base_domain)
    }

    fn tls_secret_name(&self, work_name: &str) -> String {
        if self.certificate_issuer_name.is_some() {
            format!("{work_name}-tls")
        } else {
            self.tls_secret_name.clone()
        }
    }

    fn client_for(&self, host: &str) -> Result<reqwest::Client> {
        let mut builder = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10));
        if let Some(address) = self.probe_resolve {
            builder = builder.resolve(host, address);
        }
        if let Some(path) = &self.probe_ca_file {
            let certificate = reqwest::Certificate::from_pem(&std::fs::read(path)?)
                .context("parse Published external probe CA")?;
            builder = builder.add_root_certificate(certificate);
        }
        builder
            .build()
            .context("build Published external probe client")
    }
}

impl KubernetesWorkRuntimeBackend {
    pub(super) async fn apply_ingress(
        &self,
        desired: &DesiredWorkRuntime,
        exposure: &KubernetesIngressExposure,
    ) -> Result<Ingress> {
        let ingresses: Api<Ingress> = Api::namespaced(self.client.clone(), &desired.namespace);
        assert_expected_identity(
            &ingresses,
            &desired.work_name,
            desired.expected_ingress_uid.as_deref(),
            &desired.owner_record_id,
        )
        .await?;
        let host = exposure.host(&desired.host_slug);
        for existing in ingresses.list(&ListParams::default()).await? {
            if existing.name_any() != desired.work_name
                && existing.spec.as_ref().is_some_and(|spec| {
                    spec.rules
                        .iter()
                        .flatten()
                        .any(|rule| rule.host.as_deref() == Some(&host))
                })
            {
                bail!("Published host is already owned by another Ingress");
            }
        }
        let mut annotations = json!({
            "anydesign.dev/release-id": desired.release_id,
            "nginx.ingress.kubernetes.io/ssl-redirect": "true"
        });
        if let Some(issuer) = &exposure.certificate_issuer_name {
            annotations["cert-manager.io/cluster-issuer"] = json!(issuer);
            annotations["cert-manager.io/duration"] = json!("2160h");
            annotations["cert-manager.io/renew-before"] = json!("720h");
            annotations["cert-manager.io/private-key-algorithm"] = json!("ECDSA");
            annotations["cert-manager.io/private-key-size"] = json!("256");
            annotations["cert-manager.io/private-key-rotation-policy"] = json!("Always");
            annotations["cert-manager.io/revision-history-limit"] = json!("3");
        }
        let tls_secret_name = exposure.tls_secret_name(&desired.work_name);
        let ingress = object::<Ingress>(json!({
            "apiVersion": "networking.k8s.io/v1",
            "kind": "Ingress",
            "metadata": {
                "name": desired.work_name,
                "namespace": desired.namespace,
                "labels": desired.labels,
                "annotations": annotations
            },
            "spec": {
                "ingressClassName": exposure.ingress_class,
                "tls": [{"hosts": [host], "secretName": tls_secret_name}],
                "rules": [{"host": host, "http": {"paths": [{
                    "path": "/", "pathType": "Prefix",
                    "backend": {"service": {"name": desired.stable_service_name, "port": {"name": "http"}}}
                }]}}]
            }
        }))?;
        ingresses
            .patch(&desired.work_name, &apply_params(), &Patch::Apply(&ingress))
            .await
            .context("server-side apply per-work Ingress")
    }

    pub(super) async fn verify_external_release(
        &self,
        desired: &DesiredWorkRuntime,
        exposure: &KubernetesIngressExposure,
    ) -> Result<()> {
        self.verify_external_release_id(desired, exposure, &desired.release_id)
            .await
    }

    pub(super) async fn verify_external_release_id(
        &self,
        desired: &DesiredWorkRuntime,
        exposure: &KubernetesIngressExposure,
        release_id: &str,
    ) -> Result<()> {
        let host = exposure.host(&desired.host_slug);
        let client = exposure.client_for(&host)?;
        let url = format!(
            "{}://{}{}",
            exposure.probe_scheme, host, desired.release_path
        );
        let deadline = tokio::time::Instant::now() + self.timeout;
        loop {
            if let Ok(response) = client.get(&url).send().await {
                let release_header = response
                    .headers()
                    .get("x-anydesign-release-id")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                if response.status().is_success()
                    && release_header.as_deref() == Some(release_id)
                    && response.text().await?.contains(release_id)
                {
                    return Ok(());
                }
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("external Published host did not return the desired release identity");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    pub(super) async fn delete_published_resources(
        &self,
        desired: &DesiredUnpublishRuntime,
    ) -> Result<()> {
        let ingresses: Api<Ingress> = Api::namespaced(self.client.clone(), &desired.namespace);
        assert_delete_identity(
            &ingresses,
            &desired.ingress_name,
            desired.expected_ingress_uid.as_deref(),
            &desired.owner_record_id,
        )
        .await?;
        delete_if_present(&ingresses, &desired.ingress_name).await?;
        wait_absent(&ingresses, &desired.ingress_name, self.timeout).await?;
        if let Some(exposure) = &self.exposure {
            self.verify_external_closed(desired, exposure).await?;
        }

        let services: Api<Service> = Api::namespaced(self.client.clone(), &desired.namespace);
        assert_delete_identity(
            &services,
            &desired.service_name,
            desired.expected_service_uid.as_deref(),
            &desired.owner_record_id,
        )
        .await?;
        delete_if_present(&services, &desired.service_name).await?;
        let deployments: Api<Deployment> = Api::namespaced(self.client.clone(), &desired.namespace);
        let selector = format!(
            "anydesign.dev/work={},app.kubernetes.io/managed-by={}",
            desired.work_name, FIELD_MANAGER
        );
        let names = deployments
            .list(&ListParams::default().labels(&selector))
            .await?
            .into_iter()
            .map(|deployment| {
                assert_owned_metadata(
                    deployment.metadata.labels.as_ref(),
                    &deployment.name_any(),
                    &desired.owner_record_id,
                )?;
                Ok(deployment.name_any())
            })
            .collect::<Result<Vec<_>>>()?;
        for name in &names {
            delete_if_present(&deployments, name).await?;
        }
        let policies: Api<NetworkPolicy> = Api::namespaced(self.client.clone(), &desired.namespace);
        assert_delete_identity(
            &policies,
            &desired.network_policy_name,
            None,
            &desired.owner_record_id,
        )
        .await?;
        delete_if_present(&policies, &desired.network_policy_name).await?;
        wait_absent(&services, &desired.service_name, self.timeout).await?;
        for name in &names {
            wait_absent(&deployments, name, self.timeout).await?;
        }
        wait_absent(&policies, &desired.network_policy_name, self.timeout).await?;
        Ok(())
    }

    async fn verify_external_closed(
        &self,
        desired: &DesiredUnpublishRuntime,
        exposure: &KubernetesIngressExposure,
    ) -> Result<()> {
        let host = exposure.host(&desired.host_slug);
        let client = exposure.client_for(&host)?;
        let url = format!("{}://{}/", exposure.probe_scheme, host);
        let deadline = tokio::time::Instant::now() + self.timeout;
        let mut consecutive_closed = 0;
        loop {
            let closed = client
                .get(&url)
                .send()
                .await
                .is_ok_and(|response| matches!(response.status().as_u16(), 404 | 410));
            consecutive_closed = if closed { consecutive_closed + 1 } else { 0 };
            if consecutive_closed >= 3 {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                bail!("Published host remained externally routable after Ingress deletion");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}

fn assert_owned_metadata(
    labels: Option<&std::collections::BTreeMap<String, String>>,
    name: &str,
    expected_owner_record_id: &str,
) -> Result<()> {
    if labels
        .and_then(|labels| labels.get("app.kubernetes.io/managed-by"))
        .map(String::as_str)
        != Some(FIELD_MANAGER)
        || labels
            .and_then(|labels| labels.get("anydesign.dev/owner-record-id"))
            .map(String::as_str)
            != Some(expected_owner_record_id)
    {
        bail!("refusing to delete Kubernetes resource not owned by this work: {name}");
    }
    Ok(())
}

fn valid_dns_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && label
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && label.as_bytes()[0].is_ascii_alphanumeric()
        && label.as_bytes()[label.len() - 1].is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::KubernetesIngressExposure;

    fn exposure(
        tls_secret_name: &str,
        certificate_issuer_name: Option<&str>,
    ) -> KubernetesIngressExposure {
        KubernetesIngressExposure {
            base_domain: "works.example.test".into(),
            ingress_class: "traefik".into(),
            tls_secret_name: tls_secret_name.into(),
            certificate_issuer_name: certificate_issuer_name.map(str::to_string),
            probe_scheme: "https".into(),
            probe_resolve: None,
            probe_ca_file: None,
        }
    }

    #[test]
    fn managed_certificate_mode_derives_a_per_work_secret() {
        let exposure = exposure("", Some("works-cluster-issuer"));

        exposure.validate().unwrap();

        assert_eq!(exposure.tls_secret_name("work-abc123"), "work-abc123-tls");
    }

    #[test]
    fn legacy_certificate_mode_keeps_the_shared_secret() {
        let exposure = exposure("works-wildcard-tls", None);

        exposure.validate().unwrap();

        assert_eq!(
            exposure.tls_secret_name("work-abc123"),
            "works-wildcard-tls"
        );
    }

    #[test]
    fn managed_certificate_mode_rejects_an_invalid_cluster_issuer_name() {
        let exposure = exposure("", Some("Invalid/Issuer"));

        assert_eq!(
            exposure.validate().unwrap_err().to_string(),
            "WORKS_CERTIFICATE_ISSUER_NAME is invalid"
        );
    }
}
