# G5 publication control-plane evidence

Date: 2026-07-12

Scope: persistent publication intent and query control plane only. G5 does not
create or mutate Kubernetes Deployment, StatefulSet, Service, NetworkPolicy, or
Ingress resources.

## Frozen contracts

- `publish-operation@1`
- `work-runtime-state@1`
- `publication-outbox@1`
- executable HTTP route manifest entries for Publish, Unpublish, Rollback,
  deployment-state, releases, and operation query

Every mutation requires `Idempotency-Key` plus `expectedGeneration` CAS.
Update/Rollback additionally require `expectedCurrentReleaseId`. Runtime stores
only the key hash and a length-prefixed canonical request hash.

## Atomic persistence and recovery

One append-only `PublicationCommit` contains all three authoritative records:

```text
PublishOperation
WorkRuntimeState desired publication/release/generation
PublicationOutboxEvent
```

The journal record is flushed before the in-memory state and checkpoint advance.
Recovery replays committed journal records after a missing/stale checkpoint and
ignores a truncated final record. There is no state in which an API-visible
Operation exists without its desired state and outbox event.

Delivered events whose operation remains nonterminal and whose
`observedGeneration` trails `desiredGeneration` are returned to Pending during
startup recovery. Delivery attempts use bounded retry time through
`nextAttemptAt`.

## Controller ownership

Runtime bootstrap registers exactly one Supervisor-owned task:

```text
controller/work-runtime
```

The G5 backend is deliberately `ControlPlaneOnlyBackend` and returns Deferred.
The outbox remains recoverable until G6 supplies the Kubernetes backend port.
No Agent/Sandbox receives publication controller, Registry, or Kubernetes
credentials.

## HTTP and authorization behavior

- mutation target must be a Validated WorkRelease for the same project and
  RuntimeProfile;
- the WorkRelease ProjectVersion must still be Promoted;
- same key and request returns the same Operation;
- same key with a different request returns HTTP 409;
- publication read/write scopes are distinct short-lived public-principal
  operations;
- when public auth is required, the principal must own the project;
- all publication handlers live in `src/http_api/routes/publication.rs`.

## Verification gates

- publication domain unit tests cover idempotency conflict, concurrent CAS,
  atomic journal recovery, truncated-tail recovery, outbox delivery, startup
  replay, and Supervisor ownership;
- `tests/publication_api.rs` covers real Axum request/response persistence,
  query routes, validated-release checks, required Idempotency-Key, and scoped
  owner authorization;
- HTTP executable route manifest gate passes;
- `check-publication-control-plane-architecture.sh` passes and rejects any G5
  Kubernetes/container command dependency;
- `cargo test --all-targets` passed after the publication changes and the
  existing run-log failure-injection regression was corrected;
- `cargo clippy --all-targets -- -D warnings` passed;
- `kubectl get deployment,statefulset,service,ingress -A -l
  anydesign.io/work-release -o name` returned no resources.
