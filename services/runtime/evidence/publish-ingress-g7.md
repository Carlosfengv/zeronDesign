# G7 Publish, Unpublish, and Ingress Evidence

Date: 2026-07-12

Branch: `codex/runtime-architecture-g7`

Cluster/context: `zerondesign-g7` / `k3d-zerondesign-g7`

Namespace: `anydesign-works`

## Implemented boundary

- Initial Publish requires `Idempotency-Key`, `If-None-Match: *`,
  `expectedGeneration`, a promoted version, and a validated digest-pinned
  WorkRelease.
- Update/Rollback/Unpublish preconditions require a quoted `If-Match` equal to
  `expectedCurrentReleaseId`; CAS conflicts do not allocate a new generation.
- The controller enables Ingress only after release-specific Deployment
  availability, isolated internal release probing, and stable Service apply.
- Each work owns a random persisted host slug, stable ClusterIP Service,
  per-work TLS Ingress, and NetworkPolicy. A namespace-wide scan rejects a host
  already claimed by another Ingress.
- External HTTPS verification requires both the release endpoint body and the
  `X-AnyDesign-Release-Id` header to match the desired release before operation
  status becomes `completed` and runtime status becomes `published`.
- Unpublish deletes the Ingress first, observes three consecutive closed-route
  results, then deletes Service, all controller-owned Deployments, and the
  per-work NetworkPolicy. Release history and host identity remain.
- Republish recreates Kubernetes resources with new UIDs while retaining the
  exact same public host.
- Published responses include `nosniff`, `Referrer-Policy`, HSTS, and immutable
  release identity headers. The Runtime ServiceAccount still cannot read TLS
  Secrets.

## Real TLS lifecycle gate

Command:

```text
bash infra/public-runtime/run-g7-k3d-e2e.sh
```

Result:

```text
test publish_unpublish_and_republish_keep_stable_https_host_on_k3d ... ok
G7 k3d gate passed: cluster=zerondesign-g7
host=w-6660e76e635e2d45da57.g7.test
release=release-6366fc85f07f120de99d8b388dcbfaba
digest=sha256:317693bc7c2289fac3e27f43e012ef8d93b4f501872e614b8b7e425520bc652b
```

The executable gate proves:

1. no work Ingress exists before reconciliation;
2. a foreign Ingress claiming the reserved host causes `reconcile_required`
   and is never adopted;
3. after the conflict is removed, HTTPS serves only the desired release and
   required security headers through Traefik and a wildcard test certificate;
4. Unpublish closes the host before deleting internal resources;
5. Deployment, Service, Ingress, and per-work NetworkPolicy are absent at the
   Unpublished checkpoint;
6. Republish returns the same host with new Kubernetes UIDs and the same
   validated release identity;
7. Ingress backend always names the stable work Service, never an Authoring
   Sandbox Service.

## Production boundary

The gate uses a local CA, `*.g7.test`, a dedicated k3d load balancer, and K3s
Traefik. Production requires approved public DNS, wildcard certificate
provisioning/rotation, ingress-class redirect and security policy, a
cryptographic image-verification admission provider, and monitoring before
public rollout. G8 still owns bounded blue/green switch windows, automatic
rollback, EndpointSlice convergence evidence, and Registry GC; this G7 evidence
does not claim those later guarantees.

## Verification commands

```text
cargo test --manifest-path services/runtime/Cargo.toml --all-targets
cargo clippy --manifest-path services/runtime/Cargo.toml --all-targets -- -D warnings
bash services/runtime/scripts/check-publication-control-plane-architecture.sh
kubectl apply --dry-run=server -f infra/public-runtime/base.yaml
bash infra/public-runtime/run-g7-k3d-e2e.sh
```
