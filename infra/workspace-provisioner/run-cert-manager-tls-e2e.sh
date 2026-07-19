#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cluster_name="${ZERONDESIGN_K3D_CLUSTER:-zerondesign-greenfield}"
runtime_namespace="${RUNTIME_SYSTEM_NAMESPACE:-anydesign-runtime}"
workspace_a="${WORKSPACE_A:-ws-greenfield-a}"
workspace_b="${WORKSPACE_B:-ws-greenfield-b}"
base_domain="${PUBLISHED_WORKS_BASE_DOMAIN:-works.zerondesign.localhost}"
https_port="${PUBLISHED_WORKS_HTTPS_PORT:-18443}"
cluster_issuer_name="zerondesign-works-ca"
legacy_certificate_name="zerondesign-published-works"
legacy_tls_secret_name="zerondesign-works-wildcard-tls"
cert_manager_version="${CERT_MANAGER_VERSION:-v1.19.5}"
cmctl_version="${CMCTL_VERSION:-v2.5.0}"
evidence_dir="${repo_root}/services/runtime/target/e2e-evidence/zerondesign-greenfield"
published_evidence="${evidence_dir}/published-works-tls.json"
ca_evidence="${evidence_dir}/published-works-test-ca.crt"
evidence_path="${evidence_dir}/cert-manager-workspace-tls.json"
repository="zerondesign.local/generated-works"

for command in cargo kubectl jq curl node openssl sha256sum uname; do
  command -v "${command}" >/dev/null || {
    printf 'missing required command: %s\n' "${command}" >&2
    exit 2
  }
done
[[ "$(kubectl config current-context)" == "k3d-${cluster_name}" ]]
[[ -s "${published_evidence}" ]]

mkdir -p "${evidence_dir}"
work_dir="$(mktemp -d)"
cleanup() {
  rm -rf "${work_dir}"
}
trap cleanup EXIT

cert_manager_manifest="https://github.com/cert-manager/cert-manager/releases/download/${cert_manager_version}/cert-manager.yaml"
kubectl apply -f "${cert_manager_manifest}" >/dev/null
for deployment in cert-manager cert-manager-cainjector cert-manager-webhook; do
  kubectl -n cert-manager rollout status "deployment/${deployment}" --timeout=360s >/dev/null
done
if ! kubectl -n cert-manager get deployment cert-manager -o json \
  | jq -e '.spec.template.spec.containers[0].args | index("--enable-certificate-owner-ref=true")' \
  >/dev/null; then
  kubectl -n cert-manager patch deployment cert-manager --type=json \
    -p='[{"op":"add","path":"/spec/template/spec/containers/0/args/-","value":"--enable-certificate-owner-ref=true"}]' \
    >/dev/null
  kubectl -n cert-manager rollout status deployment/cert-manager --timeout=240s >/dev/null
fi

case "$(uname -s | tr '[:upper:]' '[:lower:]')" in
  darwin) cmctl_os="darwin" ;;
  linux) cmctl_os="linux" ;;
  *) printf 'unsupported cmctl operating system\n' >&2; exit 2 ;;
esac
case "$(uname -m)" in
  arm64|aarch64) cmctl_arch="arm64" ;;
  x86_64|amd64) cmctl_arch="amd64" ;;
  *) printf 'unsupported cmctl architecture\n' >&2; exit 2 ;;
esac
cmctl_bin="${work_dir}/cmctl"
curl --fail --silent --show-error --location \
  "https://github.com/cert-manager/cmctl/releases/download/${cmctl_version}/cmctl_${cmctl_os}_${cmctl_arch}" \
  --output "${cmctl_bin}"
chmod +x "${cmctl_bin}"
"${cmctl_bin}" check api --wait=2m >/dev/null

kubectl apply -f "${repo_root}/infra/workspace-provisioner/cert-manager-platform-ca.yaml" >/dev/null
kubectl -n cert-manager wait --for=condition=Ready \
  certificate/zerondesign-works-root-ca --timeout=240s >/dev/null
kubectl wait --for=condition=Ready \
  "clusterissuer/${cluster_issuer_name}" --timeout=240s >/dev/null

project_a="$(jq -r '.projects[0].projectId' "${published_evidence}")"
project_b="$(jq -r '.projects[1].projectId' "${published_evidence}")"
version_a="$(jq -r '.projects[0].versionId' "${published_evidence}")"
version_b="$(jq -r '.projects[1].versionId' "${published_evidence}")"
artifact_hash_a="$(jq -r '.projects[0].artifactManifestHash' "${published_evidence}")"
artifact_hash_b="$(jq -r '.projects[1].artifactManifestHash' "${published_evidence}")"
runtime_manifest_hash="$(jq -r '.projects[0].runtimeManifestHash' "${published_evidence}")"
digest_a="$(jq -r '.projects[0].imageDigest' "${published_evidence}")"
digest_b="$(jq -r '.projects[1].imageDigest' "${published_evidence}")"
release_a="$(jq -r '.projects[0].releaseId' "${published_evidence}")"

env \
  WORKSPACE_PUBLICATION_PROJECT_A="${project_a}" \
  WORKSPACE_PUBLICATION_PROJECT_B="${project_b}" \
  WORKSPACE_PUBLICATION_NAMESPACE_A="${workspace_a}" \
  WORKSPACE_PUBLICATION_NAMESPACE_B="${workspace_b}" \
  WORKSPACE_PUBLICATION_IMAGE_REPOSITORY="${repository}" \
  WORKSPACE_PUBLICATION_IMAGE_DIGEST_A="${digest_a}" \
  WORKSPACE_PUBLICATION_IMAGE_DIGEST_B="${digest_b}" \
  WORKSPACE_PUBLICATION_ARTIFACT_HASH_A="${artifact_hash_a}" \
  WORKSPACE_PUBLICATION_ARTIFACT_HASH_B="${artifact_hash_b}" \
  WORKSPACE_PUBLICATION_RUNTIME_MANIFEST_HASH="${runtime_manifest_hash}" \
  WORKSPACE_PUBLICATION_VERSION_A="${version_a}" \
  WORKSPACE_PUBLICATION_VERSION_B="${version_b}" \
  WORKSPACE_PUBLICATION_EVIDENCE_PATH="${published_evidence}" \
  WORKSPACE_PUBLICATION_HTTPS_PORT="${https_port}" \
  WORK_RUNTIME_PROBER_IMAGE="${repository}/${release_a}@${digest_a}" \
  WORK_RUNTIME_EXPOSURE=ingress \
  WORKS_BASE_DOMAIN="${base_domain}" \
  WORKS_INGRESS_CLASS=traefik \
  WORKS_CERTIFICATE_ISSUER_NAME="${cluster_issuer_name}" \
  WORKS_PROBE_SCHEME=https \
  WORKS_PROBE_RESOLVE="127.0.0.1:${https_port}" \
  WORKS_PROBE_CA_FILE="${ca_evidence}" \
  RUN_WORKSPACE_PUBLICATION_E2E=1 \
  cargo test --manifest-path "${repo_root}/services/runtime/Cargo.toml" \
    --test k8s_workspace_publication_e2e -- --nocapture >/dev/null

ingress_a="$(jq -r '.projects[0].ingressName' "${published_evidence}")"
ingress_b="$(jq -r '.projects[1].ingressName' "${published_evidence}")"
certificate_a="${ingress_a}-tls"
certificate_b="${ingress_b}-tls"
secret_a="${certificate_a}"
secret_b="${certificate_b}"

for pair in "${workspace_a}:${certificate_a}" "${workspace_b}:${certificate_b}"; do
  workspace="${pair%%:*}"
  certificate="${pair#*:}"
  kubectl -n "${workspace}" wait --for=condition=Ready \
    "certificate/${certificate}" --timeout=240s >/dev/null
done

[[ "$(kubectl -n "${workspace_a}" get ingress "${ingress_a}" \
  -o jsonpath='{.metadata.annotations.cert-manager\.io/cluster-issuer}')" == "${cluster_issuer_name}" ]]
[[ "$(kubectl -n "${workspace_b}" get ingress "${ingress_b}" \
  -o jsonpath='{.metadata.annotations.cert-manager\.io/cluster-issuer}')" == "${cluster_issuer_name}" ]]
[[ "$(kubectl -n "${workspace_a}" get ingress "${ingress_a}" \
  -o jsonpath='{.spec.tls[0].secretName}')" == "${secret_a}" ]]
[[ "$(kubectl -n "${workspace_b}" get ingress "${ingress_b}" \
  -o jsonpath='{.spec.tls[0].secretName}')" == "${secret_b}" ]]

for workspace in "${workspace_a}" "${workspace_b}"; do
  kubectl -n "${workspace}" delete certificate "${legacy_certificate_name}" \
    --ignore-not-found=true --wait=true >/dev/null
  kubectl -n "${workspace}" delete secret "${legacy_tls_secret_name}" \
    --ignore-not-found=true --wait=true >/dev/null
done

decode_secret_key() {
  local namespace="$1" secret="$2" key="$3"
  kubectl -n "${namespace}" get secret "${secret}" -o json \
    | KEY="${key}" node -e '
      let input="";
      process.stdin.on("data", chunk => input += chunk);
      process.stdin.on("end", () => {
        const value=JSON.parse(input).data?.[process.env.KEY];
        if (!value) process.exit(2);
        process.stdout.write(Buffer.from(value,"base64"));
      });
    '
}

certificate_revision() {
  kubectl -n "$1" get certificate "$2" -o jsonpath='{.status.revision}'
}

secret_hash() {
  decode_secret_key "$1" "$2" "$3" | sha256sum | awk '{print $1}'
}

write_leaf_certificate() {
  decode_secret_key "$1" "$2" tls.crt >"$3"
}

write_ca_certificate() {
  decode_secret_key "$1" "$2" ca.crt >"$3"
}

leaf_fingerprint() {
  openssl x509 -in "$1" -noout -fingerprint -sha256 \
    | cut -d= -f2 | tr -d ':' | tr '[:upper:]' '[:lower:]'
}

revision_a_before="$(certificate_revision "${workspace_a}" "${certificate_a}")"
revision_b_before="$(certificate_revision "${workspace_b}" "${certificate_b}")"
key_a_before="$(secret_hash "${workspace_a}" "${secret_a}" tls.key)"
key_b_before="$(secret_hash "${workspace_b}" "${secret_b}" tls.key)"
cert_a_before_hash="$(secret_hash "${workspace_a}" "${secret_a}" tls.crt)"
cert_b_before_hash="$(secret_hash "${workspace_b}" "${secret_b}" tls.crt)"
[[ "${key_a_before}" != "${key_b_before}" ]]

write_ca_certificate "${workspace_a}" "${secret_a}" "${work_dir}/ca-a.crt"
write_ca_certificate "${workspace_b}" "${secret_b}" "${work_dir}/ca-b.crt"
cmp "${work_dir}/ca-a.crt" "${work_dir}/ca-b.crt" >/dev/null
install -m 0644 "${work_dir}/ca-a.crt" "${ca_evidence}"
write_leaf_certificate "${workspace_a}" "${secret_a}" "${work_dir}/leaf-a-before.crt"
write_leaf_certificate "${workspace_b}" "${secret_b}" "${work_dir}/leaf-b-before.crt"
openssl verify -CAfile "${ca_evidence}" "${work_dir}/leaf-a-before.crt" >/dev/null
openssl verify -CAfile "${ca_evidence}" "${work_dir}/leaf-b-before.crt" >/dev/null
fingerprint_a_before="$(leaf_fingerprint "${work_dir}/leaf-a-before.crt")"
fingerprint_b_before="$(leaf_fingerprint "${work_dir}/leaf-b-before.crt")"
[[ "${fingerprint_a_before}" != "${fingerprint_b_before}" ]]

"${cmctl_bin}" renew -n "${workspace_a}" "${certificate_a}" >/dev/null
"${cmctl_bin}" renew -n "${workspace_b}" "${certificate_b}" >/dev/null

wait_for_rotation() {
  local workspace="$1" certificate="$2" secret="$3" old_revision="$4"
  local old_key_hash="$5" old_cert_hash="$6" revision key_hash cert_hash
  for attempt in $(seq 1 180); do
    revision="$(certificate_revision "${workspace}" "${certificate}" 2>/dev/null || true)"
    key_hash="$(secret_hash "${workspace}" "${secret}" tls.key 2>/dev/null || true)"
    cert_hash="$(secret_hash "${workspace}" "${secret}" tls.crt 2>/dev/null || true)"
    if [[ "${revision}" =~ ^[0-9]+$ ]] \
      && (( revision > old_revision )) \
      && [[ "${key_hash}" != "${old_key_hash}" ]] \
      && [[ "${cert_hash}" != "${old_cert_hash}" ]]; then
      return 0
    fi
    [[ "${attempt}" != "180" ]] || {
      printf 'certificate rotation timed out for %s/%s\n' "${workspace}" "${certificate}" >&2
      return 1
    }
    sleep 1
  done
}
wait_for_rotation "${workspace_a}" "${certificate_a}" "${secret_a}" \
  "${revision_a_before}" "${key_a_before}" "${cert_a_before_hash}"
wait_for_rotation "${workspace_b}" "${certificate_b}" "${secret_b}" \
  "${revision_b_before}" "${key_b_before}" "${cert_b_before_hash}"

revision_a_after="$(certificate_revision "${workspace_a}" "${certificate_a}")"
revision_b_after="$(certificate_revision "${workspace_b}" "${certificate_b}")"
key_a_after="$(secret_hash "${workspace_a}" "${secret_a}" tls.key)"
key_b_after="$(secret_hash "${workspace_b}" "${secret_b}" tls.key)"
[[ "${key_a_after}" != "${key_a_before}" ]]
[[ "${key_b_after}" != "${key_b_before}" ]]
[[ "${key_a_after}" != "${key_b_after}" ]]
write_leaf_certificate "${workspace_a}" "${secret_a}" "${work_dir}/leaf-a-after.crt"
write_leaf_certificate "${workspace_b}" "${secret_b}" "${work_dir}/leaf-b-after.crt"
fingerprint_a_after="$(leaf_fingerprint "${work_dir}/leaf-a-after.crt")"
fingerprint_b_after="$(leaf_fingerprint "${work_dir}/leaf-b-after.crt")"
openssl verify -CAfile "${ca_evidence}" "${work_dir}/leaf-a-after.crt" >/dev/null
openssl verify -CAfile "${ca_evidence}" "${work_dir}/leaf-b-after.crt" >/dev/null

runtime_can_read_a="$(kubectl auth can-i get secrets -n "${workspace_a}" \
  --as="system:serviceaccount:${runtime_namespace}:anydesign-runtime" || true)"
runtime_can_read_b="$(kubectl auth can-i get secrets -n "${workspace_b}" \
  --as="system:serviceaccount:${runtime_namespace}:anydesign-runtime" || true)"
sandbox_a_can_read_b="$(kubectl auth can-i get secrets -n "${workspace_b}" \
  --as="system:serviceaccount:${workspace_a}:anydesign-sandbox" || true)"
[[ "${runtime_can_read_a}" == "no" && "${runtime_can_read_b}" == "no" ]]
[[ "${sandbox_a_can_read_b}" == "no" ]]
for workspace in "${workspace_a}" "${workspace_b}"; do
  if kubectl -n "${workspace}" get secret zerondesign-works-root-ca >/dev/null 2>&1; then
    printf 'root CA private-key Secret must not exist in %s\n' "${workspace}" >&2
    exit 6
  fi
done

verify_https_project() {
  local index="$1" expected_fingerprint="$2" host release headers body served_fingerprint
  host="$(jq -r ".projects[${index}].hostSlug" "${published_evidence}").${base_domain}"
  release="$(jq -r ".projects[${index}].releaseId" "${published_evidence}")"
  headers="${work_dir}/headers-${index}"
  body="${work_dir}/release-${index}"
  for attempt in $(seq 1 60); do
    served_fingerprint="$(openssl s_client -connect "127.0.0.1:${https_port}" \
      -servername "${host}" </dev/null 2>/dev/null \
      | openssl x509 -noout -fingerprint -sha256 2>/dev/null \
      | cut -d= -f2 | tr -d ':' | tr '[:upper:]' '[:lower:]' || true)"
    if [[ "${served_fingerprint}" == "${expected_fingerprint}" ]]; then
      break
    fi
    [[ "${attempt}" != "60" ]] || {
      printf 'Ingress did not serve the expected certificate for %s\n' "${host}" >&2
      return 1
    }
    sleep 1
  done
  [[ "$(curl --silent --show-error --dump-header "${headers}" --output "${body}" \
    --write-out '%{http_code}' --cacert "${ca_evidence}" \
    --resolve "${host}:${https_port}:127.0.0.1" \
    "https://${host}:${https_port}/.well-known/anydesign/release")" == "200" ]]
  awk 'BEGIN{IGNORECASE=1}/^x-anydesign-release-id:/{gsub("\r","",$2);print $2}' "${headers}" \
    | grep -Fx "${release}" >/dev/null
  grep -F "${release}" "${body}" >/dev/null
}
verify_https_project 0 "${fingerprint_a_after}"
verify_https_project 1 "${fingerprint_b_after}"

controller_pod="$(kubectl -n cert-manager get pod -l app.kubernetes.io/component=controller \
  -o jsonpath='{.items[0].metadata.name}')"
controller_uid_before="$(kubectl -n cert-manager get pod "${controller_pod}" \
  -o jsonpath='{.metadata.uid}')"
kubectl -n cert-manager delete pod "${controller_pod}" --wait=true >/dev/null
kubectl -n cert-manager rollout status deployment/cert-manager --timeout=240s >/dev/null
controller_pod="$(kubectl -n cert-manager get pod -l app.kubernetes.io/component=controller \
  -o jsonpath='{.items[0].metadata.name}')"
controller_uid_after="$(kubectl -n cert-manager get pod "${controller_pod}" \
  -o jsonpath='{.metadata.uid}')"
[[ "${controller_uid_after}" != "${controller_uid_before}" ]]
kubectl -n "${workspace_a}" wait --for=condition=Ready \
  "certificate/${certificate_a}" --timeout=120s >/dev/null
kubectl -n "${workspace_b}" wait --for=condition=Ready \
  "certificate/${certificate_b}" --timeout=120s >/dev/null
verify_https_project 0 "${fingerprint_a_after}"
verify_https_project 1 "${fingerprint_b_after}"

ca_fingerprint="$(openssl x509 -in "${ca_evidence}" -noout -fingerprint -sha256 \
  | cut -d= -f2 | tr -d ':' | tr '[:upper:]' '[:lower:]')"
ca_expires_at="$(openssl x509 -in "${ca_evidence}" -noout -enddate | cut -d= -f2-)"
kubernetes_version="$(kubectl version -o json | jq -r '.serverVersion.gitVersion')"

node - "${evidence_path}" "${published_evidence}" "${ca_evidence}" \
  "${cert_manager_version}" "${cmctl_version}" "${kubernetes_version}" \
  "${cluster_issuer_name}" "${base_domain}" "${https_port}" "${ca_fingerprint}" \
  "${ca_expires_at}" "${workspace_a}" "${certificate_a}" "${secret_a}" \
  "${revision_a_before}" "${revision_a_after}" "${fingerprint_a_before}" \
  "${fingerprint_a_after}" "${workspace_b}" "${certificate_b}" "${secret_b}" \
  "${revision_b_before}" "${revision_b_after}" "${fingerprint_b_before}" \
  "${fingerprint_b_after}" "${controller_uid_before}" "${controller_uid_after}" <<'NODE'
const {readFileSync,writeFileSync}=require('node:fs');
const [path,publishedPath,caFile,certManagerVersion,cmctlVersion,kubernetesVersion,
  clusterIssuer,baseDomain,httpsPort,caFingerprint,caExpiresAt,
  workspaceA,certificateA,secretA,revisionABefore,revisionAAfter,fingerprintABefore,fingerprintAAfter,
  workspaceB,certificateB,secretB,revisionBBefore,revisionBAfter,fingerprintBBefore,fingerprintBAfter,
  controllerUidBefore,controllerUidAfter]=process.argv.slice(2);
const published=JSON.parse(readFileSync(publishedPath,'utf8'));
const projects=published.projects.map(({projectId,workspaceNamespace,url,releaseId})=>({
  projectId,workspaceNamespace,url,releaseId,externalHttpsVerified:true,releaseIdentityVerified:true,
}));
const workspaces=[
  {namespace:workspaceA,certificateName:certificateA,secretName:secretA,
    revisionBefore:Number(revisionABefore),revisionAfter:Number(revisionAAfter),
    leafFingerprintBefore:fingerprintABefore,leafFingerprintAfter:fingerprintAAfter,
    privateKeyRotated:true,certificateRotated:fingerprintABefore!==fingerprintAAfter,
    servedCertificateMatchesSecret:true},
  {namespace:workspaceB,certificateName:certificateB,secretName:secretB,
    revisionBefore:Number(revisionBBefore),revisionAfter:Number(revisionBAfter),
    leafFingerprintBefore:fingerprintBBefore,leafFingerprintAfter:fingerprintBAfter,
    privateKeyRotated:true,certificateRotated:fingerprintBBefore!==fingerprintBAfter,
    servedCertificateMatchesSecret:true},
];
const evidence={
  schemaVersion:'cert-manager-workspace-tls-e2e@2',generatedAt:new Date().toISOString(),
  certManager:{version:certManagerVersion,cmctlVersion,kubernetesVersion,
    certificateOwnerReferencesEnabled:true,
    controllerPodUidBeforeRestart:controllerUidBefore,
    controllerPodUidAfterRestart:controllerUidAfter,
    controllerRestartVerified:controllerUidBefore!==controllerUidAfter},
  issuer:{name:clusterIssuer,kind:'ClusterIssuer',localTestImplementation:'CA',
    productionContract:'ACME, Vault, or managed cloud CA ClusterIssuer',
    localCaIsNotProductionPki:true},
  ca:{file:caFile,sha256Fingerprint:caFingerprint,expiresAt:caExpiresAt,
    privateKeyPersistedInEvidence:false,privateKeyStorage:'cert-manager/zerondesign-works-root-ca'},
  certificate:{scope:'per-published-work',baseDomain,httpsPort:Number(httpsPort),
    duration:'2160h',renewBefore:'720h',privateKeyAlgorithm:'ECDSA-256',
    rotationPolicy:'Always',legacySharedWildcardSecretRemoved:true},
  isolation:{runtimeCanReadWorkspaceTlsSecrets:false,crossWorkspaceSandboxCanReadTlsSecret:false,
    workPrivateKeysDistinct:true,rootCaPrivateKeyAbsentFromWorkspaces:true},
  workspaces,projects,
};
writeFileSync(path,`${JSON.stringify(evidence,null,2)}\n`);
published.tls={mode:'cert-manager-local-ca-per-work',baseDomain,httpsPort:Number(httpsPort),caFile,
  caSha256Fingerprint:caFingerprint,caExpiresAt,caPrivateKeyPersistedInEvidence:false,
  caPrivateKeyStorage:'cert-manager namespace Secret',serverPrivateKeyStorage:'per-work Kubernetes TLS Secret',
  serverPrivateKeysDistinct:true,rotationPolicy:'Always',certManagerVersion,clusterIssuer};
published.certManagerRotationVerified=true;
published.projects=published.projects.map((project,index)=>({...project,
  tlsCertificateName:workspaces[index].certificateName,tlsSecretName:workspaces[index].secretName,
  servedCertificateFingerprint:workspaces[index].leafFingerprintAfter,
  externalHttpsVerified:true,releaseHeaderVerified:true,certificateRotationVerified:true}));
writeFileSync(publishedPath,`${JSON.stringify(published,null,2)}\n`);
NODE

jq -e '
  .certManager.controllerRestartVerified == true and
  .certManager.certificateOwnerReferencesEnabled == true and
  .certificate.scope == "per-published-work" and
  .certificate.legacySharedWildcardSecretRemoved == true and
  .isolation.runtimeCanReadWorkspaceTlsSecrets == false and
  .isolation.crossWorkspaceSandboxCanReadTlsSecret == false and
  .isolation.workPrivateKeysDistinct == true and
  ([.workspaces[].privateKeyRotated] | all) and
  ([.workspaces[].servedCertificateMatchesSecret] | all) and
  ([.projects[].externalHttpsVerified] | all)
' "${evidence_path}" >/dev/null

printf 'cert-manager per-Work TLS E2E passed. Evidence: %s\n' "${evidence_path}"
jq -r '.projects[].url' "${evidence_path}"
