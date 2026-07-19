use super::PublicationStore;
use crate::release::{ReleaseProtectionSet, ReleaseProtectionSource, ReleaseStore};
use anyhow::{Context, Result};
use async_trait::async_trait;
use k8s_openapi::api::apps::v1::Deployment;
use kube::{api::ListParams, Api, Client};
use std::{collections::BTreeSet, sync::Arc};

pub struct KubernetesReleaseProtectionSource {
    publication_store: Arc<PublicationStore>,
    release_store: Arc<ReleaseStore>,
    client: Client,
}

impl KubernetesReleaseProtectionSource {
    pub fn new(
        publication_store: Arc<PublicationStore>,
        release_store: Arc<ReleaseStore>,
        client: Client,
    ) -> Self {
        Self {
            publication_store,
            release_store,
            client,
        }
    }
}

#[async_trait]
impl ReleaseProtectionSource for KubernetesReleaseProtectionSource {
    async fn snapshot(&self) -> Result<ReleaseProtectionSet> {
        let mut release_ids = self.publication_store.protected_release_ids();
        release_ids.extend(
            self.release_store
                .recoverable_packagings()
                .into_iter()
                .map(|packaging| packaging.release_id),
        );
        let mut image_digests = BTreeSet::new();
        let namespaces = self
            .publication_store
            .runtimes()
            .into_iter()
            .map(|runtime| runtime.workspace_namespace)
            .collect::<BTreeSet<_>>();
        for namespace in namespaces {
            let deployments: Api<Deployment> =
                Api::namespaced(self.client.clone(), namespace.as_str());
            let live = deployments
                .list(&ListParams::default())
                .await
                .with_context(|| {
                    format!("scan live Published Deployments in {namespace} before Registry GC")
                })?;
            for deployment in live {
                if let Some(release_id) = deployment
                    .metadata
                    .labels
                    .as_ref()
                    .and_then(|labels| labels.get("anydesign.dev/release-id"))
                {
                    release_ids.insert(release_id.clone());
                }
                for container in deployment
                    .spec
                    .into_iter()
                    .flat_map(|spec| spec.template.spec)
                    .flat_map(|spec| spec.containers)
                {
                    if let Some(digest) = container
                        .image
                        .as_deref()
                        .and_then(|image| image.rsplit_once('@'))
                        .map(|(_, digest)| digest)
                        .filter(|digest| is_sha256_digest(digest))
                    {
                        image_digests.insert(digest.to_string());
                    }
                }
            }
        }
        for release_id in &release_ids {
            if let Some(digest) = self
                .release_store
                .release(release_id)
                .and_then(|release| release.runtime_image_digest)
            {
                image_digests.insert(digest);
            }
        }
        Ok(ReleaseProtectionSet {
            release_ids,
            image_digests,
        })
    }
}

fn is_sha256_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
}
