#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cluster_name="${ZERONDESIGN_K3D_CLUSTER:-zerondesign-greenfield}"
runtime_namespace="${RUNTIME_SYSTEM_NAMESPACE:-anydesign-runtime}"
workspace_a="${WORKSPACE_A:-ws-greenfield-a}"
workspace_b="${WORKSPACE_B:-ws-greenfield-b}"
project_a="${PROJECT_A:-rc-website-1784368903}"
project_b="${PROJECT_B:-rc-docs-1784368745}"
version_a="${VERSION_A:-version-640}"
version_b="${VERSION_B:-version-475}"
artifact_hash_a="${ARTIFACT_HASH_A:-}"
artifact_hash_b="${ARTIFACT_HASH_B:-}"
public_port="${PUBLISHED_WORKS_PORT:-18080}"
repository="zerondesign.local/generated-works"
evidence_path="${repo_root}/services/runtime/target/e2e-evidence/zerondesign-greenfield/published-works.json"

for command in docker k3d kubectl cargo jq curl node; do
  command -v "${command}" >/dev/null || {
    printf 'missing required command: %s\n' "${command}" >&2
    exit 2
  }
done

[[ "$(kubectl config current-context)" == "k3d-${cluster_name}" ]] || {
  printf 'unexpected Kubernetes context: %s\n' "$(kubectl config current-context)" >&2
  exit 2
}

for workspace in "${workspace_a}" "${workspace_b}"; do
  RUNTIME_SYSTEM_NAMESPACE="${runtime_namespace}" \
    bash "${repo_root}/infra/workspace-provisioner/provision-workspace.sh" "${workspace}" >/dev/null
done

runtime_pod="$(kubectl -n "${runtime_namespace}" get pod -l app=anydesign-runtime -o jsonpath='{.items[0].metadata.name}')"
work_dir="$(mktemp -d)"
previous_evidence_path="${work_dir}/previous-published-works.json"
if [[ -s "${evidence_path}" ]]; then
  cp "${evidence_path}" "${previous_evidence_path}"
fi
cleanup() {
  rm -rf "${work_dir}"
}
trap cleanup EXIT

release_id() {
  node - "$1" "$2" <<'NODE'
const {createHash}=require('node:crypto');
const [artifactManifestHash,runtimeManifestHash]=process.argv.slice(2);
const fields=[artifactManifestHash,runtimeManifestHash,`sha256:${'c'.repeat(64)}`,'workspace-generated-e2e@1','workspace-generated-e2e-scan@1'];
const chunks=[];
for(const field of fields){const size=Buffer.alloc(8);size.writeBigUInt64BE(BigInt(Buffer.byteLength(field)));chunks.push(size,Buffer.from(field));}
process.stdout.write(`release-${createHash('sha256').update(Buffer.concat(chunks)).digest('hex').slice(0,32)}`);
NODE
}

build_generated_work() {
  local project="$1"
  local version="$2"
  local context_name="$3"
  local release="$4"
  local context="${work_dir}/${context_name}"
  local image="${repository}/${release}:latest"
  mkdir -p "${context}/public" "${context}/metadata"
  kubectl -n "${runtime_namespace}" cp \
    "${runtime_pod}:/var/lib/anydesign-runtime/data/artifacts/${project}/versions/${version}/." \
    "${context}/public" >/dev/null
  cp "${repo_root}/infra/published-runtime/static-web/Dockerfile" "${context}/Dockerfile"
  cp "${repo_root}/infra/published-runtime/static-web/nginx.conf" "${context}/nginx.conf"
  node - "${context}/metadata/release-provenance.json" "${release}" "${project}" "${version}" <<'NODE'
const {writeFileSync}=require('node:fs');
const [path,releaseId,projectId,versionId]=process.argv.slice(2);
writeFileSync(path,`${JSON.stringify({schemaVersion:'release-provenance@1',releaseId,projectId,versionId})}\n`);
NODE
  cp "${context}/public/.anydesign-artifact-manifest.json" \
    "${context}/metadata/artifact-manifest.json"
  node - "${context}/metadata/runtime-manifest.json" <<'NODE'
const {writeFileSync}=require('node:fs');
writeFileSync(process.argv[2],`${JSON.stringify({schemaVersion:'runtime-manifest@1',profile:'static-web-v1'})}\n`);
NODE
  docker build --platform linux/arm64 --provenance=false \
    --build-arg "RELEASE_ID=${release}" -t "${image}" "${context}" >/dev/null
  docker image inspect "${image}" --format '{{index .RepoDigests 0}}' | sed 's/^.*@//'
}

if [[ -z "${artifact_hash_a}" ]]; then
  artifact_hash_a="$(kubectl -n "${runtime_namespace}" exec "${runtime_pod}" -- \
    sha256sum "/var/lib/anydesign-runtime/data/artifacts/${project_a}/versions/${version_a}/.anydesign-artifact-manifest.json" \
    | awk '{print $1}')"
fi
if [[ -z "${artifact_hash_b}" ]]; then
  artifact_hash_b="$(kubectl -n "${runtime_namespace}" exec "${runtime_pod}" -- \
    sha256sum "/var/lib/anydesign-runtime/data/artifacts/${project_b}/versions/${version_b}/.anydesign-artifact-manifest.json" \
    | awk '{print $1}')"
fi
runtime_manifest_hash="$(node -e 'const {createHash}=require("node:crypto");const value=`${JSON.stringify({schemaVersion:"runtime-manifest@1",profile:"static-web-v1"})}\n`;process.stdout.write(createHash("sha256").update(value).digest("hex"))')"
for manifest_hash in "${artifact_hash_a}" "${artifact_hash_b}" "${runtime_manifest_hash}"; do
  [[ "${manifest_hash}" =~ ^[a-f0-9]{64}$ ]] || {
    printf 'invalid publication manifest hash: %s\n' "${manifest_hash}" >&2
    exit 3
  }
done
release_a="$(release_id "${artifact_hash_a}" "${runtime_manifest_hash}")"
release_b="$(release_id "${artifact_hash_b}" "${runtime_manifest_hash}")"
digest_a_file="${work_dir}/digest-a"
digest_b_file="${work_dir}/digest-b"
build_generated_work "${project_a}" "${version_a}" a "${release_a}" >"${digest_a_file}" &
build_a_pid=$!
build_generated_work "${project_b}" "${version_b}" b "${release_b}" >"${digest_b_file}" &
build_b_pid=$!
wait "${build_a_pid}"
wait "${build_b_pid}"
digest_a="$(cat "${digest_a_file}")"
digest_b="$(cat "${digest_b_file}")"

for digest in "${digest_a}" "${digest_b}"; do
  [[ "${digest}" =~ ^sha256:[a-f0-9]{64}$ ]] || {
    printf 'invalid generated work image digest: %s\n' "${digest}" >&2
    exit 3
  }
done

k3d image import \
  "${repository}/${release_a}:latest" \
  "${repository}/${release_b}:latest" \
  --cluster "${cluster_name}" >/dev/null

# k3d imports local images under their tag. Publication intentionally deploys
# digest-qualified references, so add the equivalent immutable aliases to each
# k3s containerd image store instead of weakening imagePullPolicy or trust checks.
while read -r node; do
  docker exec "${node}" ctr -n k8s.io images tag --force \
    "${repository}/${release_a}:latest" "${repository}/${release_a}@${digest_a}" >/dev/null
  docker exec "${node}" ctr -n k8s.io images tag --force \
    "${repository}/${release_b}:latest" "${repository}/${release_b}@${digest_b}" >/dev/null
done < <(k3d node list --no-headers \
  | awk -v cluster="${cluster_name}" '$3 == cluster && ($2 == "server" || $2 == "agent") {print $1}')

RUN_WORKSPACE_PUBLICATION_E2E=1 \
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
WORKSPACE_PUBLICATION_EVIDENCE_PATH="${evidence_path}" \
WORK_RUNTIME_PROBER_IMAGE="${repository}/${release_a}@${digest_a}" \
cargo test --manifest-path "${repo_root}/services/runtime/Cargo.toml" \
  --test k8s_workspace_publication_e2e -- --nocapture

service_a="$(jq -r '.projects[0].serviceName' "${evidence_path}")"
service_b="$(jq -r '.projects[1].serviceName' "${evidence_path}")"
host_a="website.zerondesign.localhost"
host_b="docs.zerondesign.localhost"

kubectl apply -f - >/dev/null <<EOF
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: zerondesign-generated-work-url
  namespace: ${workspace_a}
  labels:
    app.kubernetes.io/managed-by: zerondesign-e2e
spec:
  ingressClassName: traefik
  rules:
    - host: ${host_a}
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: ${service_a}
                port:
                  name: http
---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: zerondesign-generated-work-url
  namespace: ${workspace_b}
  labels:
    app.kubernetes.io/managed-by: zerondesign-e2e
spec:
  ingressClassName: traefik
  rules:
    - host: ${host_b}
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: ${service_b}
                port:
                  name: http
EOF

load_balancer="k3d-${cluster_name}-serverlb"
if ! docker port "${load_balancer}" | grep -Fq "127.0.0.1:${public_port}"; then
  k3d cluster edit "${cluster_name}" \
    --port-add "127.0.0.1:${public_port}:80@loadbalancer" >/dev/null
fi

url_a="http://${host_a}:${public_port}/"
url_b="http://${host_b}:${public_port}/"
for attempt in $(seq 1 30); do
  if curl --fail --silent --show-error "${url_a}" >/dev/null \
    && curl --fail --silent --show-error "${url_b}" >/dev/null; then
    break
  fi
  if [[ "${attempt}" == "30" ]]; then
    printf 'published work URLs did not become ready\n' >&2
    exit 4
  fi
  sleep 1
done

expected_a="$(kubectl -n "${runtime_namespace}" exec "${runtime_pod}" -- \
  sha256sum "/var/lib/anydesign-runtime/data/artifacts/${project_a}/versions/${version_a}/index.html" \
  | awk '{print $1}')"
expected_b="$(kubectl -n "${runtime_namespace}" exec "${runtime_pod}" -- \
  sha256sum "/var/lib/anydesign-runtime/data/artifacts/${project_b}/versions/${version_b}/index.html" \
  | awk '{print $1}')"
actual_a="$(curl --fail --silent --show-error "${url_a}" | shasum -a 256 | awk '{print $1}')"
actual_b="$(curl --fail --silent --show-error "${url_b}" | shasum -a 256 | awk '{print $1}')"
[[ "${actual_a}" == "${expected_a}" ]]
[[ "${actual_b}" == "${expected_b}" ]]

node - "${evidence_path}" "${url_a}" "${url_b}" "${expected_a}" "${expected_b}" <<'NODE'
const {readFileSync,writeFileSync}=require('node:fs');
const [path,urlA,urlB,hashA,hashB]=process.argv.slice(2);
const evidence=JSON.parse(readFileSync(path,'utf8'));
evidence.projects[0].url=urlA;
evidence.projects[0].externalArtifactSha256=hashA;
evidence.projects[0].externalHttpVerified=true;
evidence.projects[1].url=urlB;
evidence.projects[1].externalArtifactSha256=hashB;
evidence.projects[1].externalHttpVerified=true;
writeFileSync(path,`${JSON.stringify(evidence,null,2)}\n`);
NODE

kubectl -n "${runtime_namespace}" rollout restart deployment/anydesign-runtime >/dev/null
kubectl -n "${runtime_namespace}" rollout status deployment/anydesign-runtime --timeout=180s >/dev/null
curl --fail --silent --show-error "${url_a}" >/dev/null
curl --fail --silent --show-error "${url_b}" >/dev/null

if [[ -s "${previous_evidence_path}" ]]; then
  current_deployment_a="$(jq -r '.projects[0].deploymentName' "${evidence_path}")"
  current_deployment_b="$(jq -r '.projects[1].deploymentName' "${evidence_path}")"
  current_service_a="$(jq -r '.projects[0].serviceName' "${evidence_path}")"
  current_service_b="$(jq -r '.projects[1].serviceName' "${evidence_path}")"
  while IFS=$'\t' read -r old_namespace old_deployment old_service; do
    [[ -n "${old_namespace}" && -n "${old_deployment}" && -n "${old_service}" ]] || continue
    if [[ "${old_namespace}" == "${workspace_a}" ]]; then
      current_deployment="${current_deployment_a}"
      current_service="${current_service_a}"
    elif [[ "${old_namespace}" == "${workspace_b}" ]]; then
      current_deployment="${current_deployment_b}"
      current_service="${current_service_b}"
    else
      continue
    fi
    if [[ "${old_deployment}" != "${current_deployment}" ]] \
      && [[ "$(kubectl -n "${old_namespace}" get deployment "${old_deployment}" \
        -o 'jsonpath={.metadata.labels.app\.kubernetes\.io/managed-by}' 2>/dev/null || true)" \
        == "anydesign-work-runtime-controller" ]]; then
      kubectl -n "${old_namespace}" delete deployment "${old_deployment}" --wait=true >/dev/null
    fi
    if [[ "${old_service}" != "${current_service}" ]] \
      && [[ "$(kubectl -n "${old_namespace}" get service "${old_service}" \
        -o 'jsonpath={.metadata.labels.app\.kubernetes\.io/managed-by}' 2>/dev/null || true)" \
        == "anydesign-work-runtime-controller" ]]; then
      kubectl -n "${old_namespace}" delete service "${old_service}" --wait=true >/dev/null
      kubectl -n "${old_namespace}" delete networkpolicy "${old_service}" \
        --ignore-not-found=true --wait=true >/dev/null
    fi
  done < <(jq -r '.projects[] | [.workspaceNamespace,.deploymentName,.serviceName] | @tsv' \
    "${previous_evidence_path}")
fi

printf 'PUBLISHED_WEBSITE_URL=%s\n' "${url_a}"
printf 'PUBLISHED_DOCS_URL=%s\n' "${url_b}"
printf 'PUBLISHED_WORK_EVIDENCE=%s\n' "${evidence_path}"
