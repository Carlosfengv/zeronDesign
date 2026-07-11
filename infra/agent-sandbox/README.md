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

Prerequisites are Docker Desktop, `k3d`, `kubectl`, OpenSSL, Node.js, Rust, and a
Runtime-owned Chromium executable. The runner creates/selects the dedicated
`zerondesign-e2e` cluster and installs the pinned agent-sandbox `v0.5.0`
controller when its CRDs are absent.

Run:

```bash
bash infra/agent-sandbox/run-k8s-e2e.sh
```

Useful local overrides:

```bash
ANYDESIGN_E2E_CLUSTER=zerondesign-e2e \
RUNTIME_BROWSER_EXECUTABLE="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
SANDBOX_BASE_IMAGE=ghcr.io/carlosfengv/zerondesign/astro-website-sandbox:0.1.0 \
bash infra/agent-sandbox/run-k8s-e2e.sh
```

`SANDBOX_BASE_IMAGE` is an offline/local bootstrap override. The default remains
`node:22-bookworm`.

The runner builds the current checkout, adds a worktree content fingerprint when
the tree is dirty, imports the image into k3d, and rejects Pod/image config digest
mismatches. It deploys Astro and Fumadocs warm pools, the internal npm proxy, and
the deny-by-default NetworkPolicy.

Two gates then run:

- authenticated channel smoke with two concurrent claims, process leases, and
  binary archive export;
- Website and Docs Public Runtime API fixture lifecycle with Runtime proxy,
  real Chromium PNG evidence, artifact CAS promotion, Sandbox release, and
  post-release artifact access.

Evidence is written under `services/runtime/target/e2e-evidence/` and is checked
for required fields, cross-project screenshot identity, event order, and
secret-like values. Fixture evidence does not replace the separately approved
real-provider release gate.

## Run the deployed Runtime RC fixture gate

After the sandbox gate has bootstrapped the dedicated cluster, build and deploy
the Runtime OCI image and drive Website plus Docs through the cluster service:

```bash
bash infra/agent-sandbox/run-runtime-rc-gate.sh
```

The runner vendors locked Rust dependencies into the ignored Runtime `target`
directory, builds offline in Docker, imports the image into k3d, deploys the
Runtime and deterministic HTTP model gateway, then cross-checks `/version`, Pod
image ref, and container imageID. Evidence is written to
`services/runtime/target/e2e-evidence/runtime-rc-*.json`.

`RUNTIME_RC_REUSE_IMAGE=<ref>` may be used only to rerun the HTTP fixture driver
against an already-built image; the same commit, image ref, and imageID checks
still apply.

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
