use serde::Deserialize;
use serde_yaml::Value;

const NETWORK_POLICY_YAML: &str =
    include_str!("../../../infra/agent-sandbox/network/default-deny.yaml");
const RBAC_YAML: &str =
    include_str!("../../../infra/agent-sandbox/rbac/runtime-service-account.yaml");
const TEMPLATE_YAML: &str =
    include_str!("../../../infra/agent-sandbox/astro-website/sandbox-template.yaml");
const DOCS_TEMPLATE_YAML: &str =
    include_str!("../../../infra/agent-sandbox/fumadocs-docs/sandbox-template.yaml");
const RUNTIME_DEPLOYMENT_YAML: &str =
    include_str!("../../../infra/agent-sandbox/runtime/deployment.yaml");
const ASTRO_SANDBOX_DOCKERFILE: &str =
    include_str!("../../../infra/agent-sandbox/astro-website/Dockerfile");
const WORKSPACE_INIT_SH: &str = include_str!("../../../infra/agent-sandbox/base/workspace-init.sh");
const WORKSPACE_CHANNEL_SERVER_JS: &str =
    include_str!("../../../infra/agent-sandbox/base/workspace-channel-server.js");
const IMAGE_LOCK_JSON: &str = include_str!("../../../infra/agent-sandbox/images.lock.json");
const K8S_E2E_SH: &str = include_str!("../../../infra/agent-sandbox/run-k8s-e2e.sh");
const RUNTIME_RC_GATE_SH: &str =
    include_str!("../../../infra/agent-sandbox/run-runtime-rc-gate.sh");
const RUNTIME_RC_PREFLIGHT_SH: &str =
    include_str!("../../../infra/agent-sandbox/preflight-runtime-rc.sh");

#[test]
fn runtime_rc_refreshes_short_lived_principal_after_provider_waits() {
    assert!(RUNTIME_RC_GATE_SH.contains(
        "# Provider SSE waits may outlive the deliberately short principal JWT.\n  principal_token=\"$(issue_principal_token \"${project_id}\")\""
    ));
    assert!(RUNTIME_RC_GATE_SH.contains(
        "# Never reuse a token issued before a model/build wait for artifact access.\n  principal_token=\"$(issue_principal_token \"${project_id}\")\"\n  artifact_url="
    ));

    let refresh_count = RUNTIME_RC_GATE_SH
        .matches("principal_token=\"$(issue_principal_token \"${project_id}\")\"")
        .count();
    assert!(
        refresh_count >= 15,
        "RC gate must refresh 120-second principal tokens around every long provider wait; found {refresh_count} refreshes"
    );
}

#[test]
fn runtime_rc_proves_released_artifacts_with_a_fresh_project_principal() {
    assert!(RUNTIME_RC_GATE_SH
        .contains("Artifact routes remain principal-protected after Sandbox release. Refresh the"));
    assert!(RUNTIME_RC_GATE_SH.contains("artifact_status=\"$(curl --silent"));
    assert!(RUNTIME_RC_GATE_SH
        .contains("-H \"authorization: Bearer ${principal_token}\" \"${artifact_url}\")\""));
    assert!(RUNTIME_RC_GATE_SH
        .contains("artifactAccessAfterRelease={authentication:\"project-principal\""));
}

#[test]
fn runtime_rc_asserts_real_docs_on_the_fumadocs_content_route() {
    assert!(RUNTIME_RC_GATE_SH
        .contains("The initialized Fumadocs source renders authored content at /docs/;"));
    assert!(RUNTIME_RC_GATE_SH.contains("artifact_assertion_path=\"/docs/\""));
    assert!(RUNTIME_RC_GATE_SH.contains("artifact_assertion_url=\"${artifact_url}docs/\""));
    assert!(RUNTIME_RC_GATE_SH.contains("evidence.route=process.argv[2]"));
}

#[test]
fn runtime_rc_real_provider_build_and_edit_have_distinct_acceptance_text() {
    assert!(RUNTIME_RC_GATE_SH.contains("build_expected_text=\"RC ${kind} Built\""));
    assert!(RUNTIME_RC_GATE_SH
        .contains("Make and verify that source mutation before the first preview.publish call."));
    assert!(RUNTIME_RC_GATE_SH
        .contains("Before any preview.publish call, make one minimal source patch"));
    assert!(RUNTIME_RC_GATE_SH
        .contains("replaces the exact text ${build_expected_text} with ${expected_text}"));
}

#[test]
fn runtime_rc_registry_fallback_is_lock_bound_and_evidence_backed() {
    let lock: serde_json::Value = serde_json::from_str(IMAGE_LOCK_JSON).unwrap();
    for name in ["rustBuilder", "debianRuntime", "sandboxNode"] {
        let image = &lock["images"][name];
        assert!(image["fallbackRef"]
            .as_str()
            .is_some_and(|value| value.starts_with("mirror.gcr.io/library/")));
        assert!(image["digest"]
            .as_str()
            .is_some_and(|value| value.len() == 71));
    }
    assert!(RUNTIME_RC_PREFLIGHT_SH.contains("runtime-rc-preflight@1"));
    assert!(RUNTIME_RC_PREFLIGHT_SH.contains("lockedDigestVerified: true"));
    assert!(RUNTIME_RC_PREFLIGHT_SH.contains("mutableTagMatchesLock: true"));
    assert!(RUNTIME_RC_GATE_SH.contains("--preflight \"${preflight_evidence}\""));
}

#[test]
fn sandbox_network_policy_defaults_to_deny_all_ingress_and_egress() {
    let docs = yaml_documents(NETWORK_POLICY_YAML);
    let default_deny = named_doc(&docs, "NetworkPolicy", "anydesign-sandbox-default-deny");
    let spec = field(default_deny, "spec");

    assert!(field(spec, "podSelector").as_mapping().unwrap().is_empty());
    assert_eq!(
        string_list(field(spec, "policyTypes")),
        vec!["Ingress".to_string(), "Egress".to_string()]
    );
    assert!(field_optional(spec, "ingress").is_none());
    assert!(field_optional(spec, "egress").is_none());
}

#[test]
fn sandbox_network_policy_allows_runtime_preview_and_internal_npm_proxy_only() {
    let docs = yaml_documents(NETWORK_POLICY_YAML);

    let runtime = named_doc(
        &docs,
        "NetworkPolicy",
        "anydesign-sandbox-allow-runtime-channel",
    );
    let runtime_spec = field(runtime, "spec");
    assert_eq!(
        string_list(field(runtime_spec, "policyTypes")),
        vec!["Ingress".to_string()]
    );
    let ingress = field(runtime_spec, "ingress").as_sequence().unwrap();
    assert_eq!(ingress.len(), 1);
    assert_yaml_contains(runtime, "anydesign-runtime");
    assert_yaml_contains(runtime, "3001");
    assert_yaml_contains(runtime, "4321");
    assert!(!yaml_contains(runtime, "database"));
    assert!(!yaml_contains(runtime, "postgres"));

    let dns = named_doc(&docs, "NetworkPolicy", "anydesign-sandbox-allow-dns-egress");
    let dns_spec = field(dns, "spec");
    assert_eq!(
        string_list(field(dns_spec, "policyTypes")),
        vec!["Egress".to_string()]
    );
    let egress = field(dns_spec, "egress").as_sequence().unwrap();
    assert_eq!(egress.len(), 1);
    assert_yaml_contains(dns, "kube-system");
    assert_yaml_contains(dns, "kube-dns");
    assert_yaml_contains(dns, "53");
    assert!(!yaml_contains(dns, "0.0.0.0/0"));
    assert!(!yaml_contains(dns, "::/0"));

    let npm = named_doc(
        &docs,
        "NetworkPolicy",
        "anydesign-sandbox-allow-npm-proxy-egress",
    );
    assert_yaml_contains(npm, "anydesign-runtime");
    assert_yaml_contains(npm, "anydesign-npm-proxy");
    assert_yaml_contains(npm, "4873");

    for doc in docs
        .iter()
        .filter(|doc| field(doc, "kind").as_str() == Some("NetworkPolicy"))
    {
        assert!(!yaml_contains(doc, "0.0.0.0/0"));
        assert!(!yaml_contains(doc, "::/0"));
        assert!(!yaml_contains(doc, "registry.npmjs.org"));
    }
}

#[test]
fn sandbox_templates_delegate_egress_only_to_repository_network_policies() {
    for (yaml, name) in [
        (TEMPLATE_YAML, "anydesign-astro-website"),
        (DOCS_TEMPLATE_YAML, "anydesign-fumadocs-docs"),
    ] {
        let docs = yaml_documents(yaml);
        let template = named_doc(&docs, "SandboxTemplate", name);
        let spec = field(template, "spec");

        assert_eq!(
            field(spec, "networkPolicyManagement").as_str(),
            Some("Unmanaged"),
            "{name} must not let the Sandbox controller generate its public-internet allow policy"
        );
        assert!(
            field_optional(spec, "networkPolicy").is_none(),
            "{name} must use infra/agent-sandbox/network/default-deny.yaml as its only egress authority"
        );
    }
}

#[test]
fn local_path_workspace_helper_is_digest_pinned_before_sandbox_pvcs() {
    let lock: serde_json::Value = serde_json::from_str(IMAGE_LOCK_JSON).unwrap();
    let helper = &lock["images"]["localPathHelper"];

    assert_eq!(
        helper["ref"].as_str(),
        Some("docker.io/rancher/mirrored-library-busybox:1.36.1")
    );
    assert_eq!(
        helper["digest"].as_str(),
        Some("sha256:8a45424ddf949bbe9bb3231b05f9032a45da5cd036eb4867b511b00734756d6f")
    );
    assert!(K8S_E2E_SH.contains("local_path_helper_image=\"$(locked_image localPathHelper)\""));
    assert!(K8S_E2E_SH.contains("configmap local-path-config"));
    assert!(K8S_E2E_SH.contains("helperPod.yaml"));
    assert!(K8S_E2E_SH.contains("rollout restart deployment/local-path-provisioner"));
}

#[test]
fn runtime_rbac_cannot_read_secrets_or_mutate_sandbox_pods() {
    let docs = yaml_documents(RBAC_YAML);
    let role = named_doc(&docs, "Role", "anydesign-runtime-sandbox-claims");
    let rules = field(role, "rules").as_sequence().unwrap();

    let forbidden_resources = ["secrets", "configmaps", "pods/exec", "pods/log"];
    let forbidden_verbs = ["update", "patch", "bind", "escalate", "impersonate"];

    for rule in rules {
        let resources = string_list(field(rule, "resources"));
        let verbs = string_list(field(rule, "verbs"));
        for forbidden in forbidden_resources {
            assert!(
                !resources.iter().any(|resource| resource == forbidden),
                "runtime RBAC must not grant {forbidden}: {rule:?}"
            );
        }
        for forbidden in forbidden_verbs {
            assert!(
                !verbs.iter().any(|verb| verb == forbidden),
                "runtime RBAC must not grant verb {forbidden}: {rule:?}"
            );
        }
    }
}

#[test]
fn workspace_channel_requires_mtls_workload_identities() {
    let template_docs = yaml_documents(TEMPLATE_YAML);
    let template = named_doc(&template_docs, "SandboxTemplate", "anydesign-astro-website");
    assert_yaml_contains(template, "serviceAccountName");
    assert_yaml_contains(template, "anydesign-sandbox");
    assert_yaml_contains(template, "WORKSPACE_CHANNEL_TLS_MODE");
    assert_yaml_contains(template, "anydesign-sandbox-channel-server");
    assert_yaml_contains(template, "workspace-channel-tls");
    assert!(WORKSPACE_CHANNEL_SERVER_JS.contains("https.createServer"));
    assert!(WORKSPACE_CHANNEL_SERVER_JS.contains("requestCert: true"));
    assert!(WORKSPACE_CHANNEL_SERVER_JS.contains("rejectUnauthorized: true"));
    assert!(WORKSPACE_CHANNEL_SERVER_JS.contains("EXPECTED_RUNTIME_SAN"));

    let runtime_docs = yaml_documents(RUNTIME_DEPLOYMENT_YAML);
    let runtime = named_doc(&runtime_docs, "Deployment", "anydesign-runtime");
    assert_yaml_contains(runtime, "WORKSPACE_CHANNEL_CLIENT_CERT_FILE");
    assert_yaml_contains(runtime, "WORKSPACE_CHANNEL_CLIENT_KEY_FILE");
    assert_yaml_contains(runtime, "WORKSPACE_CHANNEL_SERVER_SAN");
    assert_yaml_contains(runtime, "anydesign-runtime-channel-client");
}

#[test]
fn sandbox_template_does_not_expose_control_plane_database_configuration() {
    let docs = yaml_documents(TEMPLATE_YAML);
    let template = named_doc(&docs, "SandboxTemplate", "anydesign-astro-website");
    for forbidden in [
        "DATABASE_URL",
        "POSTGRES",
        "MYSQL",
        "REDIS",
        "DB_HOST",
        "DB_PASSWORD",
        "SUPABASE",
    ] {
        assert!(
            !yaml_contains(template, forbidden),
            "sandbox template must not expose control-plane data config: {forbidden}"
        );
    }
}

#[test]
fn sandbox_template_runs_workspace_init_before_agent_work() {
    let docs = yaml_documents(TEMPLATE_YAML);
    let template = named_doc(&docs, "SandboxTemplate", "anydesign-astro-website");
    let command = field(astro_template_container(template), "command")
        .as_sequence()
        .expect("container.command must be a list")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");

    assert!(
        command.contains("/opt/anydesign/bootstrap/workspace-init.sh"),
        "SandboxTemplate must run workspace-init before sleeping or accepting tools"
    );
}

#[test]
fn sandbox_template_starts_workspace_channel_server_after_workspace_init() {
    let docs = yaml_documents(TEMPLATE_YAML);
    let template = named_doc(&docs, "SandboxTemplate", "anydesign-astro-website");
    let command = field(astro_template_container(template), "command")
        .as_sequence()
        .expect("container.command must be a list")
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" ");

    let init_index = command
        .find("/opt/anydesign/bootstrap/workspace-init.sh")
        .expect("SandboxTemplate must run workspace-init");
    let server_index = command
        .find("/opt/anydesign/bootstrap/workspace-channel-server.js")
        .expect("SandboxTemplate must start the workspace channel server");
    assert!(
        init_index < server_index,
        "workspace-init must run before the workspace channel server starts"
    );
}

#[test]
fn sandbox_template_uses_image_with_baked_bootstrap_assets() {
    let docs = yaml_documents(TEMPLATE_YAML);
    let template = named_doc(&docs, "SandboxTemplate", "anydesign-astro-website");
    let image = field(astro_template_container(template), "image")
        .as_str()
        .expect("container.image must be set");

    assert!(
        image.contains("zerondesign/astro-website-sandbox"),
        "SandboxTemplate must not use the plain node image because bootstrap scripts must be baked into the sandbox image"
    );
    assert!(
        ASTRO_SANDBOX_DOCKERFILE.contains(
            "ARG SANDBOX_BASE_IMAGE=node:22-bookworm@sha256:5647be709086c696ff32edaaf1c70cd26d1da6ab2b39c32f3c7b4c4a31957e37",
        )
            && ASTRO_SANDBOX_DOCKERFILE.contains("FROM ${SANDBOX_BASE_IMAGE}")
            && ASTRO_SANDBOX_DOCKERFILE.contains("COPY base/workspace-init.sh")
            && ASTRO_SANDBOX_DOCKERFILE.contains("COPY base/workspace-channel-server.js")
            && ASTRO_SANDBOX_DOCKERFILE.contains("/opt/anydesign/bootstrap"),
        "Astro sandbox Dockerfile must bake the workspace init and channel server into /opt/anydesign/bootstrap"
    );
    assert!(
        !ASTRO_SANDBOX_DOCKERFILE.contains("COPY base/workspace-init.sh /workspace"),
        "bootstrap assets must not be baked under /workspace because the workspace volume can hide image files"
    );
}

#[test]
fn sandbox_template_mounts_pvc_backed_workspace() {
    let docs = yaml_documents(TEMPLATE_YAML);
    let template = named_doc(&docs, "SandboxTemplate", "anydesign-astro-website");
    let spec = field(template, "spec");
    let container = astro_template_container(template);
    let mounts = field(container, "volumeMounts").as_sequence().unwrap();
    let volume_claim_templates = field(spec, "volumeClaimTemplates").as_sequence().unwrap();

    assert_eq!(
        field(spec, "volumeClaimTemplatesPolicy").as_str(),
        Some("Allowed")
    );
    assert!(volume_claim_templates.iter().any(|template| {
        field(field(template, "metadata"), "name").as_str() == Some("workspace")
            && yaml_contains(template, "ReadWriteOnce")
            && yaml_contains(template, "5Gi")
    }));
    assert!(mounts.iter().any(|mount| {
        field(mount, "name").as_str() == Some("workspace")
            && field(mount, "mountPath").as_str() == Some("/workspace")
    }));
}

#[test]
fn workspace_init_precreates_required_workspace_layout() {
    for dir in [
        "/workspace/inputs",
        "/workspace/project",
        "/workspace/outputs/build",
        "/workspace/outputs/export",
        "/workspace/outputs/screenshots",
        "/workspace/outputs/reports",
        "/workspace/outputs/tool-results",
        "/workspace/state/checkpoints",
    ] {
        assert!(
            WORKSPACE_INIT_SH.contains(dir),
            "workspace-init must create {dir}"
        );
    }
    assert!(
        WORKSPACE_INIT_SH.contains("/workspace/state/tasks.json")
            && WORKSPACE_INIT_SH.contains("echo '[]'"),
        "workspace-init must create empty tasks.json"
    );
    assert!(
        WORKSPACE_INIT_SH.contains("/workspace/state/preview.json")
            && WORKSPACE_INIT_SH.contains("echo '{}'"),
        "workspace-init must create empty preview.json"
    );
}

#[test]
fn workspace_channel_server_supports_bounded_runtime_fs_protocol() {
    for op in [
        "fs.read",
        "fs.write",
        "fs.list",
        "fs.stat",
        "fs.removeFile",
        "fs.removeDirAll",
        "process.exec",
    ] {
        assert!(
            WORKSPACE_CHANNEL_SERVER_JS.contains(op),
            "workspace channel server must support {op}"
        );
    }
    assert!(
        WORKSPACE_CHANNEL_SERVER_JS.contains("path outside workspace")
            && WORKSPACE_CHANNEL_SERVER_JS.contains("secret path denied"),
        "workspace channel server must enforce workspace and secret-path boundaries"
    );
}

fn yaml_documents(input: &str) -> Vec<Value> {
    serde_yaml::Deserializer::from_str(input)
        .map(|doc| Value::deserialize(doc).unwrap())
        .collect()
}

fn named_doc<'a>(docs: &'a [Value], kind: &str, name: &str) -> &'a Value {
    docs.iter()
        .find(|doc| {
            field(doc, "kind").as_str() == Some(kind)
                && field(field(doc, "metadata"), "name").as_str() == Some(name)
        })
        .unwrap_or_else(|| panic!("{kind}/{name} not found"))
}

fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    field_optional(value, key).unwrap_or_else(|| panic!("missing field {key} in {value:?}"))
}

fn field_optional<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_mapping()?.get(Value::String(key.to_string()))
}

fn astro_template_container(template: &Value) -> &Value {
    let containers = field(
        field(field(field(template, "spec"), "podTemplate"), "spec"),
        "containers",
    )
    .as_sequence()
    .expect("podTemplate.spec.containers must be a list");

    containers
        .iter()
        .find(|container| field(container, "name").as_str() == Some("astro-website"))
        .expect("astro-website container must exist")
}

fn string_list(value: &Value) -> Vec<String> {
    value
        .as_sequence()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap().to_string())
        .collect()
}

fn assert_yaml_contains(value: &Value, needle: &str) {
    assert!(
        yaml_contains(value, needle),
        "expected YAML to contain {needle}: {value:?}"
    );
}

fn yaml_contains(value: &Value, needle: &str) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => value.to_string().contains(needle),
        Value::Number(value) => value.to_string().contains(needle),
        Value::String(value) => value.contains(needle),
        Value::Sequence(values) => values.iter().any(|value| yaml_contains(value, needle)),
        Value::Mapping(map) => map
            .iter()
            .any(|(key, value)| yaml_contains(key, needle) || yaml_contains(value, needle)),
        Value::Tagged(tagged) => yaml_contains(&tagged.value, needle),
    }
}
