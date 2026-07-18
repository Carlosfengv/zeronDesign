#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
K3D="${K3D:-k3d}"
KUBECTL="${KUBECTL:-kubectl}"

cluster_name="${GENERATION_MATRIX_CLUSTER:-zerondesign-e2e}"
matrix_mode="${GENERATION_MATRIX_MODE:-fixture}"
bootstrap_mode="${GENERATION_MATRIX_BOOTSTRAP:-auto}"
rc_mode="${GENERATION_MATRIX_RC_MODE:-audit}"
runtime_port="${GENERATION_MATRIX_RUNTIME_PORT:-}"
evidence_dir="${GENERATION_MATRIX_EVIDENCE_DIR:-services/runtime/target/e2e-evidence/${cluster_name}}"
provider_env_file="${GENERATION_PROVIDER_ENV_FILE:-}"
runtime_image="${GENERATION_RUNTIME_IMAGE:-}"
dry_run="${GENERATION_MATRIX_DRY_RUN:-0}"

cd "${ROOT_DIR}"

case "${matrix_mode}" in
  fixture | real) ;;
  *)
    printf 'GENERATION_MATRIX_MODE must be fixture or real\n' >&2
    exit 2
    ;;
esac
case "${bootstrap_mode}" in
  auto | always | reuse) ;;
  *)
    printf 'GENERATION_MATRIX_BOOTSTRAP must be auto, always, or reuse\n' >&2
    exit 2
    ;;
esac
case "${rc_mode}" in
  audit | release) ;;
  *)
    printf 'GENERATION_MATRIX_RC_MODE must be audit or release\n' >&2
    exit 2
    ;;
esac
if [[ "${matrix_mode}" == "fixture" && "${rc_mode}" == "release" ]]; then
  printf 'release mode requires GENERATION_MATRIX_MODE=real\n' >&2
  exit 2
fi

load_provider_env_file() {
  local file="$1"
  local line key value mode
  [[ -f "${file}" ]] || {
    printf 'GENERATION_PROVIDER_ENV_FILE does not exist: %s\n' "${file}" >&2
    exit 2
  }
  mode="$(stat -f '%Lp' "${file}" 2>/dev/null || stat -c '%a' "${file}")"
  if [[ "${mode}" != "400" && "${mode}" != "600" ]]; then
    printf 'provider env file permissions must be 400 or 600; actual=%s\n' "${mode}" >&2
    exit 2
  fi
  while IFS= read -r line || [[ -n "${line}" ]]; do
    line="${line%$'\r'}"
    [[ -z "${line}" || "${line}" == \#* ]] && continue
    [[ "${line}" == *=* ]] || {
      printf 'invalid provider env line; expected KEY=VALUE\n' >&2
      exit 2
    }
    key="${line%%=*}"
    value="${line#*=}"
    case "${key}" in
      DEEPSEEK_API_KEY | DEEPSEEK_BASE_URL | DEEPSEEK_E2E_MODEL)
        printf -v "${key}" '%s' "${value}"
        export "${key}"
        ;;
      *)
        printf 'unsupported key in provider env file: %s\n' "${key}" >&2
        exit 2
        ;;
    esac
  done <"${file}"
}

if [[ -n "${provider_env_file}" ]]; then
  load_provider_env_file "${provider_env_file}"
fi
if [[ "${matrix_mode}" == "real" && -z "${DEEPSEEK_API_KEY:-}" ]]; then
  printf 'real matrix requires DEEPSEEK_API_KEY or GENERATION_PROVIDER_ENV_FILE\n' >&2
  exit 2
fi
if [[ "${evidence_dir}" != /* ]]; then
  evidence_dir="${ROOT_DIR}/${evidence_dir}"
fi
mkdir -p "${evidence_dir}"
marker_file="${evidence_dir}/.matrix-start-$$"
touch "${marker_file}"
failure_dir="${evidence_dir}/failure-$(date -u +%Y%m%dT%H%M%SZ)"
matrix_complete=false

redact_diagnostics() {
  local directory="$1"
  node - "${directory}" <<'NODE'
const fs = require("node:fs");
const path = require("node:path");
const root = process.argv[2];
const patterns = [
  [/\bsk-[A-Za-z0-9_-]{12,}\b/g, "[REDACTED_API_KEY]"],
  [/(authorization\s*[:=]\s*(?:bearer\s+)?)[^\s"']+/gi, "$1[REDACTED]"],
  [/(api[_-]?key\s*[:=]\s*)[^\s"']+/gi, "$1[REDACTED]"],
  [/-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----/g, "[REDACTED_PRIVATE_KEY]"],
];
for (const entry of fs.readdirSync(root, { withFileTypes: true })) {
  if (!entry.isFile()) continue;
  const file = path.join(root, entry.name);
  let value = fs.readFileSync(file, "utf8");
  for (const [pattern, replacement] of patterns) value = value.replace(pattern, replacement);
  fs.writeFileSync(file, value);
}
NODE
}

collect_failure_evidence() {
  local status="$1"
  mkdir -p "${failure_dir}"
  printf '%s\n' "${status}" >"${failure_dir}/exit-status.txt"
  printf '%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" >"${failure_dir}/recorded-at.txt"
  "${KUBECTL}" config current-context >"${failure_dir}/kube-context.txt" 2>&1 || true
  "${KUBECTL}" get nodes -o wide >"${failure_dir}/nodes.txt" 2>&1 || true
  "${KUBECTL}" get pods -A -o wide >"${failure_dir}/pods.txt" 2>&1 || true
  "${KUBECTL}" get deployments,statefulsets,pvc -A >"${failure_dir}/workloads.txt" 2>&1 || true
  "${KUBECTL}" get events -A --sort-by=.metadata.creationTimestamp \
    >"${failure_dir}/events.txt" 2>&1 || true
  for namespace in anydesign-runtime provider-system anydesign-sandboxes; do
    while IFS= read -r pod; do
      [[ -n "${pod}" ]] || continue
      "${KUBECTL}" logs -n "${namespace}" "${pod}" --all-containers --tail=250 \
        >"${failure_dir}/${namespace}-${pod}.log" 2>&1 || true
      "${KUBECTL}" logs -n "${namespace}" "${pod}" --all-containers --previous --tail=250 \
        >"${failure_dir}/${namespace}-${pod}-previous.log" 2>&1 || true
    done < <("${KUBECTL}" get pods -n "${namespace}" -o name 2>/dev/null | sed 's|^pod/||')
  done
  redact_diagnostics "${failure_dir}"
  printf 'Generation matrix failure evidence: %s\n' "${failure_dir}" >&2
}

on_exit() {
  local status=$?
  rm -f "${marker_file}"
  if [[ "${status}" -ne 0 && "${matrix_complete}" != "true" ]]; then
    collect_failure_evidence "${status}"
  fi
  exit "${status}"
}
trap on_exit EXIT

if [[ "${dry_run}" == "1" ]]; then
  command -v node >/dev/null || {
    printf 'generation_matrix.missing_command: node\n' >&2
    exit 2
  }
  node - "${cluster_name}" "${matrix_mode}" "${bootstrap_mode}" "${rc_mode}" \
    "${evidence_dir}" "${runtime_image}" "${runtime_port:-auto}" <<'NODE'
const [cluster, mode, bootstrap, rcMode, evidenceDir, runtimeImage, runtimePort] = process.argv.slice(2);
process.stdout.write(`${JSON.stringify({
  schemaVersion: "generation-matrix-plan@1",
  cluster,
  mode,
  bootstrap,
  rcMode,
  runtimePort: runtimePort === "auto" ? "auto" : Number(runtimePort),
  evidenceDir,
  runtimeImage: runtimeImage || null,
  providerCredentialSource: process.env.GENERATION_PROVIDER_ENV_FILE ? "env-file" : "environment",
}, null, 2)}\n`);
NODE
  matrix_complete=true
  exit 0
fi

for command in docker "${K3D}" "${KUBECTL}" node cargo openssl curl; do
  command -v "${command}" >/dev/null || {
    printf 'generation_matrix.missing_command: %s\n' "${command}" >&2
    exit 2
  }
done
if [[ -n "${runtime_port}" ]]; then
  if [[ ! "${runtime_port}" =~ ^[0-9]+$ ]] \
    || (( runtime_port < 1 || runtime_port > 65535 )); then
    printf 'GENERATION_MATRIX_RUNTIME_PORT must be an integer from 1 to 65535\n' >&2
    exit 2
  fi
else
  runtime_port="$(node -e '
const net = require("node:net");
const server = net.createServer();
server.unref();
server.on("error", error => { console.error(error.message); process.exit(1); });
server.listen(0, "127.0.0.1", () => {
  process.stdout.write(String(server.address().port));
  server.close();
});
')"
fi
printf 'Generation matrix Runtime port: %s\n' "${runtime_port}"

docker info >/dev/null
cluster_exists=false
if "${K3D}" cluster list --no-headers 2>/dev/null | awk '{print $1}' | grep -Fxq "${cluster_name}"; then
  cluster_exists=true
fi
channel_evidence="${evidence_dir}/k3d-channel.json"
run_bootstrap=false
case "${bootstrap_mode}" in
  always)
    run_bootstrap=true
    ;;
  auto)
    if [[ "${cluster_exists}" != "true" || ! -s "${channel_evidence}" ]]; then
      run_bootstrap=true
    fi
    ;;
  reuse)
    if [[ "${cluster_exists}" != "true" || ! -s "${channel_evidence}" ]]; then
      printf 'reuse requires cluster %s and channel evidence %s\n' \
        "${cluster_name}" "${channel_evidence}" >&2
      exit 2
    fi
    ;;
esac

if [[ "${run_bootstrap}" == "true" ]]; then
  ANYDESIGN_E2E_CLUSTER="${cluster_name}" \
    E2E_EVIDENCE_DIR="${evidence_dir}" \
    bash infra/agent-sandbox/run-k8s-e2e.sh
else
  "${KUBECTL}" config use-context "k3d-${cluster_name}" >/dev/null
fi

provider_mode="fixture"
if [[ "${matrix_mode}" == "real" ]]; then
  provider_mode="deepseek"
fi

rc_env=(
  "ANYDESIGN_E2E_CLUSTER=${cluster_name}"
  "RUNTIME_RC_MODE=${rc_mode}"
  "RUNTIME_RC_PROVIDER_MODE=${provider_mode}"
  "RUNTIME_RC_PROJECT_FILTER=all"
  "RUNTIME_RC_PORT=${runtime_port}"
  "RUNTIME_RC_EVIDENCE_DIR=${evidence_dir}"
  "RUNTIME_RC_CHANNEL_EVIDENCE=${channel_evidence}"
)
if [[ "${GENERATION_MATRIX_SKIP_PREFLIGHT:-0}" == "1" ]]; then
  rc_env+=("RUNTIME_RC_SKIP_PREFLIGHT=1")
fi
if [[ -n "${runtime_image}" ]]; then
  rc_env+=("RUNTIME_RC_REUSE_IMAGE=${runtime_image}")
fi

env "${rc_env[@]}" bash infra/agent-sandbox/run-runtime-rc-gate.sh

runtime_evidence="$(find "${evidence_dir}" -maxdepth 1 -name 'runtime-rc-*.json' \
  -newer "${marker_file}" -print | sort | tail -n 1)"
if [[ -z "${runtime_evidence}" || ! -s "${runtime_evidence}" ]]; then
  printf 'new Runtime RC evidence was not produced\n' >&2
  exit 6
fi
release_evidence="${evidence_dir}/release-evidence.json"
test -s "${release_evidence}"
deployment_evidence="${evidence_dir}/runtime-deployment.json"
"${KUBECTL}" get deployment anydesign-runtime -n anydesign-runtime -o json \
  >"${deployment_evidence}"
summary_evidence="${evidence_dir}/generation-matrix-summary.json"
node infra/generation-reliability/summarize-matrix-evidence.mjs \
  --runtime "${runtime_evidence}" \
  --release "${release_evidence}" \
  --deployment "${deployment_evidence}" \
  --out "${summary_evidence}" \
  --mode "${matrix_mode}" \
  --cluster "${cluster_name}"

if rg -n -i '(bearer[[:space:]]+[a-z0-9._-]+|sk-[a-z0-9_-]{12,}|api[_-]?key["[:space:]]*:)' \
  "${summary_evidence}" "${runtime_evidence}" "${release_evidence}"; then
  printf 'secret-like value found in generation matrix evidence\n' >&2
  exit 7
fi

matrix_complete=true
printf 'Generation Website/Docs matrix passed: %s\n' "${summary_evidence}"
