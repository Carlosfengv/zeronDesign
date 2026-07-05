use serde_json::{json, Value};

pub fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false,
    })
}

pub fn string_schema(description: &str) -> Value {
    json!({
        "type": "string",
        "description": description,
    })
}
