#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cluster_name="${ZERONDESIGN_K3D_CLUSTER:-zerondesign-greenfield}"
workspace_a="${WORKSPACE_A:-ws-greenfield-a}"
workspace_b="${WORKSPACE_B:-ws-greenfield-b}"
https_port="${PUBLISHED_WORKS_HTTPS_PORT:-18443}"
base_domain="${PUBLISHED_WORKS_BASE_DOMAIN:-works.zerondesign.localhost}"
tls_secret_name="zerondesign-works-wildcard-tls"
source_evidence="${PUBLISHED_WORK_SOURCE_EVIDENCE:-${repo_root}/services/runtime/target/e2e-evidence/zerondesign-greenfield/published-works.json}"
evidence_dir="${repo_root}/services/runtime/target/e2e-evidence/zerondesign-greenfield"
tls_evidence="${evidence_dir}/published-works-tls.json"
ca_evidence="${evidence_dir}/published-works-test-ca.crt"
repository="zerondesign.local/generated-works"

for command in docker k3d kubectl cargo jq curl node openssl; do
  command -v "${command}" >/dev/null || {
    printf 'missing required command: %s\n' "${command}" >&2
    exit 2
  }
done
[[ -s "${source_evidence}" ]] || {
  printf 'Published Work source evidence is required: %s\n' "${source_evidence}" >&2
  exit 2
}
[[ "$(kubectl config current-context)" == "k3d-${cluster_name}" ]]

project_a="$(jq -r '.projects[0].projectId' "${source_evidence}")"
project_b="$(jq -r '.projects[1].projectId' "${source_evidence}")"
version_a="$(jq -r '.projects[0].versionId' "${source_evidence}")"
version_b="$(jq -r '.projects[1].versionId' "${source_evidence}")"
artifact_hash_a="$(jq -r '.projects[0].artifactManifestHash' "${source_evidence}")"
artifact_hash_b="$(jq -r '.projects[1].artifactManifestHash' "${source_evidence}")"
runtime_manifest_hash="$(jq -r '.projects[0].runtimeManifestHash' "${source_evidence}")"
release_a="$(jq -r '.projects[0].releaseId' "${source_evidence}")"
digest_a="$(jq -r '.projects[0].imageDigest' "${source_evidence}")"
digest_b="$(jq -r '.projects[1].imageDigest' "${source_evidence}")"

for hash in "${artifact_hash_a}" "${artifact_hash_b}" "${runtime_manifest_hash}"; do
  [[ "${hash}" =~ ^[a-f0-9]{64}$ ]] || {
    printf 'source evidence has an invalid manifest hash: %s\n' "${hash}" >&2
    exit 3
  }
done
for digest in "${digest_a}" "${digest_b}"; do
  [[ "${digest}" =~ ^sha256:[a-f0-9]{64}$ ]] || {
    printf 'source evidence has an invalid image digest: %s\n' "${digest}" >&2
    exit 3
  }
done

load_balancer="k3d-${cluster_name}-serverlb"
if ! docker port "${load_balancer}" | grep -Fq "127.0.0.1:${https_port}"; then
  k3d node edit "${load_balancer}" --port-add "127.0.0.1:${https_port}:443" >/dev/null
fi

work_dir="$(mktemp -d)"
cleanup() {
  rm -rf "${work_dir}"
}
trap cleanup EXIT
ca_key="${work_dir}/ca.key"
ca_cert="${work_dir}/ca.crt"
tls_key="${work_dir}/tls.key"
tls_csr="${work_dir}/tls.csr"
tls_cert="${work_dir}/tls.crt"
tls_ext="${work_dir}/tls.ext"

openssl req -x509 -newkey rsa:2048 -nodes -days 30 -subj '/CN=zeronDesign local Published Work CA' -keyout "${ca_key}" -out "${ca_cert}" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -subj "/CN=*.${base_domain}" -keyout "${tls_key}" -out "${tls_csr}" >/dev/null 2>&1
printf 'subjectAltName=DNS:*.%s\nextendedKeyUsage=serverAuth\n' "${base_domain}" >"${tls_ext}"
openssl x509 -req -sha256 -days 30 -in "${tls_csr}" -CA "${ca_cert}" -CAkey "${ca_key}" -CAcreateserial -extfile "${tls_ext}" -out "${tls_cert}" >/dev/null 2>&1
install -m 0644 "${ca_cert}" "${ca_evidence}"

for workspace in "${workspace_a}" "${workspace_b}"; do
  kubectl create secret tls "${tls_secret_name}" -n "${workspace}" --cert="${tls_cert}" --key="${tls_key}" --dry-run=client -o yaml | kubectl apply -f - >/dev/null
done

env WORKSPACE_PUBLICATION_PROJECT_A="${project_a}" WORKSPACE_PUBLICATION_PROJECT_B="${project_b}" WORKSPACE_PUBLICATION_NAMESPACE_A="${workspace_a}" WORKSPACE_PUBLICATION_NAMESPACE_B="${workspace_b}" WORKSPACE_PUBLICATION_IMAGE_REPOSITORY="${repository}" WORKSPACE_PUBLICATION_IMAGE_DIGEST_A="${digest_a}" WORKSPACE_PUBLICATION_IMAGE_DIGEST_B="${digest_b}" WORKSPACE_PUBLICATION_ARTIFACT_HASH_A="${artifact_hash_a}" WORKSPACE_PUBLICATION_ARTIFACT_HASH_B="${artifact_hash_b}" WORKSPACE_PUBLICATION_RUNTIME_MANIFEST_HASH="${runtime_manifest_hash}" WORKSPACE_PUBLICATION_VERSION_A="${version_a}" WORKSPACE_PUBLICATION_VERSION_B="${version_b}" WORKSPACE_PUBLICATION_EVIDENCE_PATH="${tls_evidence}" WORKSPACE_PUBLICATION_HTTPS_PORT="${https_port}" WORK_RUNTIME_PROBER_IMAGE="${repository}/${release_a}@${digest_a}" WORK_RUNTIME_EXPOSURE=ingress WORKS_BASE_DOMAIN="${base_domain}" WORKS_INGRESS_CLASS=traefik WORKS_TLS_SECRET_NAME="${tls_secret_name}" WORKS_PROBE_SCHEME=https WORKS_PROBE_RESOLVE="127.0.0.1:${https_port}" WORKS_PROBE_CA_FILE="${ca_cert}" RUN_WORKSPACE_PUBLICATION_E2E=1 cargo test --manifest-path "${repo_root}/services/runtime/Cargo.toml" --test k8s_workspace_publication_e2e -- --nocapture

verify_https_project() {
  local index="$1"
  local host release root_status release_status release_header release_body response_headers
  host="$(jq -r ".projects[${index}].hostSlug" "${tls_evidence}").${base_domain}"
  release="$(jq -r ".projects[${index}].releaseId" "${tls_evidence}")"
  root_status="$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' --cacert "${ca_cert}" --resolve "${host}:${https_port}:127.0.0.1" "https://${host}:${https_port}/")"
  response_headers="${work_dir}/headers-${index}"
  release_body="${work_dir}/release-${index}"
  release_status="$(curl --silent --show-error --output "${release_body}" --dump-header "${response_headers}" --write-out '%{http_code}' --cacert "${ca_cert}" --resolve "${host}:${https_port}:127.0.0.1" "https://${host}:${https_port}/.well-known/anydesign/release")"
  release_header="$(awk 'BEGIN{IGNORECASE=1}/^x-anydesign-release-id:/{gsub("\r","",$2);print $2}' "${response_headers}")"
  [[ "${root_status}" == "200" && "${release_status}" == "200" ]]
  [[ "${release_header}" == "${release}" ]]
  grep -F "${release}" "${release_body}" >/dev/null
}

verify_https_project 0
verify_https_project 1
kubectl -n anydesign-runtime rollout restart deployment/anydesign-runtime >/dev/null
kubectl -n anydesign-runtime rollout status deployment/anydesign-runtime --timeout=180s >/dev/null
verify_https_project 0
verify_https_project 1

ca_fingerprint="$(openssl x509 -in "${ca_cert}" -noout -fingerprint -sha256 | cut -d= -f2 | tr -d ':')"
ca_expires_at="$(openssl x509 -in "${ca_cert}" -noout -enddate | cut -d= -f2-)"
node - "${tls_evidence}" "${base_domain}" "${https_port}" "${ca_evidence}" "${ca_fingerprint}" "${ca_expires_at}" <<'NODE'
const {readFileSync,writeFileSync}=require("node:fs");
const [path,baseDomain,httpsPort,caFile,caFingerprint,caExpiresAt]=process.argv.slice(2);
const evidence=JSON.parse(readFileSync(path,"utf8"));
evidence.tls={
  mode:"local-test-ca",
  baseDomain,
  httpsPort:Number(httpsPort),
  caFile,
  caSha256Fingerprint:caFingerprint.toLowerCase(),
  caExpiresAt,
  caPrivateKeyPersisted:false,
  serverPrivateKeyStorage:"per-workspace-kubernetes-tls-secret",
};
evidence.runtimeRestartHttpsVerified=true;
evidence.projects=evidence.projects.map(project=>({
  ...project,
  externalHttpsVerified:true,
  releaseHeaderVerified:true,
}));
writeFileSync(path,`${JSON.stringify(evidence,null,2)}\n`);
NODE

printf 'PUBLISHED_WEBSITE_HTTPS_URL=%s\n' "$(jq -r '.projects[0].url' "${tls_evidence}")"
printf 'PUBLISHED_DOCS_HTTPS_URL=%s\n' "$(jq -r '.projects[1].url' "${tls_evidence}")"
printf 'PUBLISHED_WORK_TLS_CA=%s\n' "${ca_evidence}"
printf 'PUBLISHED_WORK_TLS_EVIDENCE=%s\n' "${tls_evidence}"
