#!/usr/bin/env bash
set -Eeuo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
work_dir="$(mktemp -d)"
trap 'rm -rf "${work_dir}"' EXIT

fake_kubectl="${SCRIPT_DIR}/test-fixtures/fake-kubectl-runtime-provider-gateway.sh"
state_file="${work_dir}/kubectl-state"
evidence_file="${work_dir}/gateway-mode.json"

FAKE_KUBECTL_STATE="${state_file}" \
KUBECTL="${fake_kubectl}" \
RUNTIME_PROVIDER_GATEWAY_CONTEXT=k3d-test \
RUNTIME_PROVIDER_GATEWAY_EVIDENCE_FILE="${evidence_file}" \
  bash "${SCRIPT_DIR}/configure-runtime-provider-gateway.sh" >/dev/null

node - "${evidence_file}" <<'NODE'
const value = JSON.parse(require("node:fs").readFileSync(process.argv[2], "utf8"));
if (value.schemaVersion !== "runtime-provider-gateway-mode@1") throw new Error("wrong schema");
if (value.verified !== true) throw new Error("switch was not verified");
if (!value.previous.gatewayUrl.includes("fixture-model-gateway")) throw new Error("previous mode missing");
if (!value.actual.gatewayUrl.includes("provider-gateway.provider-system")) throw new Error("real mode missing");
if (value.actual.authSecretName !== "provider-gateway-runtime-auth") throw new Error("auth ref missing");
NODE

rm -f "${state_file}" "${evidence_file}"
if FAKE_KUBECTL_STATE="${state_file}" \
  FAKE_KUBECTL_BAD_AFTER=1 \
  KUBECTL="${fake_kubectl}" \
  RUNTIME_PROVIDER_GATEWAY_CONTEXT=k3d-test \
  RUNTIME_PROVIDER_GATEWAY_EVIDENCE_FILE="${evidence_file}" \
    bash "${SCRIPT_DIR}/configure-runtime-provider-gateway.sh" >/dev/null 2>&1; then
  printf 'Runtime Provider Gateway mismatch did not fail closed\n' >&2
  exit 1
fi

[[ ! -e "${evidence_file}" ]] || {
  printf 'failed Runtime Provider Gateway switch wrote success evidence\n' >&2
  exit 1
}

printf 'Runtime Provider Gateway mode switch tests passed\n'
