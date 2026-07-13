# G4 WorkRelease and OCI packaging evidence

Date: 2026-07-12

Decision: `approved_for_local_acceptance`

This evidence closes the local G4 acceptance path only. It does not approve a
production Registry, signing identity, vulnerability exception process, or
Kubernetes Published workload.

## Frozen toolchain

| Capability | Frozen local input |
|---|---|
| Registry | CNCF Distribution `registry:3` at `sha256:1be55279f18a2fe1a74edf2664cac61c1bea305b7b4642dab412e7affdcb3e33`, bound to `127.0.0.1:5001` |
| Builder | Buildx named builder `anydesign-release-g4`, driver `docker-container`, BuildKit v0.30.0 at `sha256:0168606be2315b7c807a03b3d8aa79beefdb31c98740cebdffdfeebf31190c9f` |
| Base image | `nginx:stable-alpine`, platform `linux/arm64/v8`, manifest `sha256:53cadfbebeffa241f12333cf8a63f3c6553eedad8b9f8296de89e32c566a5caa` |
| SBOM | Syft 1.46.0, SPDX JSON |
| Scan | Trivy 0.72.0, policy `trivy-critical-secret-v1` |
| Scan DB | Trivy DB v2, updated `2026-07-12T07:28:36Z`, OCI layer `sha256:b27122619190be0337d26c4c52a3e0a37af01fd0bd35ec3eee982f1805372186` |
| Signing | Cosign 3.1.1 local acceptance key, no transparency log service |

Source and checksum locks are in `infra/published-runtime/images.lock.json`.
The Runtime invokes one absolute helper through `ProcessReleasePackagingBackend`;
the helper digest is revalidated before every operation, inherited environment
is cleared, output and deadline are bounded, and no shell fragment is accepted.

## Real isolated packaging result

The successful run used a fresh Registry repository and the named isolated
builder:

```text
releaseId: release-cfa84e4b585684fe82537dae020758f9
packagingId: packaging-cfa84e4b585684fe82537dae020758f9
status: validated
attempts: 1
baseImageDigest: sha256:53cadfbebeffa241f12333cf8a63f3c6553eedad8b9f8296de89e32c566a5caa
imageDigest: sha256:6a1b37761f926fd6b20a46fadcf4b8ded9b41c106735a004d539d747bf55cd82
sbomDigest: sha256:4460c41410d2c030d1515e9b567f4676f6c72082cab2200d6358437f097bcbdc
provenanceDigest: sha256:54a19a7be46f1079e698cce2f5343d71032fce74e505b11dbba9f3c48d8d0b73
scanReportDigest: sha256:8318e6bfbda2af47dc17fe4e9385427d940168056736844dca5294da843f8884
scan: passed; critical=0; high=4; secrets=0
signatureIdentity: local-cosign-key:sha256:7cfc866f94253ffe319c9605cd267ecf44ed57c283dfc4ed84ef0a524bd630f6
signatureDigest: sha256:e99876895eaf0e343bf8e3a7291a1c38648a865181bb89677a27d6c6c097f307
```

Cosign verification bound the signature to the same repository and immutable
manifest digest. Registry `Docker-Content-Digest`, Buildx build metadata, push
metadata, WorkRelease, and the running container all reported the same image
digest.

## Runtime smoke result

The image was pulled from the local Registry by digest and started as a normal
Docker container; no Kubernetes resource was created.

```text
GET /                                      -> 200
GET /.well-known/anydesign/healthz         -> 200 {"status":"ok"}
GET /.anydesign/runtime-manifest.json      -> 404
container user                             -> 101:101
container image                            -> sha256:6a1b37761f926fd6b20a46fadcf4b8ded9b41c106735a004d539d747bf55cd82
```

`kubectl get deployment,statefulset,service,ingress -A -l
anydesign.io/work-release -o name` returned no resources.

## Failure, recovery, and GC gates

- Unit suite: `19 passed`, including stable idempotency, journal recovery,
  truncated journal recovery, Registry conflict, push-before-store crash,
  scan rejection, sign crash resume, immutable digest/signature, and failed-only GC.
- Real setup failures in base-image resolution, Registry query, initial Trivy DB
  retrieval, OCI scan input, and Cosign v3 configuration never produced a
  Validated record. Reconciliation resumed from persisted state without a second
  Registry mutation.
- Failed packaging GC requires exact digest match and complete Registry/evidence
  deletion proof. Validated releases are ineligible; desired/active/rollback
  reference protection remains a G5/G8 responsibility.
- `services/runtime/scripts/check-release-packaging-architecture.sh` passed.
- Agent/Sandbox source contains no Registry push or signing credential path.

## Production stop conditions retained

- Replace loopback HTTP Distribution with TLS, authentication, immutable/delete
  policy, audit, and managed retention.
- Replace the local Cosign key with approved KMS/Keyless identity and verification
  policy; decide Rekor/private transparency behavior.
- Operate a versioned, freshness-enforced Trivy DB mirror and a documented
  vulnerability exception/EOL policy.
- Use a separately authorized builder identity and Registry credential provider.
- Do not create Deployment, Service, or Ingress until G5/G6/G7 control-plane
  contracts and gates are complete.

## 2026-07-13 Runtime API Freeze revalidation

The current worktree was revalidated with a fresh Registry repository rather
than by reading the earlier validated record:

```text
repository: localhost:5001/anydesign/work-releases-freeze-audit
releaseId: release-cfa84e4b585684fe82537dae020758f9
status: validated
attempts: 1
helperSha256: 8525fb794f15979b8c4a0e2f5d1f5be09335eeb6cf8f1b5ca0f5c48722933152
imageDigest: sha256:7b7a03b301a91db7d97c9dbb04651f2741f0c128db82152d2ce937e1d0d8d99b
sbomDigest: sha256:19dd10593b3249677336157767b5765e209ff4f9745f6758579ca5a9ee0d53a0
scanReportDigest: sha256:d4fdb25712b455d7160f8cc9a6dc75d20730e7ff721359d27fdcd7e982fd1fdc
scan: passed; critical=0; high=4; secrets=0
signatureIdentity: local-cosign-key:sha256:4adce916bcaf857f1f74985f94d8803c0379ad9ef57bf7dd9eead63d626c78c4
signatureDigest: sha256:3d2de73f673a0cd86cde76b7763b172dfcccc422bce9ff5bee5aa8c3f137e8a9
```

Independent checks then read the Registry manifest, verified one Cosign
signature against the generated public key, pulled the image by immutable
digest, and started a clean container:

```text
Registry Docker-Content-Digest -> sha256:7b7a03b301a91db7d97c9dbb04651f2741f0c128db82152d2ce937e1d0d8d99b
Cosign verified signatures      -> 1
GET /                           -> 200
GET /.well-known/anydesign/healthz -> 200 {"status":"ok"}
GET /.anydesign/runtime-manifest.json -> 404
container user                  -> 101:101
container image                 -> sha256:7b7a03b301a91db7d97c9dbb04651f2741f0c128db82152d2ce937e1d0d8d99b
```

This remains local acceptance: the signature intentionally uses the frozen
no-Rekor configuration and verification therefore requires
`--insecure-ignore-tlog`. The production KMS/Keyless and transparency policy
stop condition above remains open.
