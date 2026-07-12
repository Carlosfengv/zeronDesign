use anydesign_runtime::tools::sandbox::sandbox_tools;
use serde_json::json;
use sha2::{Digest, Sha256};

fn main() {
    let tools = sandbox_tools();
    let contract = tools
        .iter()
        .map(|tool| {
            let schema = tool.input_schema();
            let digest = Sha256::digest(serde_json::to_vec(&schema).expect("serialize schema"));
            json!({
                "name": tool.name(),
                "schemaSha256": format!("{digest:x}"),
                "aliases": tool.aliases(),
                "loading": format!("{:?}", tool.tool_loading()),
                "interrupt": format!("{:?}", tool.interrupt_behavior()),
            })
        })
        .collect::<Vec<_>>();
    println!("{}", serde_json::to_string_pretty(&contract).unwrap());
}
