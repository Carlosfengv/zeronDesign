# G6 Kubernetes Work Runtime Evidence

Date: 2026-07-12  
Branch: `codex/runtime-architecture-g6`  
Cluster/context: `zerondesign-g6` / `k3d-zerondesign-g6`  
Namespace: `anydesign-works`

## Implemented boundary

- `WorkRuntimeController` builds one template-neutral `DesiredWorkRuntime` from
  `WorkRuntimeState`, validated `WorkRelease`, and validated packaging evidence.
- The production adapter uses the Kubernetes Rust client directly. It does not
  invoke `kubectl` from the long-running controller.
- Resources use fixed server-side apply manager
  `anydesign-work-runtime-controller` and persist Deployment/Service UID plus
  resourceVersion.
- Each release receives a distinct Deployment; each work receives a stable
  ClusterIP Service and NetworkPolicy. G6 creates no Ingress.
- Published Pods use digest-pinned images, no service-account token, no PVC,
  read-only root filesystem, non-root execution, dropped capabilities,
  RuntimeDefault seccomp, bounded resources, and a bounded writable `/tmp`
  `emptyDir` only.
- The temporary Release Prober uses its own ServiceAccount, a digest-pinned
  image, no API token, and network policy limited to DNS and Published Pod port
  8080. It verifies both health and the desired release ID before the stable
  Service is applied.
- Cross-work egress is denied by the real k3d network-policy controller.
- Same-name Kubernetes UID replacement is not silently adopted; persisted state
  becomes `reconcile_required`. Recovery requires an explicit CAS authorization
  against the last trusted UID before a fresh controller-owned Deployment is
  created.

## Real gate

Command:

```text
bash infra/public-runtime/run-g6-k3d-e2e.sh
```

Result:

```text
test dual_work_isolation_restart_and_uid_drift_on_k3d ... ok
G6 k3d gate passed: cluster=zerondesign-g6
releaseA=release-f06f73c30f41281e6d31c6c4aa1ccdb9
digestA=sha256:fc2569ef2bf41d9a2efa264bbadd42eec571c469625a4b477402015924e47fb6
releaseB=release-e1a3dc61e99bd466dd3d5156b20428ca
digestB=sha256:3c27f9ae432e156509c92e8dc813a31a59d4f630c7f71ed9e80f1f563b1c7b3a
```

The test proves:

1. two projects resolve to different Deployment names, Service names, UIDs,
   release IDs, and image digests;
2. stable Service selectors include both the work identity and release identity;
3. A-to-B and B-to-A HTTP attempts from the work Pods return `BLOCKED`;
4. a new controller instance re-observes the same resource identities;
5. deleting and recreating a Deployment under the same name produces UID drift
   and `reconcile_required` rather than silent ownership takeover;
6. a wrong recovery UID is rejected, while explicit authorization using the
   persisted UID permits recreation with a new UID and returns to Available;
7. the namespace contains no Ingress.

## Trust and admission

The controller rejects apply unless the release image, pushed digest, passing
scan, provenance digest, signature identity, and signature verification digest
are mutually present and the Release/Packaging records are both `validated`.
The native ValidatingAdmissionPolicy independently rejects Published
Deployments that are not controller-owned, tokenless, digest-pinned, and
annotated with the immutable trust evidence.

The local G6 gate uses fixture trust records and a local HTTP Registry; it does
not claim production Registry, KMS/keyless identity, or a production
cryptographic admission provider is approved. Production rollout remains
fail-closed until those environment-specific policies and keys are installed.

## Verification

```text
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
bash services/runtime/scripts/check-publication-control-plane-architecture.sh
bash infra/public-runtime/run-g6-k3d-e2e.sh
```
