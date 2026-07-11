use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration, Utc};
use ed25519_dalek::{
    pkcs8::{DecodePrivateKey, EncodePublicKey},
    Signer, SigningKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io, path::Path};

pub const WORKSPACE_CHANNEL_ISSUER: &str = "anydesign-runtime";
pub const WORKSPACE_CHANNEL_AUDIENCE: &str = "workspace-channel";
pub const WORKSPACE_CHANNEL_OPERATIONS: &[&str] = &[
    "fs.read",
    "fs.write",
    "process.exec",
    "process.manage",
    "archive.export",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceChannelClaims {
    pub iss: String,
    pub aud: String,
    pub exp: i64,
    pub iat: i64,
    pub jti: String,
    pub sandbox_binding_id: String,
    pub sandbox_name: String,
    pub pod_uid: String,
    pub project_id: String,
    pub run_id: String,
    pub operations: Vec<String>,
}

#[derive(Clone)]
pub struct WorkspaceChannelJwtIssuer {
    signing_key: SigningKey,
    key_id: String,
    ttl: Duration,
}

impl WorkspaceChannelJwtIssuer {
    pub fn from_pkcs8_der_file(path: impl AsRef<Path>, ttl_seconds: u64) -> io::Result<Self> {
        let der = fs::read(path)?;
        let signing_key = SigningKey::from_pkcs8_der(&der)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
        Ok(Self::from_signing_key(signing_key, ttl_seconds))
    }

    pub fn from_signing_key(signing_key: SigningKey, ttl_seconds: u64) -> Self {
        let key_id = workspace_channel_key_id(&signing_key);
        Self {
            signing_key,
            key_id,
            ttl: Duration::seconds(ttl_seconds.clamp(1, 300) as i64),
        }
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn issue(&self, mut claims: WorkspaceChannelClaims) -> io::Result<String> {
        if claims.jti.len() < 16
            || claims.operations.is_empty()
            || claims
                .operations
                .iter()
                .any(|operation| !WORKSPACE_CHANNEL_OPERATIONS.contains(&operation.as_str()))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "workspace channel claims contain invalid jti or operation scope",
            ));
        }
        let now = Utc::now();
        claims.iss = WORKSPACE_CHANNEL_ISSUER.to_string();
        claims.aud = WORKSPACE_CHANNEL_AUDIENCE.to_string();
        claims.iat = now.timestamp();
        claims.exp = (now + self.ttl).timestamp();

        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&serde_json::json!({
                "alg": "EdDSA",
                "typ": "JWT",
                "kid": self.key_id,
            }))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        );
        let payload = serde_json::to_vec(&claims)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let payload = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{header}.{payload}");
        let signature = self.signing_key.sign(signing_input.as_bytes());
        Ok(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }
}

fn workspace_channel_key_id(signing_key: &SigningKey) -> String {
    let der = signing_key
        .verifying_key()
        .to_public_key_der()
        .expect("Ed25519 public key DER encoding must succeed");
    let digest = Sha256::digest(der.as_bytes());
    let suffix = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("ed25519-{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signature, Verifier};

    #[test]
    fn issues_verifiable_short_lived_eddsa_jwt() {
        let signing_key = SigningKey::from_bytes(&[7_u8; 32]);
        let verifying_key = signing_key.verifying_key();
        let issuer = WorkspaceChannelJwtIssuer::from_signing_key(signing_key, 60);
        let token = issuer
            .issue(WorkspaceChannelClaims {
                iss: String::new(),
                aud: String::new(),
                exp: 0,
                iat: 0,
                jti: "jti-unit-test-0001".to_string(),
                sandbox_binding_id: "binding-1".to_string(),
                sandbox_name: "sandbox-1".to_string(),
                pod_uid: "pod-uid-1".to_string(),
                project_id: "project-1".to_string(),
                run_id: "run-1".to_string(),
                operations: vec!["fs.read".to_string()],
            })
            .expect("token");
        let parts = token.split('.').collect::<Vec<_>>();
        assert_eq!(parts.len(), 3);
        let signature =
            Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).expect("signature"))
                .expect("valid signature bytes");
        verifying_key
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .expect("valid signature");
        let claims: WorkspaceChannelClaims =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).expect("claims"))
                .expect("valid claims");
        assert_eq!(claims.iss, WORKSPACE_CHANNEL_ISSUER);
        assert_eq!(claims.aud, WORKSPACE_CHANNEL_AUDIENCE);
        assert!(claims.exp > claims.iat);
        assert!(claims.exp - claims.iat <= 60);
        let header: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).expect("header"))
                .expect("valid header");
        assert_eq!(header["kid"], issuer.key_id());
    }
}
