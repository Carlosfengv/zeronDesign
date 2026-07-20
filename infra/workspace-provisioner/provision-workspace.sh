#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  printf 'usage: %s ws-<stable-id>\n' "$0" >&2
  exit 64
fi

workspace_namespace="$1"
runtime_namespace="${RUNTIME_SYSTEM_NAMESPACE:-anydesign-runtime}"
kubectl_bin="${KUBECTL:-kubectl}"
workspace_cpu_quota="${WORKSPACE_CPU_QUOTA:-4}"
workspace_memory_quota="${WORKSPACE_MEMORY_QUOTA:-8Gi}"
workspace_pod_quota="${WORKSPACE_POD_QUOTA:-50}"
workspace_storage_quota="${WORKSPACE_STORAGE_QUOTA:-20Gi}"
works_ingress_namespace="${WORKS_INGRESS_NAMESPACE:-kube-system}"

if [[ ! "${workspace_namespace}" =~ ^ws-[a-z0-9]([a-z0-9-]*[a-z0-9])?$ ]] \
  || (( ${#workspace_namespace} > 63 )); then
  printf 'invalid Workspace Namespace: %s\n' "${workspace_namespace}" >&2
  exit 64
fi

"${kubectl_bin}" apply -f - <<EOF
apiVersion: v1
kind: Namespace
metadata:
  name: ${workspace_namespace}
  labels:
    zerondesign.dev/workspace: "true"
    app.kubernetes.io/managed-by: zerondesign-workspace-provisioner
---
apiVersion: v1
kind: ResourceQuota
metadata:
  name: zerondesign-workspace-quota
  namespace: ${workspace_namespace}
spec:
  hard:
    requests.cpu: "${workspace_cpu_quota}"
    requests.memory: ${workspace_memory_quota}
    requests.storage: ${workspace_storage_quota}
    pods: "${workspace_pod_quota}"
---
apiVersion: v1
kind: LimitRange
metadata:
  name: zerondesign-workspace-defaults
  namespace: ${workspace_namespace}
spec:
  limits:
    - type: Container
      defaultRequest:
        cpu: 100m
        memory: 256Mi
      default:
        cpu: "2"
        memory: 4Gi
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: anydesign-sandbox
  namespace: ${workspace_namespace}
automountServiceAccountToken: false
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: anydesign-release-prober
  namespace: ${workspace_namespace}
automountServiceAccountToken: false
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: zerondesign-runtime-workspace
  namespace: ${workspace_namespace}
rules:
  - apiGroups: ["extensions.agents.x-k8s.io"]
    resources: ["sandboxclaims", "sandboxtemplates", "sandboxwarmpools"]
    verbs: ["create", "get", "list", "watch", "delete"]
  - apiGroups: ["agents.x-k8s.io"]
    resources: ["sandboxes"]
    verbs: ["get", "list", "watch"]
  - apiGroups: [""]
    resources: ["pods", "services", "endpoints", "persistentvolumeclaims"]
    verbs: ["create", "get", "list", "watch", "update", "patch", "delete"]
  - apiGroups: ["apps"]
    resources: ["deployments"]
    verbs: ["create", "get", "list", "watch", "update", "patch", "delete"]
  - apiGroups: ["networking.k8s.io"]
    resources: ["ingresses", "networkpolicies"]
    verbs: ["create", "get", "list", "watch", "update", "patch", "delete"]
  - apiGroups: ["discovery.k8s.io"]
    resources: ["endpointslices"]
    verbs: ["get", "list", "watch"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: zerondesign-runtime-workspace
  namespace: ${workspace_namespace}
subjects:
  - kind: ServiceAccount
    name: anydesign-runtime
    namespace: ${runtime_namespace}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: zerondesign-runtime-workspace
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: zerondesign-runtime-workspace-reader-${workspace_namespace}
rules:
  - apiGroups: [""]
    resources: ["namespaces"]
    resourceNames: ["${workspace_namespace}"]
    verbs: ["get"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: zerondesign-runtime-workspace-reader-${workspace_namespace}
subjects:
  - kind: ServiceAccount
    name: anydesign-runtime
    namespace: ${runtime_namespace}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: zerondesign-runtime-workspace-reader-${workspace_namespace}
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: zerondesign-default-deny
  namespace: ${workspace_namespace}
spec:
  podSelector: {}
  policyTypes: [Ingress, Egress]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: zerondesign-allow-runtime
  namespace: ${workspace_namespace}
spec:
  podSelector: {}
  policyTypes: [Ingress]
  ingress:
    - from:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: ${runtime_namespace}
      ports:
        - { protocol: TCP, port: 3000 }
        - { protocol: TCP, port: 3001 }
        - { protocol: TCP, port: 4321 }
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: zerondesign-allow-dns-and-npm
  namespace: ${workspace_namespace}
spec:
  podSelector: {}
  policyTypes: [Egress]
  egress:
    - to:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: kube-system
      ports:
        - { protocol: UDP, port: 53 }
        - { protocol: TCP, port: 53 }
    - to:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: ${runtime_namespace}
          podSelector:
            matchLabels:
              app.kubernetes.io/name: anydesign-npm-proxy
      ports:
        - { protocol: TCP, port: 4873 }
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: zerondesign-release-prober-egress
  namespace: ${workspace_namespace}
spec:
  podSelector:
    matchLabels:
      anydesign.dev/role: release-prober
  policyTypes: [Egress]
  egress:
    - to:
        - podSelector:
            matchLabels:
              app.kubernetes.io/managed-by: anydesign-work-runtime-controller
      ports:
        - { protocol: TCP, port: 8080 }
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: zerondesign-allow-published-work-ingress
  namespace: ${workspace_namespace}
spec:
  podSelector:
    matchLabels:
      app.kubernetes.io/managed-by: anydesign-work-runtime-controller
  policyTypes: [Ingress]
  ingress:
    - from:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: ${works_ingress_namespace}
      ports:
        - { protocol: TCP, port: 8080 }
EOF

if [[ -n "${WORKSPACE_CHANNEL_VERIFIER_MANIFEST:-}" ]]; then
  sed "s/namespace: [a-z0-9-]*/namespace: ${workspace_namespace}/g" \
    "${WORKSPACE_CHANNEL_VERIFIER_MANIFEST}" | "${kubectl_bin}" apply -f -
fi
if [[ -n "${WORKSPACE_CHANNEL_TLS_MANIFEST:-}" ]]; then
  sed "s/namespace: [a-z0-9-]*/namespace: ${workspace_namespace}/g" \
    "${WORKSPACE_CHANNEL_TLS_MANIFEST}" | "${kubectl_bin}" apply -f -
fi
repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
for template in next-app fumadocs-docs; do
  sed \
    -e "s/namespace: anydesign-sandboxes/namespace: ${workspace_namespace}/g" \
    -e "s#/ns/anydesign-runtime/#/ns/${runtime_namespace}/#g" \
    "${repo_root}/infra/agent-sandbox/${template}/sandbox-template.yaml" \
    | "${kubectl_bin}" apply -f -
  sed \
    -e "s/namespace: anydesign-sandboxes/namespace: ${workspace_namespace}/g" \
    -e 's/replicas: 1/replicas: 0/' \
    "${repo_root}/infra/agent-sandbox/${template}/sandbox-warm-pool.yaml" \
    | "${kubectl_bin}" apply -f -
done

printf 'Workspace %s is provisioned with zero resident warm Sandbox replicas.\n' \
  "${workspace_namespace}"
