# Generation Reliability Matrix

This directory provides the single entry point for the local and CI Website/Docs
generation gate. It composes the existing Agent Sandbox bootstrap and Runtime RC
gate instead of maintaining a second deployment implementation.

Run the deterministic fixture matrix:

```bash
bash infra/generation-reliability/run-k3d-matrix.sh
```

Reuse an existing cluster and Runtime image:

```bash
GENERATION_MATRIX_BOOTSTRAP=reuse \
GENERATION_RUNTIME_IMAGE=anydesign/runtime:reliability-m6-20260716 \
GENERATION_MATRIX_SKIP_PREFLIGHT=1 \
bash infra/generation-reliability/run-k3d-matrix.sh
```

Run the legacy direct-Provider connectivity diagnostic without putting a key on the command line:

```bash
chmod 600 /path/to/provider.env
GENERATION_MATRIX_MODE=real \
GENERATION_PROVIDER_ENV_FILE=/path/to/provider.env \
bash infra/generation-reliability/run-k3d-matrix.sh
```

The env file accepts only:

```text
DEEPSEEK_API_KEY=...
DEEPSEEK_BASE_URL=https://api.deepseek.com
DEEPSEEK_E2E_MODEL=deepseek-v4-pro
```

This `real` matrix mode is audit-only and does not qualify as canary, paired-cohort, or release evidence.
`GENERATION_MATRIX_RC_MODE=release` fails closed; use the governed five-case runner below for formal real-provider
evidence.

The matrix allocates an unused localhost port for its Runtime port-forward.
Set `GENERATION_MATRIX_RUNTIME_PORT` only when a stable port is required; an
occupied explicit port fails closed instead of connecting to an unrelated
local service.

Run the five governed real Website/Docs examples against an existing k3d
cluster and Provider Gateway:

```bash
GENERATION_REAL_CLUSTER=zerondesign-e2e \
GENERATION_REAL_WORKSPACE_NAMESPACE=ws-real-provider-website \
bash infra/generation-reliability/run-real-provider-examples.sh
```

`GENERATION_REAL_WORKSPACE_NAMESPACE` is required and must identify an existing
managed Workspace namespace. The runner persists that namespace in project
access and evidence; it never falls back to a shared sandbox namespace.

The manifest is `real-provider-cases.json`. It contains three Website and two
Docs prompts written as ordinary user requests: audience, desired content, and
visual or documentation intent only. Internal tool choreography and acceptance
protocols are not injected into the prompt. The configured token total is a
batch safety ceiling rather than a usage target; Token consumption is not a
pass criterion, and evidence records actual input and output usage only for
cost and diagnostic traceability.
Use `GENERATION_REAL_CASE_IDS=id-a,id-b` for a targeted regression subset,
`GENERATION_REAL_RUN_TIMEOUT_MS` to override the default 15-minute total Run
timeout, and `GENERATION_REAL_RUN_IDLE_TIMEOUT_MS` to override the default
8-minute event-stream idle timeout. Before any paid Run, the runner reconciles
the mounted `deepseek-v4-pro` declaration, verifies current resource revision 4, and runs
a minimal readiness probe. Set `GENERATION_REAL_PROVIDER_READINESS_PROBE=0`
only for an offline diagnostic that must not qualify as release evidence. The
runner restores the original Runtime budgets and public-principal Secret on exit.
Retryable Provider terminal failures are retried per case after a 45-second
circuit-cooldown window, with at most three case attempts. Each retry uses a new
project ID while retaining every failed Run, attempt status, and actual Token
usage in the suite evidence. Override these bounded defaults with
`GENERATION_REAL_MAX_CASE_ATTEMPTS` and
`GENERATION_REAL_CASE_RETRY_COOLDOWN_MS`; content rejection, no-progress, and
budget failures are never retried as Provider transients.

The no-secret local source of truth is
`infra/provider-gateway/model-resources.deepseek-v4-pro.json`. A successful run
writes `provider-resource-reconcile.json` plus one result-named suite directory:
`suite-<id>-accepted`, `suite-<id>-rejected`, or `suite-<id>-failed`. Run events
are streamed incrementally to NDJSON; the suite summary uses
`generation-real-provider-suite-evidence@2` and records the commit, dirty flag,
Provider config digest, resource revision, manifest digest, and acceptance-probe
digest.

Before each paid Build, the runner uses the internal Producer identity to record and verify an automated fixture Content
Plan Approval bound to the exact case intent hash, route and acceptance-text hash. Control and Generation Context
candidate deployments therefore receive the same approved Plan identity; no human approval is required for this harness,
no public Principal can write the Approval, and no raw prompt is copied into the approval evidence.

Verify the deployed Wave -1 Producer, invalidation ordering and restart recovery independently of a paid model run:

```bash
bash infra/generation-reliability/verify-content-plan-approval-readiness.sh
```

The probe reads the Runtime internal-admin Secret into a mode-0600 temporary curl config, never prints or persists the
token, and does not read or rotate the public Principal Secret. Its evidence is written under
`services/runtime/target/e2e-evidence/<cluster>/content-plan-approval-readiness.json`.

Audit the complete authoritative legacy Brief snapshot before enabling a candidate cohort:

```bash
bash infra/generation-reliability/audit-content-plan-approval-migration.sh
```

The audit verifies the PostgreSQL file revision and SHA-256 before classifying the latest state of every Brief. Evidence
contains only anonymous identity hashes and reasons; raw Brief content is streamed to the auditor and is not persisted.
Only confirmed records carrying `planId + revision + contentHash + confirmationEventId` are mappable. Brief confirmation,
Acceptance evidence or successful Build evidence never creates a verified Approval by inference.

The five-case suite is a governed functional canary, not the quantitative Generation Context exit cohort. For the
paired gate, deploy fixed `off` and `enabled` Runtime variants that allow the same Gateway Model Resource snapshots,
using `deploy-generation-context-cohort-runtimes.sh`, then use
`generation-context-paired-cohort-ledger.mjs` as documented in
`generation-context-runbook.md`. The Runtime efficiency endpoint supplies the source metrics; the sample mapper removes
Project/Run identity before the hash-chained ledger accepts a side. The release harness can consume the ledger through
`RUNTIME_E2E_GENERATION_CONTEXT_COHORT_LEDGER`; undersized or incomplete cohorts fail closed.
The deployment script creates separate PostgreSQL databases, object-storage prefixes, PVCs and public-Principal Secrets
for control and candidate. It validates that the primary Runtime still references its original resources. The candidate
enables Generation Context while Content Plan attestation remains in Shadow; exact Producer Approval is still required
and Generation Context enforcement fails Build closed when that Approval is absent or stale.
Use `run-generation-context-paired-pair.sh` for fixed-session Greenfield and Warm pairs,
`run-generation-context-cold-dev-pair.sh` for the Draft-specific Cold Dev lifecycle, and
`run-generation-context-repair-pair.sh` for the Fumadocs Version→Review finding→scoped Repair lifecycle. Each runner
executes both deployments with the same fixture inputs and atomically appends validated control/candidate evidence. Cold Dev accepts
only website fixtures, proves a dependency-impacting EditImpactPlan, managed Dev restart/readiness and durable
DraftSnapshot, and does not substitute a Production Build for readiness.
Deploy the enabled candidate only after the historical Approval migration audit passes. Keep the primary Runtime in
Generation Context/attestation Shadow mode until the paired quantitative gates pass.

Publish one accepted immutable Artifact as an isolated k3d validation Work:

```bash
GENERATION_REAL_CLUSTER=zerondesign-greenfield \
GENERATION_REAL_WORKSPACE_NAMESPACE=ws-greenfield-b \
GENERATION_REAL_PROJECT_ID=real-project-id \
GENERATION_REAL_VERSION_ID=version-id \
GENERATION_REAL_RUN_ID=run-id \
GENERATION_REAL_EXPECTED_TEXT='Expected title' \
GENERATION_REAL_PUBLICATION_PATH=/docs/ \
bash infra/generation-reliability/publish-real-provider-validation.sh
```

`GENERATION_REAL_PUBLICATION_PATH` defaults to `/` and supports static-export
routes such as `/docs/`. The script verifies the Artifact manifest, builds the
pinned-base static runtime, creates an isolated Deployment/Service/Ingress with
an exact-host cert-manager Certificate, and checks HTTPS content plus Release
identity. Its evidence intentionally records `publicationMode=validation` and
`productReleaseApiCompleted=false`; it does not replace the product Release API
or a digest-pinned Release Packager deployment.

Successful runs write `generation-matrix-summary.json`. Failed runs write a
redacted `failure-*` directory containing cluster state, events, and bounded
container logs. Secret objects and raw environment variables are never
collected.
