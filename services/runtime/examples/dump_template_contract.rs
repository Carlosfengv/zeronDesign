use anydesign_runtime::{
    conversation::RuntimeStore,
    model_gateway::ToolCall,
    tools::{
        runtime::ToolExecutor,
        sandbox::sandbox_tools,
        streaming::{tool_result_error_text, StreamingToolExecutor},
    },
    types::AgentPhase,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

#[tokio::main]
async fn main() {
    let emit_patch = std::env::args().any(|argument| argument == "--patch");
    if emit_patch {
        println!("*** Begin Patch");
    }
    for template in ["astro-website", "fumadocs-docs"] {
        let workspace = std::env::temp_dir().join(format!(
            "anydesign-template-contract-{template}-{}",
            std::process::id()
        ));
        if workspace.exists() {
            fs::remove_dir_all(&workspace).unwrap();
        }
        for directory in ["project", "inputs", "state", "outputs"] {
            fs::create_dir_all(workspace.join(directory)).unwrap();
        }
        let store = RuntimeStore::new();
        let run = store
            .create_run(
                format!("contract-{template}"),
                AgentPhase::Build,
                "build".to_string(),
                "internal-balanced".to_string(),
                vec![],
            )
            .await;
        let executor = StreamingToolExecutor::new(ToolExecutor::new_with_workspace_root(
            sandbox_tools(),
            Default::default(),
            &workspace,
        ));
        let results = executor
            .execute_calls(
                store,
                &run.id,
                vec![ToolCall::new(
                    "init",
                    "project.init",
                    json!({ "template": template }),
                )],
            )
            .await;
        assert_eq!(results.len(), 1);
        assert!(
            !results[0].result.is_error,
            "{}",
            tool_result_error_text(&results[0].result)
        );

        let mut inventory = Vec::new();
        collect_files(
            &workspace.join("project"),
            &workspace.join("project"),
            &mut inventory,
        );
        inventory.sort_by(|left, right| left.0.cmp(&right.0));
        let mut digest = Sha256::new();
        for (path, bytes) in &inventory {
            digest.update(path.as_bytes());
            digest.update([0]);
            digest.update((bytes.len() as u64).to_be_bytes());
            digest.update(bytes);
        }
        if emit_patch {
            let module = template.replace('-', "_");
            for (path, bytes) in inventory {
                println!(
                    "*** Add File: /Users/carlos/Downloads/zeronDesign/services/runtime/src/templates/{module}/files/{path}"
                );
                let content = String::from_utf8(bytes).expect("template assets must be UTF-8");
                for line in content.split_inclusive('\n') {
                    print!("+{line}");
                }
                if !content.ends_with('\n') {
                    println!();
                }
            }
        } else {
            println!("{template} {:x}", digest.finalize());
            for (path, _) in inventory {
                println!("  {path}");
            }
        }
        fs::remove_dir_all(workspace).unwrap();
    }
    if emit_patch {
        println!("*** End Patch");
    }
}

fn collect_files(root: &Path, current: &Path, inventory: &mut Vec<(String, Vec<u8>)>) {
    for entry in fs::read_dir(current).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            collect_files(root, &entry.path(), inventory);
        } else {
            inventory.push((
                entry
                    .path()
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
                fs::read(entry.path()).unwrap(),
            ));
        }
    }
}
