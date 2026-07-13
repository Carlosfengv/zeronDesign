# G8 Blue/Green, Rollback, Recovery, and GC Evidence

Date: 2026-07-12

Branch: `codex/runtime-architecture-g8`

Cluster/context: `zerondesign-g8` / `k3d-zerondesign-g8`

Namespace: `anydesign-works`

## Implemented boundary

- Every target WorkRelease owns a release-specific Deployment. Update and Rollback never mutate the image of the current Deployment.
- Green is made Available and verified through a temporary release-specific Service before the stable Service changes.
- The stable Service release selector changes through a live `resourceVersion` CAS. A selector already at desired green is a resumable crash checkpoint; a selector at neither persisted blue nor desired green is drift and fails closed.
- Store `currentReleaseId` remains blue until EndpointSlices contain only target-release Pods and the unchanged HTTPS host returns the target release header and body.
- EndpointSlice convergence is durably recorded as `TrafficSwitched` before the external probe; restart resumes from that operation checkpoint.
- EndpointSlice or external verification timeout CAS-restores the blue selector, verifies blue EndpointSlices and external identity, leaves Store current unchanged, and records `reconcile_required`.
- Successful Update/Rollback atomically advances current/previous deployment and release identities. The prior Deployment remains live as the immediate rollback target.
- HTML responses carry `Cache-Control: no-store` during the bounded switch model.
- Registry GC requires a protection snapshot covering Runtime desired/current/previous/last-successful IDs, nonterminal operations and packaging, and every live Deployment release ID/image digest in the Published namespace. A protected match or unavailable scan blocks deletion before any store mutation or backend deletion.

## Real lifecycle and fault-injection gate

Command:

```text
bash infra/public-runtime/run-g8-k3d-e2e.sh
```

Result:

```text
test update_rollback_restart_and_failed_switch_restore_blue_on_k3d ... ok
G8 k3d gate passed: cluster=zerondesign-g8
host=w-5fc4f12d7947bec1bc46.g8.test
releaseA=release-6e2fc7bac9ff969db25cfc510aa854c3
digestA=sha256:d8b2c64f680dc068cea917388fa264ab2c370ec4dbbcc11d3436f51094daa34d
releaseB=release-454e623cce1ec899c9ff9b68a550021d
digestB=sha256:2a34ec07e4e77da357eef052c14ff02fa3c0ff4f6f73595525e97a9766728d4a
```

The executable gate proves:

1. initial A publication uses the stable TLS host;
2. A to B Update retains both release-specific Deployments, converges EndpointSlices exclusively to B, then commits B current/A previous;
3. B to A Rollback changes real Service traffic and Store state, not only a pointer;
4. a simulated crash after selector switch but before Store commit resumes at EndpointSlice/external verification and completes B;
5. an injected old-release EndpointSlice forces the bounded A switch to time out; the controller restores B traffic and keeps Store current at B;
6. after removing the fault, the same operation replays idempotently and completes Rollback to A;
7. the live protection snapshot contains both retained release IDs and both immutable image digests;
8. HTML is `no-store`, while release identity remains verified by header and body.

## Unit and architecture evidence

- `release::garbage_collector::tests::gc_fails_closed_for_protected_or_unavailable_reference_scans` proves no GC state transition occurs for protected IDs or unavailable reference scans.
- Publication store tests prove release history remains protected across Published and Unpublished states.
- Publication architecture gates enforce a separate `kubernetes_switch.rs` boundary, selector CAS, EndpointSlice convergence, and Kubernetes live-reference source.
- Release architecture gates keep Kubernetes code outside the release domain and require `ReleaseProtectionSource` before Registry GC.

## Operational boundary

The protection policy is intentionally conservative. Validated releases are still not broadly made garbage-collectable, and any retained live Deployment remains protected. Automated expiry/deletion of retention-window Deployments requires an explicit product retention policy and audit-hold model; G8 does not infer one. This prevents unsafe image deletion while still providing the required protection contract for future GC eligibility.

The local gate uses a private CA, `*.g8.test`, Traefik, and two fixture images. Production still requires public DNS/certificate operations, switch timeout metrics and alerts, and the incident procedure in `infra/published-runtime/blue-green-runbook.md`.

## Verification commands

```text
cargo test --manifest-path services/runtime/Cargo.toml --all-targets
cargo clippy --manifest-path services/runtime/Cargo.toml --all-targets -- -D warnings
bash services/runtime/scripts/check-publication-control-plane-architecture.sh
bash services/runtime/scripts/check-release-packaging-architecture.sh
kubectl apply --server-side --force-conflicts --dry-run=server -f infra/public-runtime/base.yaml
bash infra/public-runtime/run-g6-k3d-e2e.sh
bash infra/public-runtime/run-g7-k3d-e2e.sh
bash infra/public-runtime/run-g8-k3d-e2e.sh
```

## 2026-07-13 Runtime API Freeze revalidation

The strongest real lifecycle gate was rerun from the current dirty worktree:

```text
test update_rollback_restart_and_failed_switch_restore_blue_on_k3d ... ok
test result: ok. 1 passed; 0 failed; 1 filtered out
G8 k3d gate passed: cluster=zerondesign-g8
host=w-7c406457803bf7d149e5.g8.test
releaseA=release-6e2fc7bac9ff969db25cfc510aa854c3
digestA=sha256:d8b2c64f680dc068cea917388fa264ab2c370ec4dbbcc11d3436f51094daa34d
releaseB=release-454e623cce1ec899c9ff9b68a550021d
digestB=sha256:2a34ec07e4e77da357eef052c14ff02fa3c0ff4f6f73595525e97a9766728d4a
```

This current-state run again proved stable-host initial publish, A to B update,
B to A rollback, restart recovery after selector switch, injected mixed
EndpointSlice failure with restoration of the prior release, and idempotent
replay after removing the fault.
