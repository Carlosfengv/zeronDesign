use crate::artifact_publisher::safe_segment;
use serde_json::Value;
use std::{fs, path::PathBuf};

pub trait RuntimeEvidenceStore: Send + Sync {
    fn read_screenshot(
        &self,
        project_id: &str,
        run_id: &str,
        screenshot_id: &str,
    ) -> anyhow::Result<Value>;
}

#[derive(Debug, Clone)]
pub struct FileRuntimeEvidenceStore {
    runtime_storage_dir: PathBuf,
}

impl FileRuntimeEvidenceStore {
    pub fn new(runtime_storage_dir: impl Into<PathBuf>) -> Self {
        Self {
            runtime_storage_dir: runtime_storage_dir.into(),
        }
    }
}

impl RuntimeEvidenceStore for FileRuntimeEvidenceStore {
    fn read_screenshot(
        &self,
        project_id: &str,
        run_id: &str,
        screenshot_id: &str,
    ) -> anyhow::Result<Value> {
        let path = self
            .runtime_storage_dir
            .join("screenshots")
            .join(safe_segment(project_id))
            .join(safe_segment(run_id))
            .join(format!("{}.json", safe_segment(screenshot_id)));
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_identifiers_cannot_escape_the_runtime_root() {
        let root = std::env::temp_dir().join(format!(
            "runtime-evidence-store-{}-{}",
            std::process::id(),
            rand::random::<u64>()
        ));
        let project_id = "project/../outside";
        let run_id = "run/../outside";
        let screenshot_id = "screenshot/../outside";
        let expected = root
            .join("screenshots")
            .join(safe_segment(project_id))
            .join(safe_segment(run_id))
            .join(format!("{}.json", safe_segment(screenshot_id)));
        assert!(expected.starts_with(root.join("screenshots")));
        fs::create_dir_all(expected.parent().unwrap()).unwrap();
        fs::write(&expected, br#"{"safe":true}"#).unwrap();

        let value = FileRuntimeEvidenceStore::new(&root)
            .read_screenshot(project_id, run_id, screenshot_id)
            .unwrap();

        assert_eq!(value["safe"], true);
        let _ = fs::remove_dir_all(root);
    }
}
