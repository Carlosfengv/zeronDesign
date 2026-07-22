#!/usr/bin/env bash
set -Eeuo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"
CONTEXT="${GENERATION_COHORT_CONTEXT:-}"
NAMESPACE="${GENERATION_COHORT_NAMESPACE:-anydesign-runtime}"
SOURCE_DEPLOYMENT="${GENERATION_COHORT_SOURCE_DEPLOYMENT:-anydesign-runtime}"
CONTROL_DEPLOYMENT="anydesign-runtime-generation-control"
CANDIDATE_DEPLOYMENT="anydesign-runtime-generation-candidate"
CONTROL_DATABASE_SECRET="anydesign-runtime-postgres-generation-control"
CANDIDATE_DATABASE_SECRET="anydesign-runtime-postgres-generation-candidate"
CONTROL_PRINCIPAL_SECRET="anydesign-runtime-public-principal-generation-control"
CANDIDATE_PRINCIPAL_SECRET="anydesign-runtime-public-principal-generation-candidate"
DEPLOYMENT_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
EVIDENCE_FILE="${GENERATION_COHORT_DEPLOYMENT_EVIDENCE:-${ROOT_DIR}/services/runtime/target/e2e-evidence/generation-context-cohort-deployments-${DEPLOYMENT_ID}.json}"

for command in "${KUBECTL}" node; do
  command -v "${command}" >/dev/null || {
    printf 'generation_cohort.missing_command: %s\n' "${command}" >&2
    exit 2
  }
done

kubectl_args=()
if [[ -n "${CONTEXT}" ]]; then
  kubectl_args+=(--context "${CONTEXT}")
fi
kube() {
  "${KUBECTL}" "${kubectl_args[@]}" "$@"
}

kube get namespace "${NAMESPACE}" >/dev/null
kube get secret provider-gateway-runtime-auth -n "${NAMESPACE}" >/dev/null
kube get secret anydesign-runtime-postgres -n "${NAMESPACE}" >/dev/null
kube get secret anydesign-runtime-public-principal -n "${NAMESPACE}" >/dev/null
kube get service provider-gateway -n provider-system >/dev/null
kube rollout status statefulset/anydesign-postgres -n "${NAMESPACE}" --timeout=180s >/dev/null

runtime_image="${GENERATION_COHORT_RUNTIME_IMAGE:-}"
if [[ -z "${runtime_image}" ]]; then
  runtime_image="$(kube get deployment "${SOURCE_DEPLOYMENT}" -n "${NAMESPACE}" \
    -o jsonpath='{.spec.template.spec.containers[?(@.name=="runtime")].image}')"
fi
[[ -n "${runtime_image}" ]] || {
  printf 'generation_cohort.runtime_image_missing\n' >&2
  exit 2
}

work_dir="$(mktemp -d)"
cleanup() {
  find "${work_dir}" -type f -delete
  rmdir "${work_dir}"
}
trap cleanup EXIT

# Cohort Runtimes must not share the primary Runtime's fixed control-plane file
# namespace. Create two databases in the existing PostgreSQL server, then derive
# URL-only Secrets without ever printing credentials.
kube exec statefulset/anydesign-postgres -n "${NAMESPACE}" -- sh -ec '
  for database in anydesign_generation_control anydesign_generation_candidate; do
    if ! psql -U "$POSTGRES_USER" -d "$POSTGRES_DB" -Atqc \
      "SELECT 1 FROM pg_database WHERE datname = '\''${database}'\''" | grep -qx 1; then
      createdb -U "$POSTGRES_USER" "$database"
    fi
  done
' >/dev/null
kube get secret anydesign-runtime-postgres -n "${NAMESPACE}" -o json >"${work_dir}/source-postgres.json"
kube get secret anydesign-runtime-public-principal -n "${NAMESPACE}" -o json >"${work_dir}/source-principal.json"
node - "${work_dir}/source-postgres.json" "${work_dir}/source-principal.json" \
  "${work_dir}/cohort-secrets.json" "${NAMESPACE}" <<'NODE'
const fs = require("node:fs");
const [postgresFile, principalFile, output, namespace] = process.argv.slice(2);
const postgres = JSON.parse(fs.readFileSync(postgresFile, "utf8"));
const principal = JSON.parse(fs.readFileSync(principalFile, "utf8"));
const rawUrl = Buffer.from(postgres.data?.url || "", "base64").toString("utf8");
const parsed = new URL(rawUrl);
if (!['postgres:', 'postgresql:'].includes(parsed.protocol) || !parsed.hostname) {
  throw new Error("primary PostgreSQL URL is invalid");
}
if (!principal.data?.["public.der"]) throw new Error("primary public Principal key is missing");
const pairs = [
  ["control", "anydesign_generation_control"],
  ["candidate", "anydesign_generation_candidate"],
];
const items = [];
for (const [side, database] of pairs) {
  const url = new URL(parsed);
  url.pathname = `/${database}`;
  items.push({
    apiVersion: "v1",
    kind: "Secret",
    metadata: { name: `anydesign-runtime-postgres-generation-${side}`, namespace },
    type: "Opaque",
    data: { url: Buffer.from(url.toString()).toString("base64") },
  });
  items.push({
    apiVersion: "v1",
    kind: "Secret",
    metadata: { name: `anydesign-runtime-public-principal-generation-${side}`, namespace },
    type: "Opaque",
    data: { "public.der": principal.data["public.der"] },
  });
}
fs.writeFileSync(output, `${JSON.stringify({ apiVersion: "v1", kind: "List", items })}\n`, { mode: 0o600 });
NODE
kube apply -f "${work_dir}/cohort-secrets.json" >/dev/null

kube kustomize --load-restrictor=LoadRestrictionsNone \
  "${ROOT_DIR}/infra/generation-reliability/cohort/control" >"${work_dir}/control-rendered.yaml"
kube kustomize --load-restrictor=LoadRestrictionsNone \
  "${ROOT_DIR}/infra/generation-reliability/cohort/candidate" >"${work_dir}/candidate-rendered.yaml"
node - "${runtime_image}" "${work_dir}/control-rendered.yaml" \
  "${work_dir}/candidate-rendered.yaml" <<'NODE'
const fs = require("node:fs");
const [runtimeImage, ...files] = process.argv.slice(2);
for (const file of files) {
  const source = fs.readFileSync(file, "utf8");
  const matches = source.match(/image: anydesign\/runtime:dev/g) || [];
  if (matches.length !== 1) throw new Error(`expected one Runtime image in ${file}`);
  fs.writeFileSync(file, source.replace("image: anydesign/runtime:dev", `image: ${runtimeImage}`), { mode: 0o600 });
}
NODE
kube apply -f "${work_dir}/control-rendered.yaml" >/dev/null
kube apply -f "${work_dir}/candidate-rendered.yaml" >/dev/null
kube rollout status deployment/"${CONTROL_DEPLOYMENT}" -n "${NAMESPACE}" --timeout=300s >/dev/null
kube rollout status deployment/"${CANDIDATE_DEPLOYMENT}" -n "${NAMESPACE}" --timeout=300s >/dev/null

for side in control candidate; do
  deployment="${CONTROL_DEPLOYMENT}"
  expected_mode="off"
  expected_role="generation-control"
  expected_database_secret="${CONTROL_DATABASE_SECRET}"
  expected_object_storage="s3://anydesign-runtime/generation-control"
  expected_principal_secret="${CONTROL_PRINCIPAL_SECRET}"
  expected_producer_required="false"
  if [[ "${side}" == "candidate" ]]; then
    deployment="${CANDIDATE_DEPLOYMENT}"
    expected_mode="enabled"
    expected_role="generation-candidate"
    expected_database_secret="${CANDIDATE_DATABASE_SECRET}"
    expected_object_storage="s3://anydesign-runtime/generation-candidate"
    expected_principal_secret="${CANDIDATE_PRINCIPAL_SECRET}"
    expected_producer_required="true"
  fi
  kube get deployment "${deployment}" -n "${NAMESPACE}" -o json >"${work_dir}/${side}.json"
  kube get service "${deployment}" -n "${NAMESPACE}" -o json >"${work_dir}/${side}-service.json"
  kube get pods -n "${NAMESPACE}" \
    -l "app=anydesign-runtime,anydesign.io/runtime-role=${expected_role}" \
    -o json >"${work_dir}/${side}-pods.json"
  node - "${work_dir}/${side}.json" "${work_dir}/${side}-service.json" \
    "${work_dir}/${side}-pods.json" \
    "${expected_mode}" "${expected_role}" "${runtime_image}" \
    "${expected_database_secret}" "${expected_object_storage}" \
    "${expected_principal_secret}" "${expected_producer_required}" <<'NODE'
const fs = require("node:fs");
const [file, serviceFile, podsFile, expectedMode, expectedRole, expectedImage, expectedDatabaseSecret,
  expectedObjectStorage, expectedPrincipalSecret, expectedProducerRequired] = process.argv.slice(2);
const deployment = JSON.parse(fs.readFileSync(file, "utf8"));
const service = JSON.parse(fs.readFileSync(serviceFile, "utf8"));
const pods = (JSON.parse(fs.readFileSync(podsFile, "utf8")).items || [])
  .filter(item => !item.metadata?.deletionTimestamp);
const container = deployment.spec?.template?.spec?.containers?.find(item => item.name === "runtime");
const env = Object.fromEntries((container?.env || []).map(item => [item.name, item]));
const volumes = Object.fromEntries((deployment.spec?.template?.spec?.volumes || []).map(item => [item.name, item]));
if (container?.image !== expectedImage) throw new Error("cohort Runtime image drift");
if (deployment.spec?.template?.metadata?.labels?.["anydesign.io/runtime-role"] !== expectedRole) {
  throw new Error("cohort Runtime Pod role drift");
}
if (service.spec?.selector?.["anydesign.io/runtime-role"] !== expectedRole) {
  throw new Error("cohort Runtime Service selector can overlap the primary Runtime");
}
if (env.MODEL_PROVIDER?.value !== "internal_gateway") throw new Error("cohort Runtime must use internal_gateway");
if (env.MODEL_GATEWAY_URL?.value !== "http://provider-gateway.provider-system.svc.cluster.local:9000") {
  throw new Error("cohort Runtime Gateway URL drift");
}
if (env.MODEL_GATEWAY_AUTH_TOKEN?.valueFrom?.secretKeyRef?.name !== "provider-gateway-runtime-auth") {
  throw new Error("cohort Runtime must use the workload-token SecretRef");
}
if (env.RUNTIME_GENERATION_CONTEXT_MODE?.value !== expectedMode) throw new Error("cohort mode drift");
if (env.RUNTIME_CONTENT_PLAN_ATTESTATION_MODE?.value !== "shadow") throw new Error("cohort attestation must remain shadow");
if (env.RUNTIME_CONTENT_PLAN_APPROVAL_PRODUCER_REQUIRED?.value !== expectedProducerRequired) {
  throw new Error("cohort Approval Producer mode drift");
}
if (env.DATABASE_URL?.valueFrom?.secretKeyRef?.name !== expectedDatabaseSecret) {
  throw new Error("cohort Runtime database is not isolated");
}
if (env.OBJECT_STORAGE_URL?.value !== expectedObjectStorage) {
  throw new Error("cohort Runtime object-storage prefix is not isolated");
}
if (volumes["public-principal"]?.secret?.secretName !== expectedPrincipalSecret) {
  throw new Error("cohort Runtime public Principal Secret is not isolated");
}
if (pods.length !== 1) throw new Error("cohort Runtime must have exactly one Pod");
const runtimeStatus = pods[0].status?.containerStatuses?.find(item => item.name === "runtime");
if (!runtimeStatus?.ready || !runtimeStatus.imageID) throw new Error("cohort Runtime Pod image identity is not ready");
NODE
done

kube get service "${SOURCE_DEPLOYMENT}" -n "${NAMESPACE}" -o json >"${work_dir}/source-service.json"
kube get deployment "${SOURCE_DEPLOYMENT}" -n "${NAMESPACE}" -o json >"${work_dir}/source-deployment.json"
node - "${work_dir}/source-service.json" "${work_dir}/source-deployment.json" <<'NODE'
const fs = require("node:fs");
const service = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const deployment = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));
if (service.spec?.selector?.["anydesign.io/runtime-role"] !== "primary") {
  throw new Error("primary Runtime Service must select only anydesign.io/runtime-role=primary");
}
const container = deployment.spec?.template?.spec?.containers?.find(item => item.name === "runtime");
const env = Object.fromEntries((container?.env || []).map(item => [item.name, item]));
const volumes = Object.fromEntries((deployment.spec?.template?.spec?.volumes || []).map(item => [item.name, item]));
if (env.DATABASE_URL?.valueFrom?.secretKeyRef?.name !== "anydesign-runtime-postgres"
  || env.OBJECT_STORAGE_URL?.value !== "s3://anydesign-runtime/greenfield"
  || volumes["public-principal"]?.secret?.secretName !== "anydesign-runtime-public-principal") {
  throw new Error("primary Runtime isolation boundary drift");
}
NODE

mkdir -p "$(dirname "${EVIDENCE_FILE}")"
node - "${work_dir}/control.json" "${work_dir}/control-pods.json" \
  "${work_dir}/candidate.json" "${work_dir}/candidate-pods.json" "${EVIDENCE_FILE}" <<'NODE'
const crypto = require("node:crypto");
const fs = require("node:fs");
const [controlFile, controlPodsFile, candidateFile, candidatePodsFile, output] = process.argv.slice(2);
const project = (file, podsFile, side) => {
  const value = JSON.parse(fs.readFileSync(file, "utf8"));
  const pods = (JSON.parse(fs.readFileSync(podsFile, "utf8")).items || [])
    .filter(item => !item.metadata?.deletionTimestamp);
  if (pods.length !== 1) throw new Error(`${side} must have exactly one Pod`);
  const pod = pods[0];
  const container = value.spec.template.spec.containers.find(item => item.name === "runtime");
  const runtimeStatus = pod.status?.containerStatuses?.find(item => item.name === "runtime");
  if (!runtimeStatus?.ready || !runtimeStatus.imageID) throw new Error(`${side} Pod image identity is unavailable`);
  const env = Object.fromEntries(container.env.map(item => [item.name, item]));
  return {
    side,
    deployment: value.metadata.name,
    uid: value.metadata.uid,
    generation: value.metadata.generation,
    observedGeneration: value.status?.observedGeneration ?? null,
    podUid: pod.metadata.uid,
    image: container.image,
    imageId: runtimeStatus.imageID,
    generationContextMode: env.RUNTIME_GENERATION_CONTEXT_MODE.value,
    publicBaseUrl: env.RUNTIME_PUBLIC_BASE_URL.value,
    gatewayUrl: env.MODEL_GATEWAY_URL.value,
    workloadTokenSecretRef: env.MODEL_GATEWAY_AUTH_TOKEN.valueFrom.secretKeyRef.name,
    databaseSecretRef: env.DATABASE_URL.valueFrom.secretKeyRef.name,
    objectStorageUrl: env.OBJECT_STORAGE_URL.value,
    publicPrincipalSecretRef: value.spec.template.spec.volumes
      .find(item => item.name === "public-principal").secret.secretName,
    contentPlanAttestationMode: env.RUNTIME_CONTENT_PLAN_ATTESTATION_MODE.value,
    approvalProducerRequired: env.RUNTIME_CONTENT_PLAN_APPROVAL_PRODUCER_REQUIRED.value,
  };
};
const evidence = {
  schemaVersion: "generation-context-cohort-deployments@2",
  recordedAt: new Date().toISOString(),
  deployments: [
    project(controlFile, controlPodsFile, "control"),
    project(candidateFile, candidatePodsFile, "candidate"),
  ],
};
evidence.identitySha256 = crypto.createHash("sha256").update(JSON.stringify(evidence.deployments)).digest("hex");
fs.writeFileSync(output, `${JSON.stringify(evidence, null, 2)}\n`, { flag: "wx", mode: 0o600 });
NODE

printf 'Generation Context cohort Runtime deployments are ready: control=%s candidate=%s evidence=%s\n' \
  "${CONTROL_DEPLOYMENT}" "${CANDIDATE_DEPLOYMENT}" "${EVIDENCE_FILE}"
