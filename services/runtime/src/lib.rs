pub mod acceptance_contract;
pub mod agent_hooks;
pub mod agent_loop;
pub mod artifact_access;
pub mod artifact_manifest;
pub mod artifact_publisher;
pub mod authorization;
pub mod channel_manager;
pub mod component_registry;
pub mod config;
pub mod control_plane_persistence;
pub mod conversation;
pub mod design_context;
pub mod design_profile;
pub mod design_profile_service;
pub mod draft_preview;
pub mod edit_guard;
pub mod generation_contract;
pub mod http_api;
pub mod model_gateway;
pub mod object_storage;
pub mod permission;
pub mod preview;
pub mod preview_access;
pub mod profile_token_sync;
pub mod profiles;
pub mod project;
pub mod project_asset;
pub mod public_principal;
pub mod publication;
pub mod publish_workflow;
pub mod query_session;
pub mod recovery;
pub mod release;
pub mod release_evidence;
pub mod repair_loop;
pub mod run_lifecycle;
pub mod runtime;
pub mod runtime_storage;
pub mod sandbox_adapter;
pub mod sandbox_profiles;
pub mod style_contract;
pub mod templates;
pub mod tools;
pub mod types;
pub mod visual_artifact_store;
pub mod visual_contracts;
pub mod visual_review;
pub mod workspace_auth;

pub use config::RuntimeConfig;
pub use conversation::RuntimeStore;

pub(crate) fn ensure_rustls_crypto_provider() -> anyhow::Result<()> {
    if rustls::crypto::CryptoProvider::get_default().is_some() {
        return Ok(());
    }
    match rustls::crypto::ring::default_provider().install_default() {
        Ok(()) => Ok(()),
        Err(_) if rustls::crypto::CryptoProvider::get_default().is_some() => Ok(()),
        Err(_) => anyhow::bail!("install Runtime rustls crypto provider"),
    }
}
