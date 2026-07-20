#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
cluster_name="${GENERATION_REAL_CLUSTER:-zerondesign-greenfield}"
context="k3d-${cluster_name}"
runtime_namespace="${RUNTIME_NAMESPACE:-anydesign-runtime}"
workspace_namespace="${GENERATION_REAL_WORKSPACE_NAMESPACE:?GENERATION_REAL_WORKSPACE_NAMESPACE is required}"
project_id="${GENERATION_REAL_PROJECT_ID:?GENERATION_REAL_PROJECT_ID is required}"
version_id="${GENERATION_REAL_VERSION_ID:?GENERATION_REAL_VERSION_ID is required}"
run_id="${GENERATION_REAL_RUN_ID:?GENERATION_REAL_RUN_ID is required}"
expected_text="${GENERATION_REAL_EXPECTED_TEXT:?GENERATION_REAL_EXPECTED_TEXT is required}"
publication_path="${GENERATION_REAL_PUBLICATION_PATH:-/}"
base_domain="${PUBLISHED_WORKS_BASE_DOMAIN:-works.zerondesign.localhost}"
https_port="${PUBLISHED_WORKS_HTTPS_PORT:-18443}"
cluster_issuer="${WORKS_CERTIFICATE_ISSUER_NAME:-zerondesign-works-ca}"
ca_file="${PUBLISHED_WORKS_CA_FILE:-${ROOT_DIR}/services/runtime/target/e2e-evidence/${cluster_name}/published-works-test-ca.crt}"
evidence_file="${GENERATION_REAL_PUBLICATION_EVIDENCE_FILE:-${ROOT_DIR}/services/runtime/target/e2e-evidence/${cluster_name}/real-provider-runs/validation-publication.json}"

for command in "${KUBECTL}" docker k3d jq node curl sha256sum; do
  command -v "${command}" >/dev/null || {
    printf 'real_provider_publication.missing_command: %s\n' "${command}" >&2
    exit 2
  }
done
[[ "$(${KUBECTL} config current-context)" == "${context}" ]]
[[ -s "${ca_file}" ]]
case "${publication_path}" in
  /*) ;;
  *)
    printf 'GENERATION_REAL_PUBLICATION_PATH must start with /\n' >&2
    exit 2
    ;;
esac
if [[ "${publication_path}" == *..* || "${publication_path}" == *\?* || "${publication_path}" == *\#* ]]; then
  printf 'GENERATION_REAL_PUBLICATION_PATH must not contain traversal, query, or fragment syntax\n' >&2
  exit 2
fi
workspace_label="$(${KUBECTL} --context "${context}" get namespace "${workspace_namespace}" \
  -o jsonpath='{.metadata.labels.zerondesign\.dev/workspace}')"
[[ "${workspace_label}" == "true" ]] || {
  printf 'validation publication requires a managed Workspace namespace\n' >&2
  exit 2
}

project_hash="$(printf '%s' "${project_id}" | sha256sum | awk '{print $1}')"
runtime_config_hash="$({
  sha256sum "${ROOT_DIR}/infra/published-runtime/static-web/Dockerfile"
  sha256sum "${ROOT_DIR}/infra/published-runtime/static-web/nginx.conf"
} | sha256sum | awk '{print $1}')"
work_name="real-work-${project_hash:0:12}"
host_slug="real-${project_hash:0:20}"
host="${host_slug}.${base_domain}"
artifact_root="/var/lib/anydesign-runtime/data/artifacts/${project_id}/versions/${version_id}"

work_dir="$(mktemp -d)"
cleanup() {
  find "${work_dir}" -type f -delete 2>/dev/null || true
  find "${work_dir}" -depth -type d -empty -delete 2>/dev/null || true
}
trap cleanup EXIT
mkdir -p "${work_dir}/context/public" "${work_dir}/context/metadata"

runtime_pod="$(${KUBECTL} --context "${context}" -n "${runtime_namespace}" \
  get pod -l app=anydesign-runtime -o json | jq -r '
    [.items[] | select(
      .metadata.deletionTimestamp == null and
      any(.status.conditions[]?; .type == "Ready" and .status == "True")
    ) | .metadata.name] | if length == 1 then .[0] else empty end
  ')"
[[ -n "${runtime_pod}" ]] || {
  printf 'validation publication requires exactly one Ready Runtime Pod\n' >&2
  exit 3
}
${KUBECTL} --context "${context}" -n "${runtime_namespace}" exec "${runtime_pod}" -- \
  test -s "${artifact_root}/.anydesign-artifact-manifest.json"
${KUBECTL} --context "${context}" -n "${runtime_namespace}" cp \
  "${runtime_pod}:${artifact_root}/." "${work_dir}/context/public"

manifest_file="${work_dir}/context/public/.anydesign-artifact-manifest.json"
artifact_manifest_hash="$(jq -r '.candidateManifestHash' "${manifest_file}")"
[[ "${artifact_manifest_hash}" =~ ^[a-f0-9]{64}$ ]]
release_hash="$(printf '%s' "${project_id}:${version_id}:${run_id}:${artifact_manifest_hash}" \
  | sha256sum | awk '{print $1}')"
release_id="release-real-${release_hash:0:24}"
deployment_name="${work_name}-${release_hash:0:8}"
image_tag="zerondesign/real-provider-work:${project_hash:0:8}-${artifact_manifest_hash:0:8}-${runtime_config_hash:0:8}"
node - "${manifest_file}" "${work_dir}/context/public" <<'NODE'
const fs = require('node:fs');
const path = require('node:path');
const [manifestFile, publicRoot] = process.argv.slice(2);
const manifest = JSON.parse(fs.readFileSync(manifestFile, 'utf8'));
for (const file of manifest.files || []) {
  const target = path.resolve(publicRoot, file.path);
  if (!target.startsWith(`${path.resolve(publicRoot)}${path.sep}`) || !fs.statSync(target).isFile()) {
    throw new Error(`artifact file escapes or is missing: ${file.path}`);
  }
}
fs.rmSync(manifestFile);
NODE

cp "${ROOT_DIR}/infra/published-runtime/static-web/Dockerfile" "${work_dir}/context/Dockerfile"
cp "${ROOT_DIR}/infra/published-runtime/static-web/nginx.conf" "${work_dir}/context/nginx.conf"
node - "${work_dir}/context/metadata/release-provenance.json" \
  "${release_id}" "${project_id}" "${version_id}" "${run_id}" \
  "${artifact_manifest_hash}" <<'NODE'
const fs = require('node:fs');
const [file, releaseId, projectId, versionId, runId, artifactManifestHash] = process.argv.slice(2);
fs.writeFileSync(file, `${JSON.stringify({
  schemaVersion: 'release-provenance@1',
  releaseId,
  projectId,
  versionId,
  runId,
  artifactManifestHash,
  publicationMode: 'real-provider-validation',
}, null, 2)}\n`);
NODE

docker build --build-arg "RELEASE_ID=${release_id}" -t "${image_tag}" \
  "${work_dir}/context" >/dev/null
image_config_digest="$(docker image inspect "${image_tag}" --format '{{.Id}}')"
k3d image import -c "${cluster_name}" "${image_tag}" >/dev/null

${KUBECTL} --context "${context}" -n "${workspace_namespace}" apply -f - >/dev/null <<YAML
apiVersion: apps/v1
kind: Deployment
metadata:
  name: ${deployment_name}
  labels: &labels
    app.kubernetes.io/managed-by: real-provider-validation
    anydesign.dev/work: ${work_name}
    anydesign.dev/project: ${project_id}
    anydesign.dev/release-id: ${release_id}
spec:
  replicas: 1
  strategy:
    type: Recreate
  selector:
    matchLabels:
      anydesign.dev/work: ${work_name}
      anydesign.dev/release-id: ${release_id}
  template:
    metadata:
      labels: *labels
    spec:
      automountServiceAccountToken: false
      securityContext:
        runAsNonRoot: true
        seccompProfile:
          type: RuntimeDefault
      containers:
        - name: work
          image: ${image_tag}
          imagePullPolicy: IfNotPresent
          ports:
            - name: http
              containerPort: 8080
          readinessProbe:
            httpGet:
              path: /.well-known/anydesign/healthz
              port: http
            periodSeconds: 2
            failureThreshold: 30
          livenessProbe:
            httpGet:
              path: /.well-known/anydesign/healthz
              port: http
            periodSeconds: 10
          resources:
            requests:
              cpu: 10m
              memory: 16Mi
            limits:
              cpu: 250m
              memory: 128Mi
          securityContext:
            allowPrivilegeEscalation: false
            readOnlyRootFilesystem: true
            capabilities:
              drop: ["ALL"]
          volumeMounts:
            - name: tmp
              mountPath: /tmp
      volumes:
        - name: tmp
          emptyDir:
            sizeLimit: 16Mi
---
apiVersion: v1
kind: Service
metadata:
  name: ${work_name}
  labels:
    app.kubernetes.io/managed-by: real-provider-validation
    anydesign.dev/work: ${work_name}
    anydesign.dev/project: ${project_id}
    anydesign.dev/release-id: ${release_id}
spec:
  type: ClusterIP
  selector:
    anydesign.dev/work: ${work_name}
    anydesign.dev/release-id: ${release_id}
  ports:
    - name: http
      port: 80
      targetPort: http
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: ${work_name}
spec:
  podSelector:
    matchLabels:
      anydesign.dev/work: ${work_name}
  policyTypes: ["Ingress", "Egress"]
  ingress:
    - from:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: kube-system
          podSelector:
            matchLabels:
              app.kubernetes.io/name: traefik
      ports:
        - protocol: TCP
          port: 8080
---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: ${work_name}
  labels:
    app.kubernetes.io/managed-by: real-provider-validation
    anydesign.dev/work: ${work_name}
    anydesign.dev/project: ${project_id}
    anydesign.dev/release-id: ${release_id}
  annotations:
    anydesign.dev/release-id: ${release_id}
    cert-manager.io/cluster-issuer: ${cluster_issuer}
    cert-manager.io/duration: 2160h
    cert-manager.io/renew-before: 720h
    cert-manager.io/private-key-algorithm: ECDSA
    cert-manager.io/private-key-size: "256"
    cert-manager.io/private-key-rotation-policy: Always
    cert-manager.io/revision-history-limit: "3"
    nginx.ingress.kubernetes.io/ssl-redirect: "true"
spec:
  ingressClassName: traefik
  tls:
    - hosts: ["${host}"]
      secretName: ${work_name}-tls
  rules:
    - host: ${host}
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: ${work_name}
                port:
                  name: http
YAML

${KUBECTL} --context "${context}" -n "${workspace_namespace}" rollout status \
  "deployment/${deployment_name}" --timeout=240s >/dev/null
${KUBECTL} --context "${context}" -n "${workspace_namespace}" wait \
  --for=condition=Ready "certificate/${work_name}-tls" --timeout=240s >/dev/null

headers="${work_dir}/headers"
body="${work_dir}/body"
release_body="${work_dir}/release-body"
base_url="https://${host}:${https_port}/"
url="https://${host}:${https_port}${publication_path}"
[[ "$(curl --silent --show-error --dump-header "${headers}" --output "${body}" \
  --write-out '%{http_code}' --cacert "${ca_file}" \
  --resolve "${host}:${https_port}:127.0.0.1" "${url}")" == "200" ]]
grep -F "${expected_text}" "${body}" >/dev/null
[[ "$(curl --silent --show-error --output "${release_body}" --write-out '%{http_code}' \
  --cacert "${ca_file}" --resolve "${host}:${https_port}:127.0.0.1" \
  "${base_url}.well-known/anydesign/release")" == "200" ]]
grep -F "${release_id}" "${release_body}" >/dev/null
release_header="$(awk 'BEGIN{IGNORECASE=1}/^x-anydesign-release-id:/{gsub("\r","",$2);print $2}' "${headers}")"
[[ "${release_header}" == "${release_id}" ]]

certificate_owner_kind="$(${KUBECTL} --context "${context}" -n "${workspace_namespace}" \
  get certificate "${work_name}-tls" -o jsonpath='{.metadata.ownerReferences[0].kind}')"
secret_owner_kind="$(${KUBECTL} --context "${context}" -n "${workspace_namespace}" \
  get secret "${work_name}-tls" -o jsonpath='{.metadata.ownerReferences[0].kind}')"
[[ "${certificate_owner_kind}" == "Ingress" && "${secret_owner_kind}" == "Certificate" ]]

mkdir -p "$(dirname "${evidence_file}")"
node - "${evidence_file}" "${project_id}" "${workspace_namespace}" "${version_id}" \
  "${run_id}" "${artifact_manifest_hash}" "${release_id}" "${image_tag}" \
  "${image_config_digest}" "${work_name}" "${deployment_name}" "${host}" "${url}" \
  "${expected_text}" <<'NODE'
const fs = require('node:fs');
const [file, projectId, workspaceNamespace, versionId, runId, artifactManifestHash,
  releaseId, imageTag, imageConfigDigest, workName, deploymentName, host, url, expectedText] = process.argv.slice(2);
const evidence = {
  schemaVersion: 'real-provider-validation-publication@1',
  recordedAt: new Date().toISOString(),
  projectId,
  workspaceNamespace,
  versionId,
  runId,
  artifactManifestHash,
  releaseId,
  publicationMode: 'validation',
  productReleaseApiCompleted: false,
  productReleaseApiPendingReason: 'deployed Runtime image has no Release Packager helper',
  image: {tag: imageTag, configDigest: imageConfigDigest, digestPinnedDeployment: false},
  kubernetes: {workName, deploymentName, ingressName: workName, certificateName: `${workName}-tls`,
    certificateOwnedByIngress: true, tlsSecretOwnedByCertificate: true},
  external: {host, url, httpStatus: 200, httpsVerified: true,
    expectedText, expectedTextFound: true, releaseIdentityVerified: true},
  secretMaterialPersisted: false,
  passed: true,
};
fs.writeFileSync(file, `${JSON.stringify(evidence, null, 2)}\n`, {mode: 0o600});
NODE

printf 'Real Provider validation publication passed: %s\n' "${url}"
printf 'Evidence: %s\n' "${evidence_file}"
