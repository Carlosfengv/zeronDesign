# Published runtime packaging infrastructure

This directory contains the trusted, template-independent runtime image inputs.
Template adapters produce a verified artifact manifest; they do not select the
runtime image or inject web-server configuration.

`static-web/` packages any artifact conforming to `artifact-manifest@1` and
`runtime-manifest@1` profile `static-web-v1`. Published files are copied to
`/srv/www`; immutable release metadata is copied to `/.anydesign` and is denied
by the web server. The container runs as UID/GID 101 and listens on port 8080.

All external images are executed by digest from `images.lock.json`. Updating a
source tag without resolving and reviewing its new multi-platform digest is not
an accepted upgrade.

The G4 local acceptance path uses the exact Syft, Trivy, and Cosign versions in
the same lock file. On macOS they can be installed with
`brew install syft trivy cosign`; the helper rejects a version mismatch. Release
CI must install artifacts from the listed upstream release and validate the
listed platform checksum before exposing the tool directory to Runtime. The
helper itself is independently SHA-256 pinned by
`ProcessReleasePackagingBackend` for every operation.

The local acceptance Registry is intentionally HTTP-only on loopback. Shared
or production registries must use TLS and workload identity; Registry or signing
credentials are never passed into an Agent Sandbox.

The local builder must be a named `docker-container` instance created with the
digest-pinned BuildKit image and `buildkitd.local.toml`. The in-container
Registry alias is used only to cross the Docker Desktop VM boundary; persisted
release references retain the caller-visible Registry host.

## G6 Kubernetes runtime

`infra/public-runtime/base.yaml` establishes the shared `anydesign-works`
namespace, controller RBAC, Pod Security enforcement, quota, default-deny
networking, isolated Release Prober policy, and the native admission baseline.
It intentionally contains no Ingress.

Runtime enables the production adapter only when
`WORK_RUNTIME_BACKEND=kubernetes`. `WORK_RUNTIME_PROBER_IMAGE` is then required
and must be an immutable `repository@sha256:...` reference to the approved
probe image. The Prober Pod receives no service-account token or Kubernetes
mutation permission and is deleted with its release-specific ClusterIP Service
after the health and release-identity checks complete.

Run `infra/public-runtime/run-g6-k3d-e2e.sh` for the real dual-work gate. The
gate uses a dedicated k3d cluster and Registry, verifies cross-work traffic is
denied, exercises controller restart and UID drift, and fails if any Ingress is
present.

For public G7 exposure, set `WORK_RUNTIME_EXPOSURE=ingress` together with
`WORKS_BASE_DOMAIN`, `WORKS_INGRESS_CLASS`, and `WORKS_TLS_SECRET_NAME`.
External verification always uses HTTPS. `WORKS_PROBE_RESOLVE` and
`WORKS_PROBE_CA_FILE` are optional deployment/test routing inputs; production
normally relies on public DNS and the system trust store. The Runtime identity
may create/read/delete Ingress objects but still cannot read the wildcard TLS
Secret.

Run `infra/public-runtime/run-g7-k3d-e2e.sh` for the real TLS lifecycle gate.
It verifies host collision rejection, external release headers, security
headers, ordered Unpublish, complete workload teardown, and Republish using the
same random host identity.

Run `infra/public-runtime/run-g8-k3d-e2e.sh` for release-specific Update and
Rollback. It verifies Service selector CAS, exclusive EndpointSlice convergence,
restart after a selector-switch checkpoint, automatic blue restoration on a
bounded convergence timeout, and the Runtime/Kubernetes Registry GC protection
snapshot. Operational recovery steps are in
`infra/published-runtime/blue-green-runbook.md`.
