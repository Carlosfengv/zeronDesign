use crate::types::DesignProfile;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDiffChange {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<Value>,
}

pub(super) fn diff_profiles(
    from_profile: &DesignProfile,
    to_profile: &DesignProfile,
) -> Vec<ProfileDiffChange> {
    let mut from_value = serde_json::to_value(from_profile).unwrap_or(Value::Null);
    let mut to_value = serde_json::to_value(to_profile).unwrap_or(Value::Null);
    remove_metadata(&mut from_value);
    remove_metadata(&mut to_value);
    let mut changes = Vec::new();
    collect("", &from_value, &to_value, &mut changes);
    changes
}

fn remove_metadata(value: &mut Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    for key in ["id", "version", "createdAt", "updatedAt"] {
        object.remove(key);
    }
}

fn collect(path: &str, before: &Value, after: &Value, changes: &mut Vec<ProfileDiffChange>) {
    if before == after {
        return;
    }
    match (before, after) {
        (Value::Object(before_object), Value::Object(after_object)) => {
            let keys = before_object
                .keys()
                .chain(after_object.keys())
                .cloned()
                .collect::<BTreeSet<_>>();
            for key in keys {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                match (before_object.get(&key), after_object.get(&key)) {
                    (Some(before_child), Some(after_child)) => {
                        collect(&child_path, before_child, after_child, changes);
                    }
                    (Some(before_child), None) => changes.push(ProfileDiffChange {
                        path: child_path,
                        before: Some(before_child.clone()),
                        after: None,
                    }),
                    (None, Some(after_child)) => changes.push(ProfileDiffChange {
                        path: child_path,
                        before: None,
                        after: Some(after_child.clone()),
                    }),
                    (None, None) => {}
                }
            }
        }
        _ => changes.push(ProfileDiffChange {
            path: path.to_string(),
            before: Some(before.clone()),
            after: Some(after.clone()),
        }),
    }
}
