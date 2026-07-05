use serde::Deserialize;
use serde_yaml::Value;

const NETWORK_POLICY_YAML: &str =
    include_str!("../../../infra/agent-sandbox/network/default-deny.yaml");
const RBAC_YAML: &str =
    include_str!("../../../infra/agent-sandbox/rbac/runtime-service-account.yaml");
const TEMPLATE_YAML: &str =
    include_str!("../../../infra/agent-sandbox/astro-website/sandbox-template.yaml");
const ASTRO_SANDBOX_DOCKERFILE: &str =
    include_str!("../../../infra/agent-sandbox/astro-website/Dockerfile");
const WORKSPACE_INIT_SH: &str = include_str!("../../../infra/agent-sandbox/base/workspace-init.sh");
const WORKSPACE_CHANNEL_SERVER_JS: &str =
    include_str!("../../../infra/agent-sandbox/base/workspace-channel-server.js");

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
fn sandbox_network_policy_only_allows_runtime_ingress_and_dns_egress() {
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

    for doc in docs
        .iter()
        .filter(|doc| field(doc, "kind").as_str() == Some("NetworkPolicy"))
    {
        if let Some(egress) = field_optional(field(doc, "spec"), "egress") {
            assert!(
                yaml_contains(egress, "kube-dns"),
                "egress policy must only target DNS: {doc:?}"
            );
        }
    }
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
        ASTRO_SANDBOX_DOCKERFILE.contains("FROM node:22-bookworm")
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
