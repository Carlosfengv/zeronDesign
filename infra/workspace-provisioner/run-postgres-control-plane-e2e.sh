#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cluster_name="${ZERONDESIGN_K3D_CLUSTER:-zerondesign-greenfield}"
runtime_namespace="${RUNTIME_SYSTEM_NAMESPACE:-anydesign-runtime}"
runtime_deployment="anydesign-runtime"
postgres_statefulset="anydesign-postgres"
evidence_dir="${repo_root}/services/runtime/target/e2e-evidence/zerondesign-greenfield"
evidence_path="${evidence_dir}/postgres-control-plane.json"
tls_evidence="${evidence_dir}/published-works-tls.json"
tls_ca="${evidence_dir}/published-works-test-ca.crt"
runtime_storage="/var/lib/anydesign-runtime/data"

for command in cargo docker k3d kubectl jq curl node openssl sha256sum; do
  command -v "${command}" >/dev/null || {
    printf 'missing required command: %s\n' "${command}" >&2
    exit 2
  }
done
[[ "$(kubectl config current-context)" == "k3d-${cluster_name}" ]]
[[ -s "${tls_evidence}" && -s "${tls_ca}" ]]

mkdir -p "${evidence_dir}"
work_dir="$(mktemp -d)"
port_forward_pid=""
cleanup() {
  if [[ -n "${port_forward_pid}" ]]; then
    kill "${port_forward_pid}" 2>/dev/null || true
    wait "${port_forward_pid}" 2>/dev/null || true
  fi
  rm -rf "${work_dir}"
}
trap cleanup EXIT

if kubectl -n "${runtime_namespace}" get secret anydesign-runtime-postgres >/dev/null 2>&1; then
  postgres_password="$(kubectl -n "${runtime_namespace}" get secret anydesign-runtime-postgres \
    -o 'jsonpath={.data.password}' | node -e 'process.stdin.on("data",d=>process.stdout.write(Buffer.from(d.toString(),"base64")))')"
else
  postgres_password="$(openssl rand -hex 24)"
fi
postgres_url="postgres://anydesign_runtime:${postgres_password}@anydesign-postgres.${runtime_namespace}.svc.cluster.local:5432/anydesign_runtime"
kubectl create secret generic anydesign-runtime-postgres \
  -n "${runtime_namespace}" \
  --from-literal="password=${postgres_password}" \
  --from-literal="url=${postgres_url}" \
  --dry-run=client -o yaml | kubectl apply -f - >/dev/null
kubectl apply -f "${repo_root}/infra/workspace-provisioner/postgres-control-plane.yaml" >/dev/null
kubectl -n "${runtime_namespace}" rollout status \
  "statefulset/${postgres_statefulset}" --timeout=240s >/dev/null

if [[ -n "${RUNTIME_POSTGRES_E2E_REUSE_IMAGE:-}" ]]; then
  runtime_image="${RUNTIME_POSTGRES_E2E_REUSE_IMAGE}"
  docker image inspect "${runtime_image}" >/dev/null
  image_tag="${runtime_image##*:}"
else
  image_tag="postgres-control-plane-$(date +%s)"
  runtime_image="anydesign/runtime:${image_tag}"
  rm -rf "${repo_root}/services/runtime/target/docker-vendor"
  cargo vendor --manifest-path "${repo_root}/services/runtime/Cargo.toml" --locked --versioned-dirs \
    "${repo_root}/services/runtime/target/docker-vendor" >/dev/null
  docker build -f "${repo_root}/services/runtime/Dockerfile" --provenance=false \
    --build-arg "RUNTIME_IMAGE_REF=${runtime_image}" \
    --build-arg "REPOSITORY_COMMIT=postgres-control-plane-e2e" \
    --build-arg "REPOSITORY_DIRTY=true" \
    --build-arg "BUILD_TIMESTAMP=$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    -t "${runtime_image}" "${repo_root}" >/dev/null
  k3d image import --cluster "${cluster_name}" "${runtime_image}" >/dev/null
fi

runtime_uid_before="$(kubectl -n "${runtime_namespace}" get pod -l app=anydesign-runtime \
  -o jsonpath='{.items[0].metadata.uid}')"
kubectl -n "${runtime_namespace}" set image \
  "deployment/${runtime_deployment}" "runtime=${runtime_image}" >/dev/null
kubectl -n "${runtime_namespace}" patch "deployment/${runtime_deployment}" \
  --type=strategic -p "$(node <<'NODE'
process.stdout.write(JSON.stringify({spec:{template:{spec:{containers:[{name:'runtime',env:[{name:'DATABASE_URL',valueFrom:{secretKeyRef:{name:'anydesign-runtime-postgres',key:'url'}}}]}]}}}}));
NODE
)" >/dev/null
kubectl -n "${runtime_namespace}" rollout status \
  "deployment/${runtime_deployment}" --timeout=240s >/dev/null

postgres_pod="$(kubectl -n "${runtime_namespace}" get pod \
  -l app=anydesign-postgres -o jsonpath='{.items[0].metadata.name}')"
psql_query() {
  kubectl -n "${runtime_namespace}" exec "${postgres_pod}" -- \
    psql -U anydesign_runtime -d anydesign_runtime -Atqc "$1"
}

for attempt in $(seq 1 60); do
  file_count="$(psql_query 'SELECT count(*) FROM runtime_control_plane_files' 2>/dev/null || true)"
  if [[ "${file_count}" =~ ^[0-9]+$ ]] && (( file_count > 0 )); then
    break
  fi
  if [[ "${attempt}" == "60" ]]; then
    printf 'Runtime did not import control-plane files into PostgreSQL\n' >&2
    exit 3
  fi
  sleep 1
done

project_a="$(jq -r '.projects[0].projectId' "${tls_evidence}")"
project_b="$(jq -r '.projects[1].projectId' "${tls_evidence}")"
for project in "${project_a}" "${project_b}"; do
  matches="$(psql_query "SELECT count(*) FROM runtime_control_plane_files WHERE convert_from(content, 'UTF8') LIKE '%${project}%'" )"
  (( matches > 0 ))
done
db_access_sha="$(psql_query "SELECT content_sha256 FROM runtime_control_plane_files WHERE file_path='project-access.jsonl'")"
[[ "${db_access_sha}" =~ ^[a-f0-9]{64}$ ]]

runtime_image_ref="$(kubectl -n "${runtime_namespace}" get deployment "${runtime_deployment}" \
  -o jsonpath='{.spec.template.spec.containers[?(@.name=="runtime")].image}')"
kubectl -n "${runtime_namespace}" scale "deployment/${runtime_deployment}" --replicas=0 >/dev/null
kubectl -n "${runtime_namespace}" wait --for=delete pod -l app=anydesign-runtime --timeout=120s >/dev/null

cache_reset_pod="runtime-postgres-cache-reset-${image_tag##*-}"
kubectl apply -f - >/dev/null <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: ${cache_reset_pod}
  namespace: ${runtime_namespace}
  labels:
    app.kubernetes.io/managed-by: zerondesign-postgres-e2e
spec:
  restartPolicy: Never
  securityContext:
    runAsUser: 10001
    runAsGroup: 10001
    seccompProfile:
      type: RuntimeDefault
  containers:
    - name: cache-reset
      image: ${runtime_image_ref}
      command: ["sh", "-c", "sleep 600"]
      securityContext:
        allowPrivilegeEscalation: false
        capabilities:
          drop: ["ALL"]
      volumeMounts:
        - name: storage
          mountPath: ${runtime_storage}
  volumes:
    - name: storage
      persistentVolumeClaim:
        claimName: anydesign-runtime-storage
EOF
kubectl -n "${runtime_namespace}" wait --for=condition=Ready \
  "pod/${cache_reset_pod}" --timeout=120s >/dev/null
backup_root="${runtime_storage}/postgres-e2e-cache-backup-${image_tag##*-}"
psql_query 'SELECT file_path FROM runtime_control_plane_files ORDER BY file_path' \
  >"${work_dir}/control-plane-paths"
while IFS= read -r relative; do
  [[ -n "${relative}" && "${relative}" != /* && "${relative}" != *..* ]]
  kubectl -n "${runtime_namespace}" exec "${cache_reset_pod}" -- sh -ec '
    source_path="$1/$2"
    backup_path="$3/$2"
    if [ -f "$source_path" ]; then
      mkdir -p "$(dirname "$backup_path")"
      mv "$source_path" "$backup_path"
    fi
  ' sh "${runtime_storage}" "${relative}" "${backup_root}"
done <"${work_dir}/control-plane-paths"
kubectl -n "${runtime_namespace}" delete pod "${cache_reset_pod}" --wait=true >/dev/null
kubectl -n "${runtime_namespace}" scale "deployment/${runtime_deployment}" --replicas=1 >/dev/null
kubectl -n "${runtime_namespace}" rollout status \
  "deployment/${runtime_deployment}" --timeout=240s >/dev/null

runtime_pod="$(kubectl -n "${runtime_namespace}" get pod -l app=anydesign-runtime \
  -o jsonpath='{.items[0].metadata.name}')"
runtime_uid_after_cache_restore="$(kubectl -n "${runtime_namespace}" get pod "${runtime_pod}" \
  -o jsonpath='{.metadata.uid}')"
restored_access_sha="$(kubectl -n "${runtime_namespace}" exec "${runtime_pod}" -- \
  sha256sum "${runtime_storage}/project-access.jsonl" | awk '{print $1}')"
[[ "${restored_access_sha}" == "${db_access_sha}" ]]

postgres_uid_before="$(kubectl -n "${runtime_namespace}" get pod "${postgres_pod}" \
  -o jsonpath='{.metadata.uid}')"
kubectl -n "${runtime_namespace}" delete pod "${postgres_pod}" --wait=true >/dev/null
kubectl -n "${runtime_namespace}" rollout status \
  "statefulset/${postgres_statefulset}" --timeout=240s >/dev/null
postgres_pod="$(kubectl -n "${runtime_namespace}" get pod \
  -l app=anydesign-postgres -o jsonpath='{.items[0].metadata.name}')"
postgres_uid_after="$(kubectl -n "${runtime_namespace}" get pod "${postgres_pod}" \
  -o jsonpath='{.metadata.uid}')"
[[ "${postgres_uid_after}" != "${postgres_uid_before}" ]]
[[ "$(psql_query 'SELECT count(*) FROM runtime_control_plane_files')" == "${file_count}" ]]

audit_revision_before="$(psql_query "SELECT COALESCE(revision, 0) FROM runtime_control_plane_files WHERE file_path='audit-log.jsonl'")"
admin_token="$(kubectl -n "${runtime_namespace}" get secret anydesign-runtime-internal-admin \
  -o 'jsonpath={.data.token}' | node -e 'process.stdin.on("data",d=>process.stdout.write(Buffer.from(d.toString(),"base64")))')"
kubectl -n "${runtime_namespace}" port-forward service/anydesign-runtime 18082:8080 \
  >"${work_dir}/runtime-port-forward.log" 2>&1 &
port_forward_pid=$!
for attempt in $(seq 1 30); do
  curl --fail --silent http://127.0.0.1:18082/health >/dev/null 2>&1 && break
  [[ "${attempt}" != "30" ]] || exit 4
  sleep 1
done
curl --fail --silent --show-error -X PUT \
  -H 'x-anydesign-internal: true' \
  -H "x-runtime-admin-token: ${admin_token}" \
  -H 'content-type: application/json' \
  --data "{\"ownerPrincipalId\":\"rc-harness-principal\",\"workspaceNamespace\":\"ws-greenfield-a\"}" \
  "http://127.0.0.1:18082/internal/projects/${project_a}/access" \
  >"${work_dir}/project-access-response.json"
[[ "$(jq -r '.projectAccess.workspaceNamespace' "${work_dir}/project-access-response.json")" == "ws-greenfield-a" ]]
audit_revision_after="$(psql_query "SELECT revision FROM runtime_control_plane_files WHERE file_path='audit-log.jsonl'")"
(( audit_revision_after > audit_revision_before ))

verify_https_project() {
  local index="$1" host release headers body
  host="$(jq -r ".projects[${index}].hostSlug" "${tls_evidence}").works.zerondesign.localhost"
  release="$(jq -r ".projects[${index}].releaseId" "${tls_evidence}")"
  headers="${work_dir}/headers-${index}"
  body="${work_dir}/release-${index}"
  [[ "$(curl --silent --show-error --output /dev/null --write-out '%{http_code}' \
    --cacert "${tls_ca}" --resolve "${host}:18443:127.0.0.1" "https://${host}:18443/")" == "200" ]]
  [[ "$(curl --silent --show-error --dump-header "${headers}" --output "${body}" \
    --write-out '%{http_code}' --cacert "${tls_ca}" --resolve "${host}:18443:127.0.0.1" \
    "https://${host}:18443/.well-known/anydesign/release")" == "200" ]]
  awk 'BEGIN{IGNORECASE=1}/^x-anydesign-release-id:/{gsub("\r","",$2);print $2}' "${headers}" \
    | grep -Fx "${release}" >/dev/null
  grep -F "${release}" "${body}" >/dev/null
}
verify_https_project 0
verify_https_project 1

node - "${evidence_path}" "${tls_evidence}" "${runtime_image}" "${file_count}" \
  "${db_access_sha}" "${restored_access_sha}" "${runtime_uid_before}" \
  "${runtime_uid_after_cache_restore}" "${postgres_uid_before}" "${postgres_uid_after}" \
  "${backup_root}" "${audit_revision_before}" "${audit_revision_after}" <<'NODE'
const {readFileSync,writeFileSync}=require('node:fs');
const [path,tlsEvidencePath,runtimeImage,fileCount,dbAccessSha,restoredAccessSha,runtimeUidBefore,runtimeUidAfter,postgresUidBefore,postgresUidAfter,backupRoot,auditRevisionBefore,auditRevisionAfter]=process.argv.slice(2);
const tls=JSON.parse(readFileSync(tlsEvidencePath,'utf8'));
const evidence={
  schemaVersion:'postgres-control-plane-e2e@1',
  generatedAt:new Date().toISOString(),
  database:{
    engine:'postgresql',
    credentialsPersistedInEvidence:false,
    controlPlaneFileCount:Number(fileCount),
    projectAccessSha256:dbAccessSha,
    postgresPodUidBeforeRestart:postgresUidBefore,
    postgresPodUidAfterRestart:postgresUidAfter,
    postgresRestartRecovered:postgresUidBefore!==postgresUidAfter,
    auditRevisionBeforeReconnect:Number(auditRevisionBefore),
    auditRevisionAfterReconnect:Number(auditRevisionAfter),
    writeAfterPostgresRestartVerified:Number(auditRevisionAfter)>Number(auditRevisionBefore),
  },
  runtime:{
    image:runtimeImage,
    podUidBeforeMigration:runtimeUidBefore,
    podUidAfterCacheRestore:runtimeUidAfter,
    localControlPlaneCacheMovedTo:backupRoot,
    databaseRestoreSha256:restoredAccessSha,
    databaseWasAuthoritative:dbAccessSha===restoredAccessSha,
  },
  projects:tls.projects.map(project=>({
    projectId:project.projectId,
    workspaceNamespace:project.workspaceNamespace,
    releaseId:project.releaseId,
    status:project.status,
    url:project.url,
    externalHttpsVerified:true,
    releaseIdentityVerified:true,
  })),
};
writeFileSync(path,`${JSON.stringify(evidence,null,2)}\n`);
NODE

printf 'POSTGRES_CONTROL_PLANE_EVIDENCE=%s\n' "${evidence_path}"
printf 'PUBLISHED_WEBSITE_HTTPS_URL=%s\n' "$(jq -r '.projects[0].url' "${tls_evidence}")"
printf 'PUBLISHED_DOCS_HTTPS_URL=%s\n' "$(jq -r '.projects[1].url' "${tls_evidence}")"
