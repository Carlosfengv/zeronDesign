# Agent Sandbox Runtime Integration

This directory contains the Phase A Kubernetes contract for the runtime sandbox adapter.

## Publish the framework-neutral sandbox image

The `SandboxTemplate` references:

```text
ghcr.io/carlosfengv/zerondesign/agent-sandbox:0.1.0
```

Build and push that image with:

```bash
bash infra/agent-sandbox/base/publish-image.sh
```

Useful overrides:

```bash
SANDBOX_IMAGE_PLATFORMS=linux/amd64 bash infra/agent-sandbox/base/publish-image.sh
PUSH_IMAGE=0 bash infra/agent-sandbox/base/publish-image.sh
```

`PUSH_IMAGE=0` performs a local Docker build only. The real K8s E2E requires the image tag in each active execution profile's `sandbox-template.yaml` to be pullable by the cluster.

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
SANDBOX_BASE_IMAGE=ghcr.io/carlosfengv/zerondesign/agent-sandbox:0.1.0 \
bash infra/agent-sandbox/run-k8s-e2e.sh
```

`SANDBOX_BASE_IMAGE` is an offline/local bootstrap override. The default remains
the reviewed immutable `node:22-bookworm` digest in `images.lock.json`.

The runner builds the current checkout, adds a worktree content fingerprint when
the tree is dirty, imports the image into k3d, and rejects Pod/image config digest
mismatches. It deploys Fumadocs and Next App execution profiles, the internal npm proxy, and
the deny-by-default NetworkPolicy.

Two gates then run:

- authenticated channel smoke with two concurrent claims, process leases, and
  binary archive export;
- Website and Docs Public Runtime API fixture lifecycle with Runtime proxy,
  real Chromium PNG evidence, artifact CAS promotion, Sandbox release, and
  post-release artifact access.

Evidence is written under `services/runtime/target/e2e-evidence/` and is checked
for required fields, cross-project screenshot identity, event order, and
secret-like values. Fixture evidence does not replace the real-provider release
gate; that gate is credential-controlled and does not require a separate human
approval reference.

## Run the deployed Runtime RC fixture gate

After the sandbox gate has bootstrapped the dedicated cluster, build and deploy
the Runtime OCI image and drive Website plus Docs through the cluster service:

```bash
bash infra/agent-sandbox/run-runtime-rc-gate.sh
```

Release mode additionally requires governed real-provider evidence and a passing stable-prefix cache audit for the
same clean source commit, `modelResourceId`, Provider Resource revision, and Provider configuration SHA-256; audit mode
may omit them:

```bash
RUNTIME_RC_MODE=release \
RUNTIME_RC_PROVIDER_EVIDENCE=/absolute/path/real-provider-examples-summary.json \
RUNTIME_RC_PROVIDER_CACHE_EVIDENCE=/absolute/path/provider-cache-smoke-audit.json \
RUNTIME_RC_TERMINAL_BUNDLE_SET=/absolute/path/runtime-terminal-bundle-set.json \
RUNTIME_RC_BUDGET_POLICY=infra/generation-reliability/release-budget-policy.json \
bash infra/agent-sandbox/run-runtime-rc-gate.sh
```

Use the accepted suite's `real-provider-examples-summary.json` as Provider evidence. Its
`provider.realProviderVerified=true` assertion proves real credential-backed execution, while `modelResourceId`,
`provenance.providerResourceRevision`, and `provenance.providerConfigSha256` freeze the Provider identity. A cache audit
from an older model revision, a different Provider configuration, a different source commit, or a dirty worktree fails
closed. `provider_not_reporting_cached_usage` is not release-eligible and cannot be substituted for a passing cache
audit.

Release Budget Policy is independent from the terminal Bundles. The aggregator hashes the exact policy bytes, replays
every Bundle, validates every frozen `RunBudgetProfile@1`, and requires Production `enforced/split_enforced/enforced`
modes plus approved per-Phase and Operation ceilings. A Shadow Profile remains useful readiness evidence but cannot
become release-eligible by setting a pass boolean in the Bundle.

The release aggregator and final validator recompute `auditedRunCount`, `stableRunCount`, Gross Input, and Cached Input
from the audit's per-Run records; editing only `status` or `releaseEligible` cannot promote incomplete evidence.

The runner vendors locked Rust dependencies into the ignored Runtime `target`
directory, builds offline in Docker, imports the image into k3d, deploys the
Runtime and deterministic HTTP model gateway, then cross-checks `/version`, Pod
image ref, and container imageID. Evidence is written to
`services/runtime/target/e2e-evidence/runtime-rc-*.json`.

For the normal Website/Docs generation reliability workflow, use the unified
entry point:

```bash
bash infra/generation-reliability/run-k3d-matrix.sh
```

It bootstraps or reuses the k3d environment, runs the deployed Runtime RC gate
for both surfaces, validates the five Run budgets, emits one matrix summary,
and captures redacted cluster diagnostics on failure. Real Provider credentials
can be supplied through `GENERATION_PROVIDER_ENV_FILE`; they are never accepted
as command-line arguments.

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
