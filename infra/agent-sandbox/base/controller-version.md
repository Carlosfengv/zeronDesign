# Agent Sandbox Controller Version

- API group for `SandboxClaim`: `extensions.agents.x-k8s.io/v1beta1`
- API group for warm pools/templates: `extensions.agents.x-k8s.io/v1beta1`
- Pinned controller release: `kubernetes-sigs/agent-sandbox` `v0.5.0`.
- Install command: `bash infra/agent-sandbox/install-controller.sh`.
- `SandboxClaim.spec.warmPoolRef.name` is required.
- The workspace volume is represented as a `SandboxTemplate.spec.volumeClaimTemplates` entry named `workspace`; runtime labels claims/pods with `anydesign.dev/workspace-pvc=<binding.workspacePvcName>` so the workspace boundary is still `sandbox + PVC`.
- Runtime channel protocol for this MVP contract: `websocket`
- Runtime resolves the workspace channel by listing Services in the sandbox namespace after `SandboxClaim.status.phase == Ready`. It first accepts exact Service names matching the sandbox or claim, then owner/label/annotation matches, and finally falls back to the existing sandbox-name DNS convention.
- The gated K8s E2E may override discovery with `ANYDESIGN_E2E_SANDBOX_SERVICE` when a controller release uses a custom Service naming pattern.
- The framework-neutral sandbox image is built from `infra/agent-sandbox/base/Dockerfile` and must include `/opt/anydesign/bootstrap/workspace-init.sh` plus `/opt/anydesign/bootstrap/workspace-channel-server.js`.

This file pins the Phase A runtime assumptions before the Kubernetes adapter is connected to a live cluster.
