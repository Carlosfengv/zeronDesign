#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cluster_name="${ZERONDESIGN_K3D_CLUSTER:-zerondesign-greenfield}"
runtime_namespace="${RUNTIME_SYSTEM_NAMESPACE:-anydesign-runtime}"
postgres_statefulset="anydesign-postgres"
web_deployment="zerondesign-web"
evidence_dir="${repo_root}/services/runtime/target/e2e-evidence/zerondesign-greenfield"
evidence_path="${evidence_dir}/web-product-catalog-postgres.json"
tls_evidence="${evidence_dir}/published-works-tls.json"
tls_ca="${evidence_dir}/published-works-test-ca.crt"
stamp="$(date +%s)"

for command in docker k3d kubectl jq curl node openssl sha256sum; do
  command -v "${command}" >/dev/null || {
    printf 'missing required command: %s\n' "${command}" >&2
    exit 2
  }
done
[[ "$(kubectl config current-context)" == "k3d-${cluster_name}" ]]
[[ -s "${tls_evidence}" && -s "${tls_ca}" ]]
kubectl -n "${runtime_namespace}" get secret \
  zerondesign-web-runtime-principal >/dev/null

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

decode_secret() {
  kubectl -n "${runtime_namespace}" get secret "$1" -o "jsonpath={.data.$2}" \
    | node -e 'process.stdin.on("data",d=>process.stdout.write(Buffer.from(d.toString(),"base64")))'
}

postgres_pod="$(kubectl -n "${runtime_namespace}" get pod \
  -l app=anydesign-postgres -o jsonpath='{.items[0].metadata.name}')"
postgres_password="$(decode_secret anydesign-runtime-postgres password)"
if kubectl -n "${runtime_namespace}" get secret zerondesign-web-product-postgres >/dev/null 2>&1; then
  web_password="$(decode_secret zerondesign-web-product-postgres password)"
else
  web_password="$(openssl rand -hex 24)"
fi
web_database="zerondesign_web"
web_role="zerondesign_web"
web_database_url="postgres://${web_role}:${web_password}@anydesign-postgres.${runtime_namespace}.svc.cluster.local:5432/${web_database}"

kubectl -n "${runtime_namespace}" exec -i "${postgres_pod}" -- \
  env "WEB_PASSWORD=${web_password}" psql -U anydesign_runtime -d anydesign_runtime \
  -v ON_ERROR_STOP=1 --set=web_password="${web_password}" >/dev/null <<'SQL'
SELECT format('CREATE ROLE zerondesign_web LOGIN PASSWORD %L', :'web_password')
WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = 'zerondesign_web') \gexec
ALTER ROLE zerondesign_web PASSWORD :'web_password';
SELECT 'CREATE DATABASE zerondesign_web OWNER anydesign_runtime'
WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = 'zerondesign_web') \gexec
SQL

kubectl -n "${runtime_namespace}" exec -i "${postgres_pod}" -- \
  psql -U anydesign_runtime -d "${web_database}" -v ON_ERROR_STOP=1 \
  <"${repo_root}/apps/web/migrations/0001_product_catalog.sql" >/dev/null
kubectl -n "${runtime_namespace}" exec -i "${postgres_pod}" -- \
  psql -U anydesign_runtime -d "${web_database}" -v ON_ERROR_STOP=1 >/dev/null <<'SQL'
REVOKE CREATE ON SCHEMA public FROM PUBLIC;
GRANT USAGE ON SCHEMA public TO zerondesign_web;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO zerondesign_web;
ALTER DEFAULT PRIVILEGES FOR ROLE anydesign_runtime IN SCHEMA public
  GRANT SELECT, INSERT, UPDATE, DELETE ON TABLES TO zerondesign_web;
SQL

kubectl create secret generic zerondesign-web-product-postgres \
  -n "${runtime_namespace}" \
  --from-literal="password=${web_password}" \
  --from-literal="url=${web_database_url}" \
  --dry-run=client -o yaml | kubectl apply -f - >/dev/null
if kubectl -n "${runtime_namespace}" get secret zerondesign-web-auth >/dev/null 2>&1; then
  session_secret="$(decode_secret zerondesign-web-auth session-secret)"
else
  session_secret="$(openssl rand -hex 32)"
fi
kubectl create secret generic zerondesign-web-auth \
  -n "${runtime_namespace}" \
  --from-literal="session-secret=${session_secret}" \
  --dry-run=client -o yaml | kubectl apply -f - >/dev/null

kubectl apply -f "${repo_root}/infra/workspace-provisioner/postgres-control-plane.yaml" >/dev/null
if [[ -n "${WEB_PRODUCT_POSTGRES_E2E_REUSE_IMAGE:-}" ]]; then
  web_image="${WEB_PRODUCT_POSTGRES_E2E_REUSE_IMAGE}"
  docker image inspect "${web_image}" >/dev/null
else
  web_image="zerondesign/web:product-postgres-${stamp}"
  docker build -f "${repo_root}/apps/web/Dockerfile" --provenance=false \
    -t "${web_image}" "${repo_root}" >/dev/null
  k3d image import --cluster "${cluster_name}" "${web_image}" >/dev/null
fi

sed "s#image: zerondesign/web:dev#image: ${web_image}#" \
  "${repo_root}/infra/workspace-provisioner/web-product-catalog.yaml" \
  | kubectl apply -f - >/dev/null
kubectl -n "${runtime_namespace}" rollout status \
  "deployment/${web_deployment}" --timeout=360s >/dev/null

web_pod="$(kubectl -n "${runtime_namespace}" get pod -l app=zerondesign-web \
  -o jsonpath='{.items[0].metadata.name}')"
web_uid_before="$(kubectl -n "${runtime_namespace}" get pod "${web_pod}" \
  -o jsonpath='{.metadata.uid}')"
kubectl -n "${runtime_namespace}" exec "${web_pod}" -- \
  sh -ec 'test ! -e /app/apps/web/.data/product.sqlite'

kubectl -n "${runtime_namespace}" port-forward service/zerondesign-web 18083:3000 \
  >"${work_dir}/web-port-forward.log" 2>&1 &
port_forward_pid=$!
for attempt in $(seq 1 60); do
  if curl --fail --silent http://127.0.0.1:18083/api/health \
    >"${work_dir}/health-before.json" 2>/dev/null; then
    break
  fi
  [[ "${attempt}" != "60" ]] || {
    cat "${work_dir}/web-port-forward.log" >&2
    exit 3
  }
  sleep 1
done
[[ "$(jq -r '.backend' "${work_dir}/health-before.json")" == "postgresql" ]]
[[ "$(jq -r '.schemaVersion' "${work_dir}/health-before.json")" == "product-catalog@1" ]]

session_cookie="$(SESSION_SECRET="${session_secret}" node <<'NODE'
const {createHmac}=require('node:crypto');
const payload=Buffer.from(JSON.stringify({sub:'rc-harness-principal',exp:Math.floor(Date.now()/1000)+3600})).toString('base64url');
const signature=createHmac('sha256',process.env.SESSION_SECRET).update(payload).digest('base64url');
process.stdout.write(`${payload}.${signature}`);
NODE
)"
api_curl=(curl --silent --show-error -H "cookie: zerondesign_session=${session_cookie}")

for workspace in ws-greenfield-a ws-greenfield-b; do
  if [[ "${workspace}" == "ws-greenfield-a" ]]; then
    workspace_name="Greenfield Workspace A"
  else
    workspace_name="Greenfield Workspace B"
  fi
  "${api_curl[@]}" --fail -H 'content-type: application/json' \
    --data "{\"namespace\":\"${workspace}\",\"name\":\"${workspace_name}\",\"ownerPrincipalId\":\"rc-harness-principal\"}" \
    http://127.0.0.1:18083/api/admin/workspaces \
    >"${work_dir}/workspace-${workspace}.json"
done

project_a="$(jq -r '.projects[0].projectId' "${tls_evidence}")"
project_b="$(jq -r '.projects[1].projectId' "${tls_evidence}")"
kubectl -n "${runtime_namespace}" exec -i "${postgres_pod}" -- \
  psql -U anydesign_runtime -d "${web_database}" -v ON_ERROR_STOP=1 \
  --set=project_a="${project_a}" --set=project_b="${project_b}" >/dev/null <<'SQL'
INSERT INTO projects
  (id, owner_id, name, kind, runtime_project_id, workspace_namespace, status, created_at, updated_at)
VALUES
  (:'project_a', 'rc-harness-principal', 'Published Website', 'website', :'project_a',
   'ws-greenfield-a', 'published', CURRENT_TIMESTAMP::TEXT, CURRENT_TIMESTAMP::TEXT),
  (:'project_b', 'rc-harness-principal', 'Published Docs', 'docs', :'project_b',
   'ws-greenfield-b', 'published', CURRENT_TIMESTAMP::TEXT, CURRENT_TIMESTAMP::TEXT)
ON CONFLICT(id) DO NOTHING;
SQL

psql_web_query() {
  kubectl -n "${runtime_namespace}" exec "${postgres_pod}" -- \
    psql -U anydesign_runtime -d "${web_database}" -Atqc "$1"
}
psql_runtime_query() {
  kubectl -n "${runtime_namespace}" exec "${postgres_pod}" -- \
    psql -U anydesign_runtime -d anydesign_runtime -Atqc "$1"
}
catalog_digest() {
  psql_web_query "SELECT concat_ws('|', id, owner_id, name, kind, runtime_project_id, workspace_namespace, status, created_at, updated_at) FROM projects ORDER BY id" \
    | sha256sum | awk '{print $1}'
}

"${api_curl[@]}" --fail http://127.0.0.1:18083/api/projects \
  >"${work_dir}/projects-before.json"
project_count_before="$(jq '.projects | length' "${work_dir}/projects-before.json")"
(( project_count_before >= 2 ))
for project in "${project_a}" "${project_b}"; do
  jq -e --arg project "${project}" '.projects | any(.id == $project and .status == "published")' \
    "${work_dir}/projects-before.json" >/dev/null
done

"${api_curl[@]}" --fail -X PATCH -H 'content-type: application/json' \
  --data '{"status":"disabled"}' \
  http://127.0.0.1:18083/api/admin/workspaces/ws-greenfield-b \
  >"${work_dir}/workspace-disabled.json"
runtime_revision_before_rejection="$(psql_runtime_query "SELECT revision FROM runtime_control_plane_files WHERE file_path='project-access.jsonl'")"
rejected_status="$("${api_curl[@]}" --output "${work_dir}/project-rejected.json" \
  --write-out '%{http_code}' -H 'content-type: application/json' \
  --data "{\"name\":\"Rejected ${stamp}\",\"kind\":\"docs\",\"workspaceNamespace\":\"ws-greenfield-b\"}" \
  http://127.0.0.1:18083/api/projects)"
[[ "${rejected_status}" == "403" ]]
runtime_revision_after_rejection="$(psql_runtime_query "SELECT revision FROM runtime_control_plane_files WHERE file_path='project-access.jsonl'")"
[[ "${runtime_revision_after_rejection}" == "${runtime_revision_before_rejection}" ]]
[[ "$(psql_web_query 'SELECT count(*) FROM projects')" == "${project_count_before}" ]]
"${api_curl[@]}" --fail -X PATCH -H 'content-type: application/json' \
  --data '{"status":"active"}' \
  http://127.0.0.1:18083/api/admin/workspaces/ws-greenfield-b \
  >"${work_dir}/workspace-active.json"

runtime_revision_before_success="$(psql_runtime_query "SELECT revision FROM runtime_control_plane_files WHERE file_path='project-access.jsonl'")"
"${api_curl[@]}" --fail -H 'content-type: application/json' \
  --data "{\"name\":\"PostgreSQL Saga ${stamp}\",\"kind\":\"website\",\"workspaceNamespace\":\"ws-greenfield-a\"}" \
  http://127.0.0.1:18083/api/projects >"${work_dir}/project-created.json"
created_project="$(jq -r '.project.id' "${work_dir}/project-created.json")"
[[ -n "${created_project}" && "${created_project}" != "null" ]]
[[ "$(jq -r '.project.status' "${work_dir}/project-created.json")" == "draft" ]]
project_count_after_success="$(psql_web_query 'SELECT count(*) FROM projects')"
[[ "${project_count_after_success}" == "$((project_count_before + 1))" ]]
runtime_revision_after_success="$(psql_runtime_query "SELECT revision FROM runtime_control_plane_files WHERE file_path='project-access.jsonl'")"
(( runtime_revision_after_success > runtime_revision_before_success ))
[[ "$(psql_runtime_query "SELECT count(*) FROM runtime_control_plane_files WHERE file_path='project-access.jsonl' AND convert_from(content, 'UTF8') LIKE '%${created_project}%'")" == "1" ]]

catalog_digest_before="$(catalog_digest)"
postgres_uid_before="$(kubectl -n "${runtime_namespace}" get pod "${postgres_pod}" \
  -o jsonpath='{.metadata.uid}')"
kubectl -n "${runtime_namespace}" delete pod "${postgres_pod}" --wait=true >/dev/null
kubectl -n "${runtime_namespace}" rollout status \
  "statefulset/${postgres_statefulset}" --timeout=300s >/dev/null
postgres_pod="$(kubectl -n "${runtime_namespace}" get pod \
  -l app=anydesign-postgres -o jsonpath='{.items[0].metadata.name}')"
postgres_uid_after="$(kubectl -n "${runtime_namespace}" get pod "${postgres_pod}" \
  -o jsonpath='{.metadata.uid}')"
[[ "${postgres_uid_after}" != "${postgres_uid_before}" ]]

kubectl -n "${runtime_namespace}" rollout restart "deployment/${web_deployment}" >/dev/null
replacement_web_pod=""
for attempt in $(seq 1 120); do
  replacement_web_pod="$(kubectl -n "${runtime_namespace}" get pods -l app=zerondesign-web \
    -o json | jq -r --arg old_uid "${web_uid_before}" \
    '[.items[] | select(.metadata.uid != $old_uid and .metadata.deletionTimestamp == null)][0].metadata.name // empty')"
  [[ -n "${replacement_web_pod}" ]] && break
  [[ "${attempt}" != "120" ]] || {
    printf 'Web rollout did not create a replacement Pod\n' >&2
    exit 5
  }
  sleep 1
done
kubectl -n "${runtime_namespace}" rollout status \
  "deployment/${web_deployment}" --timeout=360s >/dev/null
web_pod="${replacement_web_pod}"
kubectl -n "${runtime_namespace}" wait --for=condition=Ready \
  "pod/${web_pod}" --timeout=180s >/dev/null
web_uid_after="$(kubectl -n "${runtime_namespace}" get pod "${web_pod}" \
  -o jsonpath='{.metadata.uid}')"
[[ "${web_uid_after}" != "${web_uid_before}" ]]
kubectl -n "${runtime_namespace}" exec "${web_pod}" -- \
  sh -ec 'test ! -e /app/apps/web/.data/product.sqlite'

for attempt in $(seq 1 60); do
  if curl --fail --silent http://127.0.0.1:18083/api/health \
    >"${work_dir}/health-after.json" 2>/dev/null; then
    break
  fi
  [[ "${attempt}" != "60" ]] || exit 4
  sleep 1
done
[[ "$(jq -r '.backend' "${work_dir}/health-after.json")" == "postgresql" ]]
"${api_curl[@]}" --fail http://127.0.0.1:18083/api/projects \
  >"${work_dir}/projects-after.json"
[[ "$(jq '.projects | length' "${work_dir}/projects-after.json")" == "${project_count_after_success}" ]]
catalog_digest_after="$(catalog_digest)"
[[ "${catalog_digest_after}" == "${catalog_digest_before}" ]]

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

node - "${evidence_path}" "${tls_evidence}" "${web_image}" \
  "${web_database}" "${web_role}" "${project_count_before}" "${project_count_after_success}" \
  "${created_project}" "${runtime_revision_before_rejection}" "${runtime_revision_after_rejection}" \
  "${runtime_revision_before_success}" "${runtime_revision_after_success}" \
  "${catalog_digest_before}" "${catalog_digest_after}" "${postgres_uid_before}" \
  "${postgres_uid_after}" "${web_uid_before}" "${web_uid_after}" <<'NODE'
const {readFileSync,writeFileSync}=require('node:fs');
const [path,tlsEvidencePath,webImage,database,role,countBefore,countAfter,createdProject,
  rejectedRevisionBefore,rejectedRevisionAfter,successRevisionBefore,successRevisionAfter,
  digestBefore,digestAfter,postgresUidBefore,postgresUidAfter,webUidBefore,webUidAfter]=process.argv.slice(2);
const tls=JSON.parse(readFileSync(tlsEvidencePath,'utf8'));
const evidence={
  schemaVersion:'web-product-catalog-postgres-e2e@1',
  generatedAt:new Date().toISOString(),
  migration:{version:'product-catalog@1',file:'apps/web/migrations/0001_product_catalog.sql',explicit:true,autoMigration:false},
  database:{engine:'postgresql',database,applicationRole:role,credentialsPersistedInEvidence:false,
    catalogDigestBeforeRestart:digestBefore,catalogDigestAfterRestart:digestAfter,
    catalogRecovered:digestBefore===digestAfter,postgresPodUidBeforeRestart:postgresUidBefore,
    postgresPodUidAfterRestart:postgresUidAfter,postgresRestartVerified:postgresUidBefore!==postgresUidAfter},
  catalog:{legacySqlitePresent:false,publishedProjectsBootstrapped:2,
    projectCountBeforeSaga:Number(countBefore),projectCountAfterSaga:Number(countAfter),
    createdProjectId:createdProject,sqliteAbsentInWebPod:true},
  transactionBoundary:{disabledWorkspaceStatus:403,
    runtimeRevisionBeforeRejectedCreate:Number(rejectedRevisionBefore),
    runtimeRevisionAfterRejectedCreate:Number(rejectedRevisionAfter),
    rejectedCreateDidNotReachRuntime:rejectedRevisionBefore===rejectedRevisionAfter,
    runtimeRevisionBeforeSuccessfulCreate:Number(successRevisionBefore),
    runtimeRevisionAfterSuccessfulCreate:Number(successRevisionAfter),
    successfulCreateReachedRuntime:Number(successRevisionAfter)>Number(successRevisionBefore)},
  web:{image:webImage,healthBackend:'postgresql',schemaVersion:'product-catalog@1',
    podUidBeforeRestart:webUidBefore,podUidAfterRestart:webUidAfter,
    restartVerified:webUidBefore!==webUidAfter},
  projects:tls.projects.map(({projectId,workspaceNamespace,url,releaseId})=>({
    projectId,workspaceNamespace,url,releaseId,externalHttpsVerified:true,releaseIdentityVerified:true,
  })),
};
writeFileSync(path,`${JSON.stringify(evidence,null,2)}\n`);
NODE

jq -e '
  .database.catalogRecovered == true and
  .database.postgresRestartVerified == true and
  .catalog.sqliteAbsentInWebPod == true and
  .transactionBoundary.rejectedCreateDidNotReachRuntime == true and
  .transactionBoundary.successfulCreateReachedRuntime == true and
  .web.restartVerified == true and
  ([.projects[].externalHttpsVerified] | all)
' "${evidence_path}" >/dev/null

printf 'Web product catalog PostgreSQL E2E passed. Evidence: %s\n' "${evidence_path}"
jq -r '.projects[].url' "${evidence_path}"
