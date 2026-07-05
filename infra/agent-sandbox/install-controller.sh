#!/usr/bin/env bash
set -euo pipefail

KUBECTL="${KUBECTL:-kubectl}"
AGENT_SANDBOX_VERSION="${AGENT_SANDBOX_VERSION:-v0.5.0}"
RELEASE_BASE="https://github.com/kubernetes-sigs/agent-sandbox/releases/download/${AGENT_SANDBOX_VERSION}"

printf 'Installing agent-sandbox controller %s into kubectl context %s\n' \
  "${AGENT_SANDBOX_VERSION}" \
  "$("${KUBECTL}" config current-context 2>/dev/null || echo '<unknown>')"

"${KUBECTL}" apply -f "${RELEASE_BASE}/manifest.yaml"
"${KUBECTL}" apply -f "${RELEASE_BASE}/extensions.yaml"

if "${KUBECTL}" get deployment/agent-sandbox-controller -n agent-sandbox-system >/dev/null 2>&1; then
  "${KUBECTL}" rollout status deployment/agent-sandbox-controller \
    -n agent-sandbox-system \
    --timeout=120s
else
  "${KUBECTL}" rollout status statefulset/agent-sandbox-controller \
    -n agent-sandbox-system \
    --timeout=120s
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
