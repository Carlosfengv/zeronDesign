use crate::conversation::RuntimeStore;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};

pub async fn list_sources(store: &RuntimeStore, run_id: &str) -> Result<Value> {
    let sources = store.content_sources(run_id).await;
    Ok(json!({
        "sources": sources
            .iter()
            .map(|source| json!({
                "id": source.id,
                "kind": source.kind,
                "readable": source.readable,
            }))
            .collect::<Vec<_>>()
    }))
}

pub async fn read_source(store: &RuntimeStore, run_id: &str, input: &Value) -> Result<Value> {
    let id = input
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("content.read_source requires id"))?;
    let source = store
        .content_sources(run_id)
        .await
        .into_iter()
        .find(|source| source.id == id)
        .ok_or_else(|| anyhow!("content source not found: {id}"))?;

    if !source.readable {
        return Err(anyhow!("content source is unreadable: {id}"));
    }

    Ok(json!({
        "id": source.id,
        "kind": source.kind,
        "text": source.text,
    }))
}
