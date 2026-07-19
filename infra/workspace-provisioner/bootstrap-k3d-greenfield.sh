#!/usr/bin/env bash
set -euo pipefail

cluster_name="${ZERONDESIGN_K3D_CLUSTER:-zerondesign-greenfield}"
k3d_bin="${K3D:-k3d}"
kubectl_bin="${KUBECTL:-kubectl}"
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"

if "${k3d_bin}" cluster list --no-headers 2>/dev/null \
  | awk '{print $1}' | grep -Fxq "${cluster_name}"; then
  printf 'refusing to reuse existing k3d cluster: %s\n' "${cluster_name}" >&2
  printf 'delete it or set ZERONDESIGN_K3D_CLUSTER to a new name\n' >&2
  exit 65
fi

"${k3d_bin}" cluster create "${cluster_name}" --servers 1 --agents 1 --wait
"${kubectl_bin}" config use-context "k3d-${cluster_name}" >/dev/null

bash "${repo_root}/infra/agent-sandbox/install-controller.sh"

"${kubectl_bin}" apply -f - <<'EOF'
apiVersion: v1
kind: Namespace
metadata:
  name: anydesign-runtime
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: anydesign-runtime
  namespace: anydesign-runtime
EOF

for workspace_namespace in ws-greenfield-a ws-greenfield-b; do
  RUNTIME_SYSTEM_NAMESPACE=anydesign-runtime \
    bash "${repo_root}/infra/workspace-provisioner/provision-workspace.sh" \
    "${workspace_namespace}"
done

RUNTIME_SYSTEM_NAMESPACE=anydesign-runtime \
  bash "${repo_root}/infra/workspace-provisioner/configure-workspace-channel.sh" \
  ws-greenfield-a ws-greenfield-b

for workspace_namespace in ws-greenfield-a ws-greenfield-b; do
  "${kubectl_bin}" wait \
    --for=jsonpath='{.status.phase}'=Active \
    "namespace/${workspace_namespace}" --timeout=60s >/dev/null
  [[ "$("${kubectl_bin}" get namespace "${workspace_namespace}" \
    -o 'jsonpath={.metadata.labels.zerondesign\.dev/workspace}')" == "true" ]]
  [[ "$("${kubectl_bin}" get sandboxwarmpool -n "${workspace_namespace}" \
    -o 'jsonpath={range .items[*]}{.spec.replicas}{"\n"}{end}' \
    | sort -u)" == "0" ]]
  if "${kubectl_bin}" get pods -n "${workspace_namespace}" -o name | grep -q .; then
    printf 'unexpected resident Sandbox pod in %s\n' "${workspace_namespace}" >&2
    exit 1
  fi
done

printf 'Fresh k3d cluster %s has two isolated zero-warm-replica Workspaces.\n' \
  "${cluster_name}"
printf 'Delete after testing with: %s cluster delete %s\n' \
  "${k3d_bin}" "${cluster_name}"
