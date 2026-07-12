use serde::{Deserialize, Serialize};
use std::{error::Error, fmt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateIdError {
    kind: &'static str,
    value: String,
}

impl TemplateIdError {
    fn new(kind: &'static str, value: impl Into<String>) -> Self {
        Self {
            kind,
            value: value.into(),
        }
    }
}

impl fmt::Display for TemplateIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {}: {}", self.kind, self.value)
    }
}

impl Error for TemplateIdError {}

macro_rules! open_id {
    ($name:ident, $kind:literal, $validator:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, TemplateIdError> {
                let value = value.into();
                if !$validator(&value) {
                    return Err(TemplateIdError::new($kind, value));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

fn valid_kebab_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_lowercase()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
        && bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        })
        && !value.contains("--")
}

fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'@' | b'_'))
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

open_id!(TemplateId, "template id", valid_kebab_id);
open_id!(FrameworkId, "framework id", valid_kebab_id);
open_id!(
    SandboxExecutionProfileId,
    "sandbox execution profile id",
    valid_kebab_id
);
open_id!(TemplateVersion, "template version", valid_version);
open_id!(
    SandboxExecutionProfileVersion,
    "sandbox execution profile version",
    valid_version
);
open_id!(ManifestHash, "manifest sha256", valid_hash);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxExecutionProfileRef {
    pub id: SandboxExecutionProfileId,
    pub version: SandboxExecutionProfileVersion,
}
