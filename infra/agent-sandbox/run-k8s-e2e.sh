#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/../.." && pwd)"
KUBECTL="${KUBECTL:-kubectl}"

required_crds=(
  "sandboxes.agents.x-k8s.io"
  "sandboxclaims.extensions.agents.x-k8s.io"
  "sandboxtemplates.extensions.agents.x-k8s.io"
  "sandboxwarmpools.extensions.agents.x-k8s.io"
)

missing_crds=()
for crd in "${required_crds[@]}"; do
  if ! "${KUBECTL}" get crd "${crd}" >/dev/null 2>&1; then
    missing_crds+=("${crd}")
  fi
done

if (( ${#missing_crds[@]} > 0 )); then
  printf 'agent-sandbox CRDs are not installed in kubectl context %s:\n' "$("${KUBECTL}" config current-context 2>/dev/null || echo '<unknown>')" >&2
  printf '  - %s\n' "${missing_crds[@]}" >&2
  printf 'Install the pinned agent-sandbox controller before running the Phase A E2E:\n' >&2
  printf '  bash infra/agent-sandbox/install-controller.sh\n' >&2
  exit 2
fi

cd "${ROOT_DIR}"

"${KUBECTL}" apply -f infra/agent-sandbox/rbac/runtime-service-account.yaml
"${KUBECTL}" apply -f infra/agent-sandbox/network/default-deny.yaml
"${KUBECTL}" apply -f infra/agent-sandbox/astro-website/sandbox-template.yaml
"${KUBECTL}" apply -f infra/agent-sandbox/astro-website/sandbox-warm-pool.yaml

if [[ "${ANYDESIGN_E2E_RESET_WARM_POOL:-0}" == "1" ]]; then
  "${KUBECTL}" delete sandboxes.agents.x-k8s.io \
    -n anydesign-sandboxes \
    -l agents.x-k8s.io/launch-type=warm \
    --ignore-not-found=true
fi

deadline=$((SECONDS + 180))
while true; do
  ready_replicas="$("${KUBECTL}" get sandboxwarmpool anydesign-astro-website-pool \
    -n anydesign-sandboxes \
    -o 'jsonpath={.status.readyReplicas}' 2>/dev/null || true)"
  if [[ "${ready_replicas:-0}" -ge 1 ]]; then
    break
  fi
  if (( SECONDS >= deadline )); then
    printf 'SandboxWarmPool anydesign-astro-website-pool did not become ready; readyReplicas=%s\n' "${ready_replicas:-0}" >&2
    exit 3
  fi
  sleep 2
done

RUN_AGENT_SANDBOX_E2E=1 \
ANYDESIGN_E2E_SKIP_APPLY=1 \
KUBECTL="${KUBECTL}" \
cargo test --manifest-path services/runtime/Cargo.toml --test k8s_sandbox_e2e -- --nocapture
