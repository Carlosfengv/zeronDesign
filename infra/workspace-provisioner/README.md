# Workspace Provisioner

`provision-workspace.sh` idempotently creates one Kubernetes Namespace as one
zeronDesign Workspace. It installs quota, limits, default-deny networking,
namespace-scoped Runtime RBAC, Sandbox templates, and zero-replica pool objects.

```bash
RUNTIME_SYSTEM_NAMESPACE=anydesign-runtime \
  WORKS_INGRESS_NAMESPACE=kube-system \
  bash infra/workspace-provisioner/provision-workspace.sh ws-team-a
```

The Agent Sandbox `v1beta1` API requires a `warmPoolRef`; the installed pool
objects therefore remain as template indirection with `replicas: 0`. They do
not keep pre-warmed Pods. A `SandboxClaim` cold-creates a Sandbox when no pooled
Sandbox is available.

Before running Builds, provide namespace-specific verifier and TLS manifests
through `WORKSPACE_CHANNEL_VERIFIER_MANIFEST` and
`WORKSPACE_CHANNEL_TLS_MANIFEST`. Production provisioning should source these
from the platform certificate issuer rather than copying private keys between
Workspace namespaces.

For a greenfield development cluster, generate a shared Runtime client identity
and one namespace-bound Sandbox server identity per Workspace:

```bash
bash infra/workspace-provisioner/configure-workspace-channel.sh \
  ws-team-a ws-team-b
```

The helper refuses to overwrite existing credentials unless
`ROTATE_WORKSPACE_CHANNEL_KEYS=true` is explicitly set. Production should use
the platform certificate issuer instead of this development helper.

For a fresh, non-reused k3d control-plane smoke environment:

```bash
bash infra/workspace-provisioner/bootstrap-k3d-greenfield.sh
```

The bootstrap intentionally fails if its cluster name already exists, creates
two Workspace Namespaces, and verifies that neither keeps resident Sandbox
Pods. It is a provisioning smoke test; Runtime, Provider, PostgreSQL, and the
full Build/Publish isolation gate remain separate deployment steps.

After importing the Sandbox image into the development cluster, run the
repeatable isolation gate:

```bash
bash infra/workspace-provisioner/verify-workspace-isolation.sh \
  ws-greenfield-a ws-greenfield-b
```

The gate cold-starts one real `SandboxClaim`, proves that the second Workspace
does not receive its Sandbox, Pod, Service, or PVC, checks namespace-scoped
RBAC and the disabled service-account token mount, and performs a Runtime to
Sandbox mTLS handshake with the namespace-bound SPIFFE identity. Its temporary
Claim, Pod, and PVC are removed on exit.

To package the latest generated Website and Docs artifacts, publish them into
their own Workspace Namespaces, expose test-only `.localhost` URLs through
Traefik, and verify the URLs across a Runtime restart:

```bash
bash infra/workspace-provisioner/run-published-work-e2e.sh
```

The command prints both URLs and writes `published-works.json` under the Runtime
E2E evidence directory. The generated Deployments and stable Services remain in
the cluster so the URLs stay available until the k3d cluster is deleted.

Promote the same generated Work releases through the production HTTPS Ingress
state machine and verify external Release identity:

```bash
bash infra/workspace-provisioner/run-published-work-g7-e2e.sh
```

This creates a short-lived local test CA, installs the same wildcard TLS Secret
name independently in each Workspace, and records only the public CA
certificate. The CA private key is not persisted; each server private key is
stored only in that Workspace's Kubernetes TLS Secret and is never written to
the evidence directory. The resulting control-plane state is `Published`, not
merely `workload_ready`.

To deploy the local PostgreSQL control plane, migrate the existing Runtime
control-plane cache, prove database-authoritative restoration, restart both
Runtime and PostgreSQL, and recheck the HTTPS releases:

```bash
bash infra/workspace-provisioner/run-postgres-control-plane-e2e.sh
```

The gate moves mirrored local cache files into a recoverable directory on the
Runtime PVC before restart; it does not remove generated artifacts or design
source blobs. PostgreSQL restores the cache, and the gate compares the restored
`project-access.jsonl` SHA-256 with the authoritative database digest. Database
credentials stay in the `anydesign-runtime-postgres` Kubernetes Secret and are
never written to evidence. For a faster repeat run, an already imported image
can be supplied through `RUNTIME_POSTGRES_E2E_REUSE_IMAGE`.

To deploy the local S3-compatible object store, migrate Runtime Artifact and
Evidence objects, prove object-store-authoritative cache restoration, restart
both Runtime and MinIO, and recheck the HTTPS releases:

```bash
bash infra/workspace-provisioner/run-object-storage-e2e.sh
```

The object boundary is limited to `artifacts`, `source-snapshots`,
`validation-reports`, `acceptance-reports`, and `screenshots`. PostgreSQL
control-plane files and Workspace files are not copied into the bucket. Before
the restore test, the five local cache directories are moved to a recoverable
directory on the Runtime PVC. Object-store credentials remain only in the
`anydesign-runtime-object-storage` Kubernetes Secret. A repeat run may reuse an
already imported image through `RUNTIME_OBJECT_STORAGE_E2E_REUSE_IMAGE`.

To deploy the Web product catalog against its own PostgreSQL database, apply an
explicit versioned migration, bootstrap the current published projects, and
verify the Web/Runtime transaction boundary and restart recovery:

```bash
bash infra/workspace-provisioner/run-web-product-postgres-e2e.sh
```

The Web application uses database `zerondesign_web` and a DML-only application
role with the same name. It does not share Runtime's `anydesign_runtime`
database schema or allow production auto-migration. The gate proves that a
project rejected by a disabled Workspace never reaches Runtime, while a
successful project registration is present in both catalogs. PostgreSQL and
Web are then restarted and the ordered product-catalog digest must remain
unchanged. Credentials stay in the `zerondesign-web-product-postgres`,
`zerondesign-web-auth`, and `zerondesign-web-runtime-principal` Kubernetes
Secrets. The Runtime RC gate creates the last Secret from the same Ed25519 key
pair whose public half Runtime verifies. A repeat run may reuse an imported
image through `WEB_PRODUCT_POSTGRES_E2E_REUSE_IMAGE`.

To replace manually copied Published Work TLS Secrets with cert-manager-owned
per-Work exact-host certificates, exercise private-key rotation, and verify
HTTPS after a cert-manager controller restart:

```bash
bash infra/workspace-provisioner/run-cert-manager-tls-e2e.sh
```

The retained k3d cluster runs Kubernetes 1.31, so this gate defaults to
cert-manager `v1.19.5`, the final security patch compatible with that cluster.
Production must use a supported Kubernetes release and a currently supported
cert-manager release. The local `zerondesign-works-ca` ClusterIssuer is only an
E2E implementation of the production issuer contract; production must replace
it with ACME, Vault, or a managed cloud CA. The root private key remains in the
`cert-manager` namespace. Runtime annotates every Published Work Ingress with
the ClusterIssuer and references `<work-name>-tls`. cert-manager's ingress shim
then creates one exact-host `Certificate` and ECDSA private key per Work, with
automatic renewal and `rotationPolicy: Always`. This avoids ambiguous SNI
selection when multiple Workspace namespaces publish below the same base
domain.

For normal Runtime deployment, set `WORKS_CERTIFICATE_ISSUER_NAME` to the
platform ClusterIssuer together with `WORKS_BASE_DOMAIN` and
`WORKS_INGRESS_CLASS`. `WORKS_TLS_SECRET_NAME` is only the legacy compatibility
path when no certificate issuer is configured. Workspace provisioning does not
create publication certificates.
