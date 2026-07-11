use anydesign_runtime::public_principal::{
    PublicPrincipalClaims, PublicPrincipalJwtIssuer, PublicPrincipalVerifier,
    PREVIEW_READ_OPERATION,
};
use ed25519_dalek::{pkcs8::EncodePublicKey, SigningKey};
use std::{fs, path::PathBuf};

fn write_public_key(name: &str, signing_key: &SigningKey) -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "anydesign-mock-bff-{}-{}",
        std::process::id(),
        name
    ));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("public.der");
    fs::write(
        &path,
        signing_key
            .verifying_key()
            .to_public_key_der()
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    path
}

fn issue(signing_key: SigningKey, principal_id: &str) -> String {
    PublicPrincipalJwtIssuer::from_signing_key(
        signing_key,
        "anydesign-bff",
        "anydesign-runtime-public",
        60,
    )
    .issue(PublicPrincipalClaims {
        iss: String::new(),
        aud: String::new(),
        sub: principal_id.to_string(),
        jti: format!("mock-bff-jti-{principal_id}-0001"),
        exp: 0,
        iat: 0,
        project_id: "project-1".to_string(),
        operations: vec![PREVIEW_READ_OPERATION.to_string()],
    })
    .unwrap()
}

#[test]
fn bff_tokens_support_current_and_previous_key_rotation_without_token_leakage() {
    let current = SigningKey::from_bytes(&[21_u8; 32]);
    let previous = SigningKey::from_bytes(&[22_u8; 32]);
    let current_path = write_public_key("current", &current);
    let previous_path = write_public_key("previous", &previous);
    let verifier = PublicPrincipalVerifier::from_public_key_files(
        &[current_path, previous_path],
        "anydesign-bff",
        "anydesign-runtime-public",
        60,
    )
    .unwrap();

    for token in [
        issue(current, "principal-current"),
        issue(previous, "principal-previous"),
    ] {
        let principal = verifier
            .verify_bearer(Some(&format!("Bearer {token}")), PREVIEW_READ_OPERATION)
            .unwrap();
        assert_eq!(principal.project_id, "project-1");
        assert!(!format!("{principal:?}").contains(&token));
    }
}
