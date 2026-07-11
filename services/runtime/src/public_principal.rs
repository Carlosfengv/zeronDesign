use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration, Utc};
use ed25519_dalek::{
    pkcs8::{DecodePublicKey, EncodePublicKey},
    Signature, Signer, SigningKey, Verifier, VerifyingKey,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fs, io, path::Path};

pub const PREVIEW_READ_OPERATION: &str = "preview.read";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublicPrincipalClaims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub jti: String,
    pub exp: i64,
    pub iat: i64,
    pub project_id: String,
    pub operations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicPrincipal {
    pub principal_id: String,
    pub project_id: String,
    pub operations: Vec<String>,
    pub jti: String,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicPrincipalError {
    MissingAuthorization,
    InvalidAuthorization,
    InvalidToken,
    ExpiredToken,
    WrongAudience,
    InvalidScope,
}

impl PublicPrincipalError {
    pub fn message(&self) -> &'static str {
        match self {
            Self::MissingAuthorization => "public_auth.missing",
            Self::InvalidAuthorization | Self::InvalidToken => "public_auth.invalid",
            Self::ExpiredToken => "public_auth.expired",
            Self::WrongAudience => "public_auth.wrong_audience",
            Self::InvalidScope => "public_auth.operation_forbidden",
        }
    }
}

#[derive(Debug, Deserialize)]
struct JwtHeader {
    alg: String,
    typ: String,
    kid: String,
}

#[derive(Clone)]
pub struct PublicPrincipalVerifier {
    keys: HashMap<String, VerifyingKey>,
    issuer: String,
    audience: String,
    max_ttl_seconds: i64,
}

impl PublicPrincipalVerifier {
    pub fn from_public_key_files(
        paths: &[impl AsRef<Path>],
        issuer: impl Into<String>,
        audience: impl Into<String>,
        max_ttl_seconds: u64,
    ) -> io::Result<Self> {
        let mut keys = HashMap::new();
        for path in paths {
            let der = fs::read(path)?;
            let key = VerifyingKey::from_public_key_der(&der)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
            keys.insert(public_principal_key_id(&key), key);
        }
        if keys.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "at least one public principal verification key is required",
            ));
        }
        Ok(Self {
            keys,
            issuer: issuer.into(),
            audience: audience.into(),
            max_ttl_seconds: max_ttl_seconds.clamp(1, 300) as i64,
        })
    }

    pub fn verify_bearer(
        &self,
        authorization: Option<&str>,
        required_operation: &str,
    ) -> Result<PublicPrincipal, PublicPrincipalError> {
        let authorization = authorization.ok_or(PublicPrincipalError::MissingAuthorization)?;
        let token = authorization
            .strip_prefix("Bearer ")
            .filter(|token| !token.trim().is_empty())
            .ok_or(PublicPrincipalError::InvalidAuthorization)?;
        let parts = token.split('.').collect::<Vec<_>>();
        if parts.len() != 3 {
            return Err(PublicPrincipalError::InvalidToken);
        }
        let header: JwtHeader = decode_json(parts[0])?;
        if header.alg != "EdDSA" || header.typ != "JWT" {
            return Err(PublicPrincipalError::InvalidToken);
        }
        let key = self
            .keys
            .get(&header.kid)
            .ok_or(PublicPrincipalError::InvalidToken)?;
        let signature = URL_SAFE_NO_PAD
            .decode(parts[2])
            .ok()
            .and_then(|bytes| Signature::from_slice(&bytes).ok())
            .ok_or(PublicPrincipalError::InvalidToken)?;
        key.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .map_err(|_| PublicPrincipalError::InvalidToken)?;
        let claims: PublicPrincipalClaims = decode_json(parts[1])?;
        let now = Utc::now().timestamp();
        if claims.exp <= now {
            return Err(PublicPrincipalError::ExpiredToken);
        }
        if claims.iss != self.issuer || claims.aud != self.audience {
            return Err(PublicPrincipalError::WrongAudience);
        }
        if claims.sub.trim().is_empty()
            || claims.project_id.trim().is_empty()
            || claims.jti.len() < 16
            || claims.iat > now + 30
            || claims.exp <= claims.iat
            || claims.exp - claims.iat > self.max_ttl_seconds
        {
            return Err(PublicPrincipalError::InvalidToken);
        }
        if !claims
            .operations
            .iter()
            .any(|operation| operation == required_operation)
            || claims
                .operations
                .iter()
                .any(|operation| operation != PREVIEW_READ_OPERATION)
        {
            return Err(PublicPrincipalError::InvalidScope);
        }
        Ok(PublicPrincipal {
            principal_id: claims.sub,
            project_id: claims.project_id,
            operations: claims.operations,
            jti: claims.jti,
            expires_at: claims.exp,
        })
    }
}

fn decode_json<T: for<'de> Deserialize<'de>>(part: &str) -> Result<T, PublicPrincipalError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|_| PublicPrincipalError::InvalidToken)?;
    serde_json::from_slice(&bytes).map_err(|_| PublicPrincipalError::InvalidToken)
}

pub fn public_principal_key_id(key: &VerifyingKey) -> String {
    let der = key
        .to_public_key_der()
        .expect("Ed25519 public key DER encoding must succeed");
    let digest = Sha256::digest(der.as_bytes());
    let suffix = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("ed25519-{suffix}")
}

#[derive(Clone)]
pub struct PublicPrincipalJwtIssuer {
    signing_key: SigningKey,
    key_id: String,
    issuer: String,
    audience: String,
    ttl: Duration,
}

impl PublicPrincipalJwtIssuer {
    pub fn from_signing_key(
        signing_key: SigningKey,
        issuer: impl Into<String>,
        audience: impl Into<String>,
        ttl_seconds: u64,
    ) -> Self {
        let key_id = public_principal_key_id(&signing_key.verifying_key());
        Self {
            signing_key,
            key_id,
            issuer: issuer.into(),
            audience: audience.into(),
            ttl: Duration::seconds(ttl_seconds.clamp(1, 300) as i64),
        }
    }

    pub fn issue(&self, mut claims: PublicPrincipalClaims) -> io::Result<String> {
        if claims.sub.trim().is_empty()
            || claims.project_id.trim().is_empty()
            || claims.jti.len() < 16
            || claims.operations != [PREVIEW_READ_OPERATION.to_string()]
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "public principal claims are invalid",
            ));
        }
        let now = Utc::now();
        claims.iss = self.issuer.clone();
        claims.aud = self.audience.clone();
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
        let payload = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&claims)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
        );
        let signing_input = format!("{header}.{payload}");
        let signature = self.signing_key.sign(signing_input.as_bytes());
        Ok(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::pkcs8::EncodePublicKey;

    #[test]
    fn verifies_scoped_short_lived_token() {
        let signing_key = SigningKey::from_bytes(&[9_u8; 32]);
        let public_der = signing_key.verifying_key().to_public_key_der().unwrap();
        let path = std::env::temp_dir().join(format!(
            "anydesign-public-principal-{}.der",
            std::process::id()
        ));
        fs::write(&path, public_der.as_bytes()).unwrap();
        let issuer = PublicPrincipalJwtIssuer::from_signing_key(
            signing_key,
            "anydesign-bff",
            "anydesign-runtime-public",
            60,
        );
        let token = issuer
            .issue(PublicPrincipalClaims {
                iss: String::new(),
                aud: String::new(),
                sub: "user-1".to_string(),
                jti: "public-jti-0000001".to_string(),
                exp: 0,
                iat: 0,
                project_id: "project-1".to_string(),
                operations: vec![PREVIEW_READ_OPERATION.to_string()],
            })
            .unwrap();
        let verifier = PublicPrincipalVerifier::from_public_key_files(
            &[path.as_path()],
            "anydesign-bff",
            "anydesign-runtime-public",
            60,
        )
        .unwrap();
        let principal = verifier
            .verify_bearer(Some(&format!("Bearer {token}")), PREVIEW_READ_OPERATION)
            .unwrap();
        assert_eq!(principal.principal_id, "user-1");
        assert_eq!(principal.project_id, "project-1");
        let _ = fs::remove_file(path);
    }
}
