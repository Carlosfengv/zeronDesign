# Generation Context operations runbook

## Readiness

Before enabling Generation Context, verify the Content Plan Approval producer is ready and append-only, Runtime and
Shared tests are green, and the target Template Version contains Editable Surface metadata. Enforced startup must fail
if the approval producer is unavailable. Do not bypass this check with fixture data in a release environment.

Use `RUNTIME_GENERATION_CONTEXT_MODE=shadow` for the compatibility observation window. Move to `enabled` only when
shadow attestation differences are zero, tamper tests pass, and the canary evidence validator accepts both a legacy
Run and a Generation Context Run. Keep historical Template Versions recoverable throughout the window.

## Canary procedure

1. Freeze Provider model/version and parameters, Template Version, capability snapshot, fixture intent and batch ID.
2. Collect paired legacy/control and Generation Context samples. Keep failures, timeouts and fallbacks.
3. Record only hashes, counters, capability state and Run lineage; never store prompts, source, image bytes, temporary
   image URLs, credentials or Provider response bodies.
4. Validate the append-only canary ledger and release evidence with the scripts in `services/runtime/scripts`.
5. Report sample count, median, P95 and 95% bootstrap interval. An undersized cohort is
   `insufficient_evidence`, never pass.

Control and candidate must be separate fixed Runtime deployments. The control deployment uses
`RUNTIME_GENERATION_CONTEXT_MODE=off`; the candidate uses `enabled`. Both deployments must allow the exact same frozen
Provider Resource set through the internal Gateway. Within each pair, both sides must bind the same `modelResourceId`,
Provider Resource revision, resolved model version and Provider parameter hash. This permits separately bucketed visual
and non-visual resources without comparing different models inside a pair. Do not mutate a live deployment between the
two sides of a pair.

Create the fixed dual Runtime deployments from the same already-reviewed Runtime image with:

```sh
GENERATION_COHORT_RUNTIME_IMAGE=registry.example/anydesign/runtime@sha256:... \
bash infra/generation-reliability/deploy-generation-context-cohort-runtimes.sh
```

If the image is omitted, the deployer reads it from the existing `anydesign-runtime` Deployment. The generated
`anydesign-runtime-generation-control` and `anydesign-runtime-generation-candidate` Deployments have independent PVCs,
distinct Services/selectors, the exact `off`/`enabled` modes, and the same internal Gateway URL plus workload-token
SecretRef. Candidate startup requires the Approval producer while Content Plan attestation remains Shadow; Generation
Context enforcement still rejects missing, invalidated or stale Approval before Build. The deployer verifies these
properties after rollout and writes create-only, hashes-only deployment evidence; it never reads the Provider Secret.

Before collecting paid samples, prepare one fixed session. This reconciles the Provider Resource once, installs one
session-scoped Runtime Principal public key, applies the frozen budget to both Runtime deployments, rolls both sides,
records their exact revisions, and initializes the ledger. It does not rotate or copy the Provider API key:

```sh
GENERATION_COHORT_CONTEXT=k3d-zerondesign-e2e \
GENERATION_COHORT_WORKSPACE_NAMESPACE=ws-runtime-rc \
bash infra/generation-reliability/prepare-generation-context-cohort-session.sh
```

Pass the printed directory as `GENERATION_REAL_PREPARED_SESSION_DIR` to every control and candidate invocation of
`run-real-provider-examples.sh`. Prepared mode is read-only with respect to Runtime and Provider deployments: it rejects
deployment generation, the target Deployment's mounted Principal key, Workspace or Provider revision drift instead of
patching or rolling either side. Cohort collection never reads or modifies the primary Runtime's Principal Secret.

The default preparation remains the reviewed DeepSeek non-visual resource. To prepare a separately governed vision
resource, provide an immutable Provider configuration and a case manifest whose `provider.modelResourceId` is the same
resource ID. The Provider secret must already be mounted at the configuration's `secretRef`; never pass an API key in
the command line or evidence directory:

```sh
GENERATION_COHORT_PROVIDER_RESOURCE_ID=vision-model-resource \
GENERATION_COHORT_PROVIDER_POLICY_ID=vision-model-policy \
GENERATION_COHORT_PROVIDER_CONFIG=/reviewed/model-resources.vision.json \
GENERATION_COHORT_CASES_FILE=/reviewed/real-provider-cases.vision.json \
GENERATION_COHORT_SESSION_ID=vision-model-resource-$(date -u +%Y%m%dT%H%M%SZ) \
bash infra/generation-reliability/prepare-generation-context-cohort-session.sh
```

Preparation fails unless the case manifest matches the reconciled resource. A multimodal pair additionally requires
the frozen resource to declare `vision` or `visionInput`, bounded `maxImageCount`, and PNG in
`supportedImageMediaTypes`.

Run one fixed Greenfield, Warm, Cold Dev, Repair or Runtime Restart pair with the session-aware orchestrator. It invokes each frozen deployment
exactly once, preserves both successful and failed suite evidence, freezes the same case and lifecycle marker, selects the matching
Runtime metric result, and atomically appends both hashes-only sides. A failed collection keeps its create-only pair
directory and must be retried under a new batch ID rather than overwritten:

```sh
bash infra/generation-reliability/run-generation-context-paired-pair.sh \
  /path/to/prepared-session batch-01 zenova-agent-cloud greenfield

bash infra/generation-reliability/run-generation-context-paired-pair.sh \
  /path/to/prepared-session batch-01 zenova-agent-cloud warm_copy_css

GENERATION_COHORT_NONVISUAL_REFERENCE=1 \
bash infra/generation-reliability/run-generation-context-paired-pair.sh \
  /path/to/prepared-session batch-01 oilfield-operations-dashboard warm_copy_css

GENERATION_COHORT_CASES_FILE=/reviewed/real-provider-cases.vision.json \
GENERATION_COHORT_MULTIMODAL_REFERENCE=1 \
bash infra/generation-reliability/run-generation-context-paired-pair.sh \
  /path/to/vision-prepared-session batch-01 oilfield-operations-dashboard warm_copy_css

bash infra/generation-reliability/run-generation-context-paired-pair.sh \
  /path/to/prepared-session batch-01 zenova-agent-cloud warm_structural

bash infra/generation-reliability/run-generation-context-cold-dev-pair.sh \
  /path/to/prepared-session batch-01 zenova-agent-cloud

bash infra/generation-reliability/run-generation-context-repair-pair.sh \
  /path/to/prepared-session batch-01 agent-cloud-quickstart

bash infra/generation-reliability/run-generation-context-runtime-restart-pair.sh \
  /path/to/prepared-session batch-01 agent-cloud-quickstart

bash infra/generation-reliability/run-generation-context-runtime-restart-pair.sh \
  /path/to/prepared-session batch-01 oilfield-operations-dashboard
```

The non-visual canary creates one immutable reference-image Artifact and passes it in
`inputContext.visualBindings` when both Runs start. Control remains the matching legacy baseline; candidate must prove
the frozen Visual Binding Set Hash and Runtime Attestation Hash, record `visual_binding.unavailable`, continue the text
main task, and finish normal fixture acceptance. The collector rejects a coverage claim if the binding was added after
StartRun, if the Provider Resource is marked vision-capable, if the unavailable metric is absent, or if the main Run
did not complete. The canary stores only Artifact identity and hashes, never image bytes or a storage URL.

The multimodal canary uses the same StartRun binding contract but requires candidate delivery to finish as
`delivered`. Runtime status alone is not sufficient: the accepted `model.execution` must contain a Gateway-produced
`visualInput.state=verified_and_provider_accepted` record whose Artifact SHA and MIME match the frozen reference.
Gateway emits that hashes-only record only after it has fetched the Run-bound Artifact, verified bytes/MIME/SHA/
dimensions, converted it to a real Provider image input, and received a valid upstream response. The collector also
requires the prepared session capability snapshot to be vision-capable and rejects any unavailable metric, asserted
coverage string, mismatched model resource, or missing main-task completion.

`GENERATION_COHORT_MAX_CASE_ATTEMPTS=2` or `3` may be used for real-Provider sampling when an otherwise unchanged
fixture occasionally ends with a model `no_progress` result. Each attempt remains a normal paid Run with the same
per-Run budget and stop limits; the first accepted case is selected, all attempted suite evidence is preserved, and
the value must remain between 1 and 3. It does not permit retrying an already appended pair or overwriting a batch.

Website buckets validate the durable Draft Preview produced by the `next-app` Build. They do not read
`/artifacts/<project>/current`, because a Draft-only `next-app` Build intentionally does not create or advance a
published current Version. `fumadocs-docs` retains its template-specific `preview.publish` Candidate/Version lifecycle,
so Docs Greenfield buckets validate the artifact atomically promoted by `run.complete`. The current Warm canary is
DraftPreviewSession-specific and therefore accepts only website fixtures; Docs Warm collection fails preflight until a
Fumadocs Edit lifecycle runner provides equivalent revision, visibility and durability evidence. Cold Dev is also
DraftPreviewSession-specific and accepts only website fixtures. Its approved EditImpactPlan fixes the operations to
`dependency + copy` and its writable target to `project/app/page.tsx`; Runtime restores the frozen dependency graph
through `project.ensure_dependencies` without allowing direct package-manifest mutation, restarts the managed Dev process, and requires both current-revision
`coldDevReady` and durable DraftSnapshot evidence. It never runs a Production Build.

Repair currently accepts only Fumadocs fixtures with the Version lifecycle. The base Build contains one deterministic,
isolated contrast defect; a real read-only Review child must record a repairable finding, and the Repair child must
target that exact finding, mutate source, publish a fresh validated Version, and preserve the canary marker. Website
fixtures fail preflight until Next Draft Review/Repair has its own product lifecycle.

Runtime Restart accepts Fumadocs and Next Greenfield Builds. Fumadocs binds the promoted Version and requires the
published artifact bytes to remain exact. Next intentionally has no current Version, so it binds the durable
DraftSnapshot source hash, Preview Lease, Project history, HTTP 200 and frozen acceptance marker. Live Next Dev SSR
HTML may contain dynamic Runtime fragments; body-SHA equality is recorded as a non-blocking observation and cannot
replace the durable Draft and marker checks. After each side completes, the runner captures a sanitized pre-restart snapshot, deletes exactly
the Deployment-owned Runtime Pod without changing the frozen Deployment template, waits for a different Ready Pod UID,
and captures the same state again. `runtimeRestart` coverage is accepted only when the collector validates that the
Deployment UID/generation/template, Generation Context contract and dual hashes, Workflow State, Runtime efficiency,
current Version or durable Draft, Project history, release-critical state and matching artifact semantics are preserved. The probe records hashes,
IDs and booleans only; the session Principal key and Runtime Admin token remain credential files and are never copied to
evidence. A plain coverage string without valid control and candidate restart evidence is rejected.

The pair runner owns the Greenfield, Warm copy/CSS, Warm structural, Cold Dev and Repair buckets; the Cold Dev, Repair
and Runtime Restart wrappers keep those lifecycles explicit at the command boundary. Runtime Restart remains a
Greenfield sample plus independently validated cross-cutting coverage, not a fabricated sixth efficiency bucket. Do not
relabel one lifecycle into another.

Create and append the hashes-only, tamper-evident ledger with:

```sh
node services/runtime/scripts/generation-context-paired-cohort-ledger.mjs init cohort.ndjson session.json
node services/runtime/scripts/create-generation-context-paired-sample.mjs \
  control-metadata.json control-run-efficiency-metrics.json control-sample.json
node services/runtime/scripts/generation-context-paired-cohort-ledger.mjs append cohort.ndjson control-sample.json
node services/runtime/scripts/create-generation-context-paired-sample.mjs \
  candidate-metadata.json candidate-run-efficiency-metrics.json candidate-sample.json
node services/runtime/scripts/generation-context-paired-cohort-ledger.mjs append cohort.ndjson candidate-sample.json
node services/runtime/scripts/generation-context-paired-cohort-ledger.mjs verify cohort.ndjson
node services/runtime/scripts/generation-context-paired-cohort-ledger.mjs assemble cohort.ndjson rollout-evidence.json
```

For real-Provider case evidence produced by `run-real-provider-examples.mjs`, use the collector instead of manually
copying Runtime metrics. A `generation-context-real-provider-pair-spec@1` file selects exactly one control Run and one
candidate Run by `runId` or `phase`; both must prove the same fixture intent, acceptance contract, template, Provider
revision, physical model and capability snapshot. The collector derives hashes-only samples from the event-stream and
acceptance evidence hashes, then atomically validates and appends both sides:

```sh
node services/runtime/scripts/collect-generation-context-paired-sample.mjs \
  session.json cohort.ndjson pair-spec.json
```

The real-Provider runner fetches the Runtime-owned `/runs/{runId}/efficiency-metrics` result after every selected
terminal Run. Missing metrics or identity drift closes collection instead of estimating values from summary text.

`init` and the assembled output are create-only; `append` uses an exclusive lock, rejects a duplicate pair side and
requires exact control/candidate identity. Every record is chained to the previous SHA-256. The validator rejects
prompt/source text, image payloads or URLs, credentials, authorization values and Provider request/response bodies.
Only immutable storage references, content hashes, counters, timings, pass/fail states, Gateway execution evidence hash
and a frozen Provider Resource identity from the session allowlist are allowed. Required coverage is derived only from
completed, fidelity-passing candidate samples and cannot be asserted in the session header. A failed, timed-out or
fallback Run is appended with that status;
it is never filtered from the assembled cohort. Manual single-side append can leave a partial pair visible in `verify`
and blocking `assemble`; the real-Provider collector instead validates both samples first and appends both records in
one locked write, so a validation failure appends neither side.

Fetch `run-efficiency-metrics@1` from `GET /runs/{runId}/efficiency-metrics` with the normal project-scoped principal.
The sample creator maps the Runtime-owned counters/timings into the rollout metric names and deliberately drops Project
and Run IDs. Its metadata file contains only pair identity, Gateway execution hash, immutable source/acceptance hashes,
terminal classification and coverage claims—never the fixture prompt or source. A `completed` override cannot mask a
non-completed Runtime status, and a missing Required Fidelity result requires an explicit boolean backed by the
acceptance evidence hash.

Evaluate the immutable paired-cohort manifest with:

```sh
node services/runtime/scripts/evaluate-generation-context-rollout.mjs <rollout-evidence.json>
```

The command exits successfully only for `pass`; `fail`, `invalid` and `insufficient_evidence` remain closed gates.
The provider harness runs the same gate and stores the evaluation beside the other release evidence when
`RUNTIME_E2E_GENERATION_CONTEXT_COHORT_LEDGER` points to the append-only ledger. It verifies and assembles the ledger
before evaluating it. `RUNTIME_E2E_GENERATION_CONTEXT_ROLLOUT_EVIDENCE` remains available for an already assembled,
immutable manifest; the two inputs are mutually exclusive.

Required cohort size is 20 valid pairs per Greenfield/Warm bucket and 10 per Cold Dev/Repair bucket, across at least
three batches. Cold Dev P50/P95 must be at most 15s/30s; Warm copy/CSS iframe ACK 1s/3s; Warm structural ACK 2s/5s;
durable snapshot P95 5s. First-success and required-fidelity interval lower bounds must stay within -5pp and -2pp of
baseline respectively.

## Monitoring hookup

Runtime exposes the low-cardinality OpenMetrics snapshot at
`/internal/metrics/generation-context`. The endpoint accepts only the configured internal Admin Secret as a standard
Bearer token (the legacy internal headers remain valid for operator diagnostics). It aggregates durable Run/Event
state and never emits Project ID, Run ID, model name, paths, prompts, source, Provider responses or credentials.

Install the Prometheus Operator `ServiceMonitor` and the Grafana sidecar ConfigMap with:

```sh
kubectl apply -k infra/generation-reliability
```

Before applying, set the ServiceMonitor namespace and `release` label to the target monitoring stack if they differ
from `monitoring` and `prometheus`. The monitoring namespace must contain an
`anydesign-runtime-internal-admin` Secret with key `token`; it must hold the same value mounted into Runtime. Do not
copy that value into the ServiceMonitor or dashboard. Verify the target is `UP`, then check that
`generation_context_compile_total`, `agent_time_to_first_mutation_seconds_count` and
`agent_successor_run_created_total` appear without project/run/path/model labels. The checked-in dashboard ConfigMap
uses the `grafana_dashboard=1` sidecar label.

After a real canary has exercised compile, mutation, Warm HMR, durable snapshot and replan-successor paths, collect the
machine-verifiable Prometheus proof with:

```sh
PROMETHEUS_AUTH_TOKEN_FILE=/var/run/secrets/prometheus/token \
node infra/generation-reliability/verify-generation-context-monitoring.mjs \
  https://prometheus.example generation-context-monitoring-proof.json
```

The verifier requires exactly one fresh, error-free `UP` target for
`/internal/metrics/generation-context`, plus non-zero live series for compile, first mutation, iframe applied, durable
snapshot and successor creation. It rejects Project, Run, path and model labels. The create-only proof stores only the
Prometheus build identity, safe target labels, hashes and aggregate observations; it never stores the Prometheus token,
scrape address or raw API response. Missing canary series is a failed proof, not an empty-but-ready pass.

## Incident triage

- Compile or binding mismatch: stop the affected cohort, preserve Run Evidence, and compare canonical input identities.
- Runtime Attestation failure: block mutation/build for the affected Run. Never downgrade it to a model-read warning.
- `replan_required`: confirm no further mutation or snapshot occurred. Use the recorded successor reference when the
  Orchestrator creates the replacement Run.
- Late Epoch/Revision event: confirm it increased the rejection counter and did not advance workflow state.
- Visual delivery unavailable: keep the main task running when visuals are optional; never report Review as passed.
- Repeated reads or slow first mutation: inspect unique/duplicate read counters and prebuild list/search totals by
  phase/template/model resource. Do not add Project ID, Run ID or paths as metric labels.

## Rollback

Set new traffic back to the legacy protocol by Template Version and disable Generation Context compilation for new
Runs. Do not disable DCP materialization, Style Contract or Runtime integrity checks. Existing compiled Runs retain
their frozen Binding and either complete under it or terminate; never rewrite a Binding in place. Preserve checkpoints,
lineage and evidence for post-incident comparison.

Rollback is complete only after a new legacy canary passes, no orphan preview/snapshot resources remain, and the
published release is unchanged by failed Draft operations.
