# Generation Reliability Matrix

This directory provides the single entry point for the local and CI Website/Docs
generation gate. It composes the existing Agent Sandbox bootstrap and Runtime RC
gate instead of maintaining a second deployment implementation.

Replay a sanitized Runtime evidence bundle without network or Provider access:

```bash
node infra/generation-reliability/replay-evidence.mjs \
  services/runtime/evidence/replay/fixture-docs-route-failure
```

Run the Node implementation against the same route conformance corpus consumed
by the Rust Route Oracle:

```bash
node infra/generation-reliability/replay-evidence.mjs --conformance \
  services/runtime/evidence/replay/contracts/artifact-route-conformance@1.json
```

The replay fails closed on missing inputs, checksum drift, raw sensitive fields,
per-turn usage divergence, `RunModelUsage@1` projection drift, route-probe
divergence, failure-owner drift, or progress-ledger drift. The committed fixture
is synthetic and contains no real prompt, source, credential, or screenshot
content.

Real-provider runners persist a redacted event projection: agent messages, tool output, terminal summaries, and tool
errors are replaced with SHA-256 plus byte counts; Provider request IDs become presence booleans. Lifecycle prompts are
stored only as `promptSha256` and byte counts. Raw prompt, source, command output, and credential material must not enter
the evidence directory.

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

For evidence intended to enter a release gate, first prepare a frozen cohort session from a clean commit, then run:

```bash
GENERATION_REAL_EVIDENCE_MODE=release \
GENERATION_REAL_PREPARED_SESSION_DIR=/absolute/path/to/frozen-session \
GENERATION_REAL_CLUSTER=zerondesign-e2e \
GENERATION_REAL_WORKSPACE_NAMESPACE=ws-runtime-rc \
bash infra/generation-reliability/run-real-provider-examples.sh
```

Release mode exits before Provider reconciliation or paid Runs when the worktree is dirty, the prepared session is
missing, its frozen source commit differs, or the Provider readiness probe is disabled. The default `audit` mode keeps
the existing diagnostic workflow but cannot make dirty-source evidence release-eligible.

After the governed suite completes, audit stable-prefix reuse and Provider-reported cached usage without persisting
prompt text:

```bash
node infra/generation-reliability/audit-provider-cache-smoke.mjs \
  services/runtime/target/e2e-evidence/<cluster>/real-provider-runs/<suite-directory>
```

`passed` requires an accepted real-provider-verified suite and at least one non-estimated multi-turn Run with the same
static-prefix/tool-set hashes and positive Provider-reported cached input. `toolSetHash` is valid only with
`toolSetHashVersion=tool-definition-set@1`, which hashes the complete sorted Tool Definitions rather than names alone.
The audit binds the result to the suite's
source commit, `modelResourceId`, Provider Resource revision, and Provider configuration SHA-256. Dirty-source evidence
may diagnose cache behavior but is not release-eligible. `provider_not_reporting_cached_usage` is conservative and not
release-eligible; it is not silently treated as a cache hit.

The audit also resolves each selected Run's event stream within the suite directory, verifies its SHA-256 and event
count, and rejects raw prompt, source, Provider request ID, agent-message text, tool output/error, terminal summary, or
credential-shaped fields. Evidence created by a pre-redaction runner therefore cannot pass a current release gate.

Freeze one accepted terminal Build as a portable, checksummed `RuntimeEvidenceBundle@1` after the cache audit passes:

```bash
node infra/generation-reliability/assemble-runtime-terminal-evidence.mjs \
  --suite /absolute/path/to/suite-...-accepted \
  --case agent-cloud-quickstart \
  --cache /absolute/path/to/provider-cache-smoke-audit.json \
  --out /absolute/path/to/runtime-evidence-bundle

node infra/generation-reliability/replay-evidence.mjs \
  /absolute/path/to/runtime-evidence-bundle
```

The same assembler accepts a published Edit summary stored beneath the accepted suite. The Edit runner must have
provisioned and then released its own Sandbox; retained Draft/HMR sessions and caller-owned existing Runs are not
terminal release evidence:

```bash
node infra/generation-reliability/assemble-runtime-terminal-evidence.mjs \
  --suite /absolute/path/to/suite-...-accepted \
  --edit published-edit-...-accepted \
  --cache /absolute/path/to/provider-cache-smoke-audit.json \
  --out /absolute/path/to/runtime-edit-evidence-bundle

node infra/generation-reliability/replay-evidence.mjs \
  /absolute/path/to/runtime-edit-evidence-bundle
```

For a suite executed with `GENERATION_REAL_REPAIR_CANARY=1`, freeze the entire setup-Edit → Review → Repair lifecycle.
The bundle concatenates all three redacted streams and recomputes their combined usage, while the final Route identity
comes only from the accepted Repair Run:

```bash
node infra/generation-reliability/assemble-runtime-terminal-evidence.mjs \
  --suite /absolute/path/to/suite-...-accepted \
  --repair agent-cloud-quickstart \
  --cache /absolute/path/to/provider-cache-smoke-audit.json \
  --out /absolute/path/to/runtime-repair-evidence-bundle
```

For a candidate Runtime Restart cohort, the restart probe now emits cleanup-aware
`generation-context-runtime-restart-evidence@2`. Bind it back to the accepted Build, its redacted stream, the stable
Provider Cache audit, and the frozen deployment/Pod identities:

```bash
node infra/generation-reliability/assemble-runtime-terminal-evidence.mjs \
  --suite /absolute/path/to/candidate-suite-...-accepted \
  --restart runtime-restart-evidence.json \
  --restart-case agent-cloud-quickstart \
  --cache /absolute/path/to/provider-cache-smoke-audit.json \
  --out /absolute/path/to/runtime-restart-terminal-bundle
```

The restart adapter accepts the candidate side only. It requires a changed Pod UID, unchanged Deployment template,
Run/Generation Context/Budget/Source/Artifact identity across replacement, and two confirmed Sandbox-release responses.
Historical `generation-context-runtime-restart-evidence@1` remains readable for diagnostics but is not terminal release
evidence because it did not freeze cleanup confirmation.

Release aggregation consumes bundle directories, not hand-authored pass booleans. Create a portable set file relative
to its own directory:

```json
{
  "schemaVersion": "runtime-terminal-bundle-set@1",
  "bundles": [
    "website-build",
    "docs-build",
    "published-edit",
    "repair-lifecycle",
    "runtime-restart"
  ]
}
```

Set `RUNTIME_RC_TERMINAL_BUNDLE_SET` to this file for a release-mode RC gate. The aggregator replays every directory
before producing `runtime-terminal-bundle-index@1`; release validation independently recomputes coverage and requires
Website, Docs, Edit, Repair, and Runtime Restart entries from the same clean commit, Model Resource revision, Provider
configuration digest, and frozen Budget Profile. Audit mode may omit the set but cannot become release-eligible.
`scenarioKind` is frozen in both the Bundle Manifest and case summary. Website and Docs additionally require `/` and
`/docs/` respectively and must have distinct Project and Run identities, so copying a Website bundle under a second
directory cannot satisfy Docs coverage.
Every Bundle also freezes the raw Provider Cache Audit SHA-256. Release aggregation and the final validator compare
that digest with the exact file supplied through `RUNTIME_RC_PROVIDER_CACHE_EVIDENCE`; the final validator also compares
the canonical full object. Matching Commit/Model/Revision fields from a different cache sample are insufficient.

The terminal bundle does not fabricate or copy the Sandbox route-manifest body after cleanup. It freezes the actual
Runtime-emitted Route Manifest hash/path, Candidate Manifest hash, Source Fingerprint, Generation Context and Budget
Profile identities, every Run's complete `RunBudgetProfile@1` in `budget-profiles.json`, every Run's API-compatible
`RunModelUsage@1` projection in `run-model-usage.json`, redacted event stream, replayed usage, HTTP acceptance probe,
Sandbox release result, and Provider Cache identity. Replay verifies every
checksum, recomputes each Profile's canonical SHA-256, de-duplicates Provider Usage by `(runId, turn)`, and independently
checks the full RunModelUsage projection, per-turn Prompt, per-Run and cross-Run Operation ceilings. Repair evidence
includes Setup Edit, Review and Repair Profiles rather than validating only the final Run.

Production release aggregation additionally requires the versioned policy at
`infra/generation-reliability/release-budget-policy.json` (or an explicitly supplied reviewed replacement). Pass it via
`--budget-policy` to `aggregate-release-evidence.mjs`, or set `RUNTIME_RC_BUDGET_POLICY` for the RC harness. The policy
must approve the exact Profile Hash and effective Phase/Operation limits and must require
`rolloutMode=enforced`, `tokenBudgetMode=split_enforced`, and `operationBudgetMode=enforced`. Shadow evidence can pass
offline replay but remains `not_evaluated` for release policy and cannot produce `releaseEligible=true`.

### Runtime efficiency benchmark cohort

Statistical Token/turn claims use `runtime-efficiency-benchmark-cohort@1`; they are separate from the smaller
Generation Context rollout cohort and from one-shot real Provider smoke tests. Each Profile freezes one Prompt Set,
Design Profile, Template version, Model Resource revision and Provider parameter hash. Its append-only source Ledger
must be continuous, and failed/partial/timeout/rejected Attempts remain visible beside Accepted Attempts.

For fixture/contract work, create the immutable source Ledger, append hashes-only Attempts, verify it, and assemble the
evaluator input with:

```sh
node infra/generation-reliability/runtime-efficiency-benchmark-ledger.mjs init <ledger.jsonl> <session.json>
node infra/generation-reliability/runtime-efficiency-benchmark-ledger.mjs append <ledger.jsonl> <attempt.json>
node infra/generation-reliability/runtime-efficiency-benchmark-ledger.mjs verify <ledger.jsonl>
node infra/generation-reliability/runtime-efficiency-benchmark-ledger.mjs assemble <ledger.jsonl> <benchmark-cohort.json>
node infra/generation-reliability/runtime-efficiency-benchmark-ledger.mjs evaluate <ledger.jsonl>
```

For real release cohorts, do not hand-author Attempt JSON. The real-provider pair collector freezes Runtime
`run-efficiency-metrics@1` and `run-prompt-efficiency@1` values in the hashes-only Generation Context paired Ledger.
Import every verified control/candidate pair atomically with:

```sh
node infra/generation-reliability/prepare-runtime-efficiency-benchmark.mjs \
  <generation-context-paired-ledger.ndjson> <new-benchmark-directory>

node infra/generation-reliability/collect-runtime-efficiency-benchmark.mjs \
  sync \
  <generation-context-paired-ledger.ndjson> \
  <new-benchmark-directory>/benchmark.ndjson \
  <new-benchmark-directory>/import-mapping.json
```

The quantitative corpus is `runtime-efficiency-benchmark-cases.json`: ten distinct Design System Website prompts,
separate from the governed five-case functional canary. The paid runner itself revalidates schema, unique IDs/prompts/
expected text, Website kind, `/` route, and Design System scope; a matching Session SHA alone is insufficient. Preview
the fixed 60-Pair/120-Attempt plan without Provider
calls, then remove the dry-run flag only when paid execution is authorized:

```sh
GENERATION_EFFICIENCY_DRY_RUN=1 \
  bash infra/generation-reliability/run-runtime-efficiency-benchmark-cohort.sh \
  <prepared-session-dir> <batch-prefix>
```

The planner verifies the Paired Ledger Hash-chain and Pair identities before any Provider call, requires a clean paired
Session prepared against the exact corpus SHA, forces one Case Attempt, skips only fully recorded Pairs, and stops on
partial Pair state rather than silently repeating a paid call. Its final `sync` is
idempotent: it atomically imports new Attempts, or, when none are new, verifies the complete Source Binding and exits
successfully. The stricter no-op-failing collector API remains available for fixtures and audit tests.

The mapping binds `greenfield` and `warm_copy_css` buckets to frozen Benchmark Profile IDs and maps Fixture IDs to
the frozen Prompt Set. The collector rechecks Template and Provider Resource identity, derives both variants from the
same Pair, skips already imported Pair sides, and validates the whole batch before appending any record. An Accepted
sample missing a required Runtime metric fails with zero writes. `generationContextBytes` is the conservative Runtime
upper bound `generationContextEstimatedTokens * 4`; a control run with Generation Context disabled may legitimately
record zero. Benchmark collection must keep `GENERATION_COHORT_MAX_CASE_ATTEMPTS=1`; the sample freezes that count and
the importer rejects a value other than one so a failed retry cannot disappear inside an Accepted sample. Direct
`ledger.mjs append` remains useful for fixtures and contract tests, but is not the real release
collection path.

Every selected Build/Edit/Repair Run also freezes `run-design-profile-identity@1`, projected from the Runtime's
read-only Design Context Manifest. Control and Candidate must carry the same effective Profile SHA-256, the paired
Ledger persists it in Pair identity, and Benchmark import requires it to equal the target Profile's
`designProfileHash`. The current administrator selection is never used to backfill historical evidence.

The leading paired-cohort Session record freezes `{source: {commit, dirty}}` and the raw case-manifest SHA-256.
Benchmark import requires the same source identity and requires the Benchmark Prompt Set SHA-256 to equal that case
manifest. The release Source Binding carries the paired source identity forward and must match the clean release
commit; `session-meta.json` alone is not accepted as statistical provenance.

The Ledger uses exclusive creation, an append lock, continuous Attempt sequence numbers and a SHA-256 record chain.
The assembler computes the raw Ledger SHA-256; callers cannot self-assert it in the Cohort. The authoritative gate is
`ledger.mjs evaluate`, which verifies and evaluates the same raw Ledger in one process. For offline inspection, evaluate
an already assembled hashes-only cohort with:

```sh
node infra/generation-reliability/runtime-efficiency-benchmark.mjs <benchmark-cohort.json>
```

Baseline and Candidate each require at least 30 Accepted Attempts covering at least 10 frozen Prompt IDs. Below that
boundary the result and affected distributions are `insufficient_sample`; P50/P95 values and confidence intervals stay
null. A ready cohort reports P50/P95, bootstrap intervals, failure counts and baseline effect sizes, then applies the
documented Greenfield Build or Style/Token Edit limits. Providers without standard cached-usage reporting mark only the
cache distribution/effect/gate `not_applicable`; they never fabricate cache numbers. The CLI exits zero only for `pass`.

Release mode requires all three immutable inputs:

```sh
RUNTIME_RC_EFFICIENCY_BENCHMARK_LEDGER=<benchmark-ledger.jsonl>
RUNTIME_RC_EFFICIENCY_SOURCE_LEDGER=<generation-context-paired-ledger.ndjson>
RUNTIME_RC_EFFICIENCY_IMPORT_MAPPING=<benchmark-import-mapping.json>
```

The aggregator verifies the Benchmark and Paired hash chains, re-derives every Attempt from the source Ledger and
Mapping, embeds `runtime-efficiency-benchmark-source-binding@1`, and binds every Profile to the clean release commit and
selected Model Resource revision/configuration digest. The final Validator repeats the derivation from the same three
raw inputs. It rejects a missing source, a structurally valid manually appended Attempt, missing Greenfield/Edit
workloads, `insufficient_sample`, threshold failures, identity drift, or a modified embedded Evaluation/Source Binding.

Sandbox release is not best-effort evidence. Both Build and standalone published Edit runners use the same bounded,
idempotent confirmation contract and require two successful release responses. Exhausting the retry bound changes an
otherwise accepted operation to `failed`.

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
