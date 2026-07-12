# Runtime Architecture Master Plan Final Audit

Date: 2026-07-12

Verified repository commit: `e953a350b45db98347c17fe9bde036fd0225597f`

Repository dirty files during audit: `0`

Audit worktree: detached clean worktree created from merged `main`; the user's two untracked planning documents in the primary worktree were not moved, modified, staged, or included in any image.

## Completion decision

**GO / COMPLETE** for G0 through G9 and for the master execution plan's declared `static_web_v1` scope.

The implementation and real-environment evidence satisfy the master exit gates for HTTP modularity, generic templates and Artifact delivery, independent Authoring Sandboxes, immutable Published Work releases, per-work Deployment/Service/Ingress, Publish/Unpublish, Blue/Green update, Rollback, recovery, and protected GC.

StatefulSet, SSR runtimes, and custom domains remain explicit future work and are not represented as complete.

## Merged G9 increments

| Increment | PR | Merged main evidence |
|---|---:|---|
| HTTP test discovery split | 10 | `3a04b7f` |
| Run mutation service | 11 | `c384aac` |
| StartRun service | 12 | `cd7b8b3` |
| DesignProfile pure domain | 13 | `229e784` |
| DesignProfile application service | 14 | `2431e63` |
| Artifact/Runtime storage boundary | 15 | `16479a3` |
| Preview/application authorization | 16 | `cc828cb` |
| Internal/ReleaseEvidence split | 17 | `e953a35` |

## Merged-main Rust and architecture gates

Executed from the clean `e953a350b45d` worktree:

```text
cargo fmt --check                                      passed
cargo clippy --all-targets --all-features -D warnings  passed
cargo test --all-targets                               passed
  core library                                         107 passed
  HTTP integration                                      85 passed, 1 real-provider opt-in ignored
  Astro/Fumadocs integration build                       5 passed
  remaining Runtime integration suites                  passed
strict Sandbox architecture                            passed
remote workspace filesystem boundary                   passed
Artifact manifest architecture                         passed
Runtime bootstrap architecture                         passed
Release packaging architecture                         passed
Publication control-plane architecture                 passed
HTTP API architecture                                  passed
```

Key final boundaries:

```text
http_api/mod.rs                                         279 lines
http_api/routes/internal.rs                              16 lines
http_api/routes/artifacts.rs                             66 lines
http_api/routes/previews.rs                             145 lines
HTTP Cargo-discovered inventory                         86 tests
```

## Real Authoring Sandbox and lifecycle gate

Command:

```text
bash infra/agent-sandbox/run-k8s-e2e.sh
```

Machine evidence:

```text
repositoryCommit=e953a350b45d
repositoryDirtyFiles=0
cluster=zerondesign-e2e
kubeContext=k3d-zerondesign-e2e
sandboxImage=anydesign/astro-website-sandbox:e953a350b45d
sandboxImageID=sha256:cd53dcf378ae3afb62be99bca2d5efe31ca3568bcad882965da9ce10b4c74880
authenticatedWorkspaceChannel=true
parallelWorkspaceIsolation=true
mtlsVerified=true
rotationWindowVerified=true
```

The first attempt exposed an environment-only Verdaccio pull timeout. The retry imported the exact locked multi-architecture digest
`sha256:44af8dec4b8bfb9b940263f56ee5f371484515e4397eea56ab9c942500ab9dfa` from an existing local k3d content cache; no image version, digest, admission rule, or test was changed. The clean gate then passed.

Dual project lifecycle evidence after Sandbox release:

| Project | Kind | Artifact manifest | Lease after release | Artifact HTTP after release |
|---|---|---|---|---:|
| `website-k3d` | website | `98e948119cbc016edc2990f60e2034b229c95f7b8fb19498ae585d464950a130` | `stopped` | 200 |
| `docs-k3d` | docs | `133f715e6a8e9f16e7962b892ccbb255cba826148b8fbccf19bf40ad3b11aa78` | `stopped` | 200 |

Both projects recorded valid PreviewUpdated-before-RunCompleted ordering, distinct Pod UIDs, screenshot hashes, source snapshots, CAS before/after versions, and immutable Artifact manifests.

## Real Published Runtime gates

### G7 Publish/Unpublish/Ingress

```text
command=bash infra/public-runtime/run-g7-k3d-e2e.sh
cluster=zerondesign-g7
host=w-b593f044367151d27f6c.g7.test
release=release-6366fc85f07f120de99d8b388dcbfaba
digest=sha256:c09150161629464a2f93a7c464991c9e9df5f19631125b930381315ceef7348f
result=passed
```

This gate verified real TLS ingress, stable per-work host, Publish, Unpublish, Republish, idempotency, release identity, and external availability ordering.

### G8 Blue/Green/Rollback/Recovery

```text
command=bash infra/public-runtime/run-g8-k3d-e2e.sh
cluster=zerondesign-g8
host=w-2dde6c060e32e3978978.g8.test
releaseA=release-6e2fc7bac9ff969db25cfc510aa854c3
digestA=sha256:d8b2c64f680dc068cea917388fa264ab2c370ec4dbbcc11d3436f51094daa34d
releaseB=release-454e623cce1ec899c9ff9b68a550021d
digestB=sha256:2a34ec07e4e77da357eef052c14ff02fa3c0ff4f6f73595525e97a9766728d4a
result=passed
```

This gate verified release-specific green deployment, identity probing, stable Service switch, EndpointSlice convergence, failed-switch restoration of blue, successful update, Rollback, and restart recovery.

## Final architectural judgment

The original oversized HTTP implementation has been replaced by explicit inbound adapters, application services, domain modules, and infrastructure ports. Framework-specific Astro/Fumadocs compatibility remains isolated from generic route dispatch. New static templates use registry capabilities and Artifact manifests without adding framework-global HTTP routes. Authoring Sandbox identity and Published Runtime identity are separate, and external Ingress targets stable per-work Services rather than Sandbox endpoints.

The master plan can therefore be marked complete for its declared scope.
