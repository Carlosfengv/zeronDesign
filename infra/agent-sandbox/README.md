# Agent Sandbox Runtime Integration

This directory contains the Phase A Kubernetes contract for the runtime sandbox adapter.

## Publish the Astro sandbox image

The `SandboxTemplate` references:

```text
ghcr.io/carlosfengv/zerondesign/astro-website-sandbox:0.1.0
```

Build and push that image with:

```bash
bash infra/agent-sandbox/astro-website/publish-image.sh
```

Useful overrides:

```bash
SANDBOX_IMAGE_PLATFORMS=linux/amd64 bash infra/agent-sandbox/astro-website/publish-image.sh
PUSH_IMAGE=0 bash infra/agent-sandbox/astro-website/publish-image.sh
```

`PUSH_IMAGE=0` performs a local Docker build only. The real K8s E2E requires the image tag in `astro-website/sandbox-template.yaml` to be pullable by the cluster.

## Run the gated K8s E2E

Prerequisites:

- Current `kubectl` context points at the development cluster.
- agent-sandbox CRDs are installed. The Phase A contract is pinned to `kubernetes-sigs/agent-sandbox` `v0.5.0`:

```bash
bash infra/agent-sandbox/install-controller.sh
```

- The GHCR image above is pushed and pullable.
- `anydesign-sandboxes` and `anydesign-runtime` can be created or already exist.

Run:

```bash
bash infra/agent-sandbox/run-k8s-e2e.sh
```

The script applies the runtime RBAC, network policy, Astro `SandboxTemplate`, and `SandboxWarmPool`, then runs:

```bash
RUN_AGENT_SANDBOX_E2E=1 cargo test --manifest-path services/runtime/Cargo.toml --test k8s_sandbox_e2e -- --nocapture
```

If the controller release uses a custom Service name for the workspace channel, override discovery with:

```bash
ANYDESIGN_E2E_SANDBOX_SERVICE=<service-name> bash infra/agent-sandbox/run-k8s-e2e.sh
```

The workspace boundary for one generated work is the checked-out `SandboxClaim` plus its PVC-backed `/workspace` volume. The `SandboxTemplate` carries a `volumeClaimTemplates` entry named `workspace`; runtime labels each claim/pod with `anydesign.dev/workspace-pvc=<workspacePvcName>` for traceability.

## Run the Phase A freeze gate

Before declaring the Phase A runtime contract frozen, run the full gate:

```bash
bash infra/phase-a/verify.sh
```

The gate runs runtime formatting, runtime tests, shared package tests, shared package typecheck, verifies `apps/web` is absent, and then runs the K8s sandbox E2E above. The K8s E2E is included by default because sandbox + PVC is part of the Phase A runtime contract.

For local iteration only, skip the cluster-dependent step explicitly:

```bash
ANYDESIGN_SKIP_K8S_E2E=1 bash infra/phase-a/verify.sh
```
