#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cluster_name="${ZERONDESIGN_K3D_CLUSTER:-zerondesign-greenfield}"
runtime_namespace="${RUNTIME_SYSTEM_NAMESPACE:-anydesign-runtime}"
runtime_deployment="anydesign-runtime"
object_statefulset="anydesign-object-store"
object_bucket="anydesign-runtime"
object_prefix="greenfield"
runtime_storage="/var/lib/anydesign-runtime/data"
evidence_dir="${repo_root}/services/runtime/target/e2e-evidence/zerondesign-greenfield"
evidence_path="${evidence_dir}/object-storage.json"
tls_evidence="${evidence_dir}/published-works-tls.json"
tls_ca="${evidence_dir}/published-works-test-ca.crt"
stamp="$(date +%s)"

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
object_client_pod="object-storage-e2e-client-${stamp}"
cache_reset_pod="object-storage-e2e-cache-reset-${stamp}"
cleanup() {
  kubectl -n "${runtime_namespace}" delete pod \
    "${object_client_pod}" "${cache_reset_pod}" --ignore-not-found=true \
    --wait=false >/dev/null 2>&1 || true
  rm -rf "${work_dir}"
}
trap cleanup EXIT

if kubectl -n "${runtime_namespace}" get secret anydesign-runtime-object-storage >/dev/null 2>&1; then
  object_access_key="$(kubectl -n "${runtime_namespace}" \
    get secret anydesign-runtime-object-storage -o 'jsonpath={.data.access-key}' \
    | node -e 'process.stdin.on("data",d=>process.stdout.write(Buffer.from(d.toString(),"base64")))')"
  object_secret_key="$(kubectl -n "${runtime_namespace}" \
    get secret anydesign-runtime-object-storage -o 'jsonpath={.data.secret-key}' \
    | node -e 'process.stdin.on("data",d=>process.stdout.write(Buffer.from(d.toString(),"base64")))')"
else
  object_access_key="zerondesign$(openssl rand -hex 8)"
  object_secret_key="$(openssl rand -hex 24)"
fi
kubectl create secret generic anydesign-runtime-object-storage \
  -n "${runtime_namespace}" \
  --from-literal="access-key=${object_access_key}" \
  --from-literal="secret-key=${object_secret_key}" \
  --dry-run=client -o yaml | kubectl apply -f - >/dev/null
kubectl apply -f "${repo_root}/infra/workspace-provisioner/object-storage.yaml" >/dev/null
kubectl -n "${runtime_namespace}" rollout status \
  "statefulset/${object_statefulset}" --timeout=300s >/dev/null

kubectl apply -f - >/dev/null <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: ${object_client_pod}
  namespace: ${runtime_namespace}
  labels:
    app.kubernetes.io/managed-by: zerondesign-object-storage-e2e
spec:
  restartPolicy: Never
  securityContext:
    runAsUser: 1000
    runAsGroup: 1000
    seccompProfile:
      type: RuntimeDefault
  containers:
    - name: client
      image: quay.io/minio/mc:RELEASE.2025-08-13T08-35-41Z
      command: ["sh", "-ec"]
      args:
        - for attempt in \$(seq 1 60); do if mc alias set local http://anydesign-object-store.${runtime_namespace}.svc.cluster.local:9000 "\${MINIO_ACCESS_KEY}" "\${MINIO_SECRET_KEY}" && mc mb --ignore-existing local/${object_bucket}; then exec sleep 3600; fi; sleep 2; done; exit 1
      env:
        - name: HOME
          value: /tmp
        - name: MINIO_ACCESS_KEY
          valueFrom:
            secretKeyRef:
              name: anydesign-runtime-object-storage
              key: access-key
        - name: MINIO_SECRET_KEY
          valueFrom:
            secretKeyRef:
              name: anydesign-runtime-object-storage
              key: secret-key
      securityContext:
        allowPrivilegeEscalation: false
        capabilities:
          drop: ["ALL"]
EOF
kubectl -n "${runtime_namespace}" wait --for=condition=Ready \
  "pod/${object_client_pod}" --timeout=180s >/dev/null

if [[ -n "${RUNTIME_OBJECT_STORAGE_E2E_REUSE_IMAGE:-}" ]]; then
  runtime_image="${RUNTIME_OBJECT_STORAGE_E2E_REUSE_IMAGE}"
  docker image inspect "${runtime_image}" >/dev/null
else
  runtime_image="anydesign/runtime:object-storage-${stamp}"
  rm -rf "${repo_root}/services/runtime/target/docker-vendor"
  cargo vendor --manifest-path "${repo_root}/services/runtime/Cargo.toml" --locked --versioned-dirs \
    "${repo_root}/services/runtime/target/docker-vendor" >/dev/null
  docker build -f "${repo_root}/services/runtime/Dockerfile" --provenance=false \
    --build-arg "RUNTIME_IMAGE_REF=${runtime_image}" \
    --build-arg "REPOSITORY_COMMIT=object-storage-e2e" \
    --build-arg "REPOSITORY_DIRTY=true" \
    --build-arg "BUILD_TIMESTAMP=$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    -t "${runtime_image}" "${repo_root}" >/dev/null
fi
k3d image import --cluster "${cluster_name}" "${runtime_image}" >/dev/null

project_a="$(jq -r '.projects[0].projectId' "${tls_evidence}")"
project_b="$(jq -r '.projects[1].projectId' "${tls_evidence}")"
runtime_pod_before="$(kubectl -n "${runtime_namespace}" get pod -l app=anydesign-runtime \
  -o jsonpath='{.items[0].metadata.name}')"
runtime_uid_before="$(kubectl -n "${runtime_namespace}" get pod "${runtime_pod_before}" \
  -o jsonpath='{.metadata.uid}')"
project_tree_sha() {
  local pod="$1" project="$2"
  kubectl -n "${runtime_namespace}" exec "${pod}" -- sh -ec '
    root="$1/artifacts/$2"
    find "$root" -type f | sort | while IFS= read -r file; do
      sha256sum "$file"
    done | sha256sum | awk "{print \$1}"
  ' sh "${runtime_storage}" "${project}"
}
project_a_sha_before="$(project_tree_sha "${runtime_pod_before}" "${project_a}")"
project_b_sha_before="$(project_tree_sha "${runtime_pod_before}" "${project_b}")"

kubectl -n "${runtime_namespace}" set image \
  "deployment/${runtime_deployment}" "runtime=${runtime_image}" >/dev/null
kubectl -n "${runtime_namespace}" set env "deployment/${runtime_deployment}" \
  OBJECT_STORAGE_URL="s3://${object_bucket}/${object_prefix}" \
  OBJECT_STORAGE_ENDPOINT="http://anydesign-object-store.${runtime_namespace}.svc.cluster.local:9000" \
  OBJECT_STORAGE_REGION=us-east-1 OBJECT_STORAGE_ALLOW_HTTP=true >/dev/null
kubectl -n "${runtime_namespace}" patch "deployment/${runtime_deployment}" \
  --type=strategic -p "$(node <<'NODE'
process.stdout.write(JSON.stringify({spec:{template:{spec:{containers:[{name:'runtime',env:[
  {name:'OBJECT_STORAGE_ACCESS_KEY',valueFrom:{secretKeyRef:{name:'anydesign-runtime-object-storage',key:'access-key'}}},
  {name:'OBJECT_STORAGE_SECRET_KEY',valueFrom:{secretKeyRef:{name:'anydesign-runtime-object-storage',key:'secret-key'}}},
] }]}}}}));
NODE
)" >/dev/null
kubectl -n "${runtime_namespace}" rollout status \
  "deployment/${runtime_deployment}" --timeout=360s >/dev/null

runtime_pod_migrated="$(kubectl -n "${runtime_namespace}" get pod -l app=anydesign-runtime \
  -o jsonpath='{.items[0].metadata.name}')"
runtime_uid_migrated="$(kubectl -n "${runtime_namespace}" get pod "${runtime_pod_migrated}" \
  -o jsonpath='{.metadata.uid}')"
kubectl -n "${runtime_namespace}" exec "${object_client_pod}" -- \
  mc find "local/${object_bucket}/${object_prefix}" \
  | rg '/(artifacts|source-snapshots|validation-reports|acceptance-reports|screenshots)/' \
  | sort >"${work_dir}/object-keys"
object_count="$(wc -l <"${work_dir}/object-keys" | tr -d ' ')"
(( object_count > 0 ))
for boundary in artifacts source-snapshots validation-reports acceptance-reports screenshots; do
  boundary_count="$(rg -c "/${boundary}/" "${work_dir}/object-keys" || true)"
  printf '%s=%s\n' "${boundary}" "${boundary_count}" >>"${work_dir}/boundary-counts"
  (( boundary_count > 0 ))
done
rg -F "/artifacts/${project_a}/" "${work_dir}/object-keys" >/dev/null
rg -F "/artifacts/${project_b}/" "${work_dir}/object-keys" >/dev/null

kubectl -n "${runtime_namespace}" scale "deployment/${runtime_deployment}" --replicas=0 >/dev/null
kubectl -n "${runtime_namespace}" wait --for=delete pod -l app=anydesign-runtime \
  --timeout=180s >/dev/null
runtime_image_ref="$(kubectl -n "${runtime_namespace}" get deployment "${runtime_deployment}" \
  -o jsonpath='{.spec.template.spec.containers[?(@.name=="runtime")].image}')"
kubectl apply -f - >/dev/null <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: ${cache_reset_pod}
  namespace: ${runtime_namespace}
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
  "pod/${cache_reset_pod}" --timeout=180s >/dev/null
backup_root="${runtime_storage}/object-storage-e2e-cache-backup-${stamp}"
kubectl -n "${runtime_namespace}" exec "${cache_reset_pod}" -- sh -ec '
  mkdir -p "$2"
  for directory in artifacts source-snapshots validation-reports acceptance-reports screenshots; do
    if [ -d "$1/$directory" ]; then
      mv "$1/$directory" "$2/$directory"
    fi
  done
  for directory in artifacts source-snapshots validation-reports acceptance-reports screenshots; do
    [ ! -e "$1/$directory" ]
  done
' sh "${runtime_storage}" "${backup_root}"
kubectl -n "${runtime_namespace}" delete pod "${cache_reset_pod}" --wait=true >/dev/null
kubectl -n "${runtime_namespace}" scale "deployment/${runtime_deployment}" --replicas=1 >/dev/null
kubectl -n "${runtime_namespace}" rollout status \
  "deployment/${runtime_deployment}" --timeout=360s >/dev/null

runtime_pod_restored="$(kubectl -n "${runtime_namespace}" get pod -l app=anydesign-runtime \
  -o jsonpath='{.items[0].metadata.name}')"
runtime_uid_restored="$(kubectl -n "${runtime_namespace}" get pod "${runtime_pod_restored}" \
  -o jsonpath='{.metadata.uid}')"
project_a_sha_restored="$(project_tree_sha "${runtime_pod_restored}" "${project_a}")"
project_b_sha_restored="$(project_tree_sha "${runtime_pod_restored}" "${project_b}")"
[[ "${project_a_sha_restored}" == "${project_a_sha_before}" ]]
[[ "${project_b_sha_restored}" == "${project_b_sha_before}" ]]
restored_count="$(kubectl -n "${runtime_namespace}" exec "${runtime_pod_restored}" -- sh -ec '
  count=0
  for directory in artifacts source-snapshots validation-reports acceptance-reports screenshots; do
    current=$(find "$1/$directory" -type f | wc -l)
    count=$((count + current))
  done
  printf "%s" "$count"
' sh "${runtime_storage}")"
[[ "${restored_count}" == "${object_count}" ]]

object_pod="$(kubectl -n "${runtime_namespace}" get pod -l app=anydesign-object-store \
  -o jsonpath='{.items[0].metadata.name}')"
object_uid_before_restart="$(kubectl -n "${runtime_namespace}" get pod "${object_pod}" \
  -o jsonpath='{.metadata.uid}')"
kubectl -n "${runtime_namespace}" delete pod "${object_pod}" --wait=true >/dev/null
kubectl -n "${runtime_namespace}" rollout status \
  "statefulset/${object_statefulset}" --timeout=300s >/dev/null
object_pod="$(kubectl -n "${runtime_namespace}" get pod -l app=anydesign-object-store \
  -o jsonpath='{.items[0].metadata.name}')"
object_uid_after_restart="$(kubectl -n "${runtime_namespace}" get pod "${object_pod}" \
  -o jsonpath='{.metadata.uid}')"
[[ "${object_uid_before_restart}" != "${object_uid_after_restart}" ]]
kubectl -n "${runtime_namespace}" rollout restart "deployment/${runtime_deployment}" >/dev/null
kubectl -n "${runtime_namespace}" rollout status \
  "deployment/${runtime_deployment}" --timeout=360s >/dev/null
runtime_pod_final="$(kubectl -n "${runtime_namespace}" get pod -l app=anydesign-runtime \
  -o jsonpath='{.items[0].metadata.name}')"
project_a_sha_final="$(project_tree_sha "${runtime_pod_final}" "${project_a}")"
project_b_sha_final="$(project_tree_sha "${runtime_pod_final}" "${project_b}")"
[[ "${project_a_sha_final}" == "${project_a_sha_before}" ]]
[[ "${project_b_sha_final}" == "${project_b_sha_before}" ]]

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

node - "${evidence_path}" "${tls_evidence}" "${runtime_image}" "${object_count}" \
  "${restored_count}" "${work_dir}/boundary-counts" "${backup_root}" \
  "${runtime_uid_before}" "${runtime_uid_migrated}" "${runtime_uid_restored}" \
  "${object_uid_before_restart}" "${object_uid_after_restart}" \
  "${project_a_sha_before}" "${project_a_sha_restored}" "${project_a_sha_final}" \
  "${project_b_sha_before}" "${project_b_sha_restored}" "${project_b_sha_final}" <<'NODE'
const {readFileSync,writeFileSync}=require('node:fs');
const [path,tlsPath,runtimeImage,objectCount,restoredCount,boundaryPath,backupRoot,
  runtimeUidBefore,runtimeUidMigrated,runtimeUidRestored,objectUidBefore,objectUidAfter,
  projectASha,projectARestoredSha,projectAFinalSha,projectBSha,projectBRestoredSha,projectBFinalSha]=process.argv.slice(2);
const tls=JSON.parse(readFileSync(tlsPath,'utf8'));
const boundaryCounts=Object.fromEntries(readFileSync(boundaryPath,'utf8').trim().split('\n').map(line=>{
  const [key,value]=line.split('='); return [key,Number(value)];
}));
const hashes=[
  [projectASha,projectARestoredSha,projectAFinalSha],
  [projectBSha,projectBRestoredSha,projectBFinalSha],
];
const evidence={
  schemaVersion:'object-storage-e2e@1',
  generatedAt:new Date().toISOString(),
  objectStorage:{
    engine:'MinIO (S3-compatible)',
    bucket:'anydesign-runtime',
    prefix:'greenfield',
    credentialsPersistedInEvidence:false,
    objectCount:Number(objectCount),
    boundaryCounts,
    podUidBeforeRestart:objectUidBefore,
    podUidAfterRestart:objectUidAfter,
    persistentAfterPodRestart:objectUidBefore!==objectUidAfter,
  },
  runtime:{
    image:runtimeImage,
    podUidBeforeMigration:runtimeUidBefore,
    podUidAfterMigration:runtimeUidMigrated,
    podUidAfterCacheRestore:runtimeUidRestored,
    localObjectCacheMovedTo:backupRoot,
    restoredObjectCount:Number(restoredCount),
    objectStorageWasAuthoritative:Number(objectCount)===Number(restoredCount) && hashes.every(([a,b,c])=>a===b&&a===c),
  },
  projects:tls.projects.map((project,index)=>({
    projectId:project.projectId,
    workspaceNamespace:project.workspaceNamespace,
    releaseId:project.releaseId,
    status:project.status,
    url:project.url,
    artifactTreeSha256Before:hashes[index][0],
    artifactTreeSha256AfterCacheRestore:hashes[index][1],
    artifactTreeSha256AfterObjectStoreRestart:hashes[index][2],
    externalHttpsVerified:true,
    releaseIdentityVerified:true,
  })),
};
writeFileSync(path,`${JSON.stringify(evidence,null,2)}\n`);
NODE

printf 'OBJECT_STORAGE_EVIDENCE=%s\n' "${evidence_path}"
printf 'PUBLISHED_WEBSITE_HTTPS_URL=%s\n' "$(jq -r '.projects[0].url' "${tls_evidence}")"
printf 'PUBLISHED_DOCS_HTTPS_URL=%s\n' "$(jq -r '.projects[1].url' "${tls_evidence}")"
