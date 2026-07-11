#!/usr/bin/env bash
set -euo pipefail

KUBECTL="${KUBECTL:-kubectl}"
AGENT_SANDBOX_VERSION="${AGENT_SANDBOX_VERSION:-v0.5.0}"
AGENT_SANDBOX_CONTROLLER_IMAGE="${AGENT_SANDBOX_CONTROLLER_IMAGE:-}"
AGENT_SANDBOX_ROLLOUT_TIMEOUT="${AGENT_SANDBOX_ROLLOUT_TIMEOUT:-600s}"
RELEASE_BASE="https://github.com/kubernetes-sigs/agent-sandbox/releases/download/${AGENT_SANDBOX_VERSION}"

printf 'Installing agent-sandbox controller %s into kubectl context %s\n' \
  "${AGENT_SANDBOX_VERSION}" \
  "$("${KUBECTL}" config current-context 2>/dev/null || echo '<unknown>')"

"${KUBECTL}" apply -f "${RELEASE_BASE}/manifest.yaml"
"${KUBECTL}" apply -f "${RELEASE_BASE}/extensions.yaml"

if "${KUBECTL}" get deployment/agent-sandbox-controller -n agent-sandbox-system >/dev/null 2>&1; then
  if [[ -n "${AGENT_SANDBOX_CONTROLLER_IMAGE}" ]]; then
    "${KUBECTL}" set image deployment/agent-sandbox-controller \
      -n agent-sandbox-system \
      "agent-sandbox-controller=${AGENT_SANDBOX_CONTROLLER_IMAGE}"
  fi
  "${KUBECTL}" rollout status deployment/agent-sandbox-controller \
    -n agent-sandbox-system \
    --timeout="${AGENT_SANDBOX_ROLLOUT_TIMEOUT}"
else
  if [[ -n "${AGENT_SANDBOX_CONTROLLER_IMAGE}" ]]; then
    "${KUBECTL}" set image statefulset/agent-sandbox-controller \
      -n agent-sandbox-system \
      "agent-sandbox-controller=${AGENT_SANDBOX_CONTROLLER_IMAGE}"
  fi
  "${KUBECTL}" rollout status statefulset/agent-sandbox-controller \
    -n agent-sandbox-system \
    --timeout="${AGENT_SANDBOX_ROLLOUT_TIMEOUT}"
fi

required_crds=(
  "sandboxes.agents.x-k8s.io"
  "sandboxclaims.extensions.agents.x-k8s.io"
  "sandboxtemplates.extensions.agents.x-k8s.io"
  "sandboxwarmpools.extensions.agents.x-k8s.io"
)

for crd in "${required_crds[@]}"; do
  "${KUBECTL}" get crd "${crd}" >/dev/null
done

printf 'agent-sandbox controller %s is installed and required CRDs are present.\n' \
  "${AGENT_SANDBOX_VERSION}"
