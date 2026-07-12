use anyhow::Result;
use async_trait::async_trait;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReleaseProtectionSet {
    pub release_ids: BTreeSet<String>,
    pub image_digests: BTreeSet<String>,
}

impl ReleaseProtectionSet {
    pub fn protects(&self, release_id: &str, image_digest: &str) -> bool {
        self.release_ids.contains(release_id) || self.image_digests.contains(image_digest)
    }
}

#[async_trait]
pub trait ReleaseProtectionSource: Send + Sync {
    async fn snapshot(&self) -> Result<ReleaseProtectionSet>;
}
