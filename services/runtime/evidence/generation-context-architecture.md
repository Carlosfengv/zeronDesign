# Generation Context runtime architecture

## Authority boundaries

`ContentPlanApproval@1`, the frozen Design Profile or template default, TemplateSpec, Visual Binding, EditBase and
EditImpactPlan are authoritative inputs. The model cannot create, replace or expand them. The Runtime compiles the
immutable payload, caches it by `contextContentHash`, then creates a Run-specific binding identified by
`runContextBindingHash`.

```text
authoritative inputs
  -> canonical GenerationContext payload -> contextContentHash -> immutable payload cache
  -> Run/Project/Workspace binding        -> runContextBindingHash
  -> Runtime validation                   -> runtimeAttestationHash
  -> model injection + visual delivery
  -> observed_full/self_authored Mutation Lease + live content CAS
  -> workflow ACK -> durable DraftSnapshot -> run.complete
```

The content hash never grants write permission. Existing-file mutations still require a current full-file
observation or self-authorship plus a live CAS match. Cross-Run reuse is limited to the immutable payload cache;
Bindings, delivery state, observation receipts and mutation leases are never reusable.

For Generation Context runs, `project.init` may return up to 24 KiB of full `sourceObservations` derived only from the
TemplateSpec Editable Surface (primary routes, global styles and token file). Each delivered source is hashed, recorded
as an Observation Receipt and establishes the same Run-scoped `observed_full` lease as an explicit `fs.read`. This lets
the next model turn author immediately; missing or over-budget targets still require a targeted full read.

DCP materialization is a two-phase attestation. A Run bound before Sandbox Bootstrap records immutable
`materializationState=pending`; after the package is written, the live Runtime gate requires the Run's materialization
hash to equal the frozen artifact manifest hash before `project.init`, mutation, build or snapshot operations. Missing
or mismatched proof fails closed. The live proof does not rewrite the immutable Run binding.

## Model resource boundary

Runtime sends only `modelResourceId` to Provider Gateway. The Gateway resolves the fixed resource revision, selection
policy, capability snapshot, physical model and credential `SecretRef`. Provider keys remain managed Secrets behind
the resource and never enter Run payloads, evidence or logs. Runtime-to-Gateway bearer authentication is a separate
service boundary and is not a substitute for the Model Resource.

Formal real-provider evidence must use the Gateway path and record only low-sensitivity execution identity and usage.
Direct provider clients are connectivity diagnostics only. Gateway idempotency is derived from the scoped canonical
request hash so Runtime restart cannot rebind a repeated local `runId + turn` key to different content.

The real HTTP lifecycle harness follows the same boundary for Website, Docs and Attachment cases: it requires a
credential-backed Gateway, sends `modelResourceId` on Build/Edit/Repair start, and asserts every Run contains a
matching `ModelExecution` snapshot. Its environment may contain a Runtime-to-Gateway workload token, but it neither
accepts nor forwards a Provider API Key.

## Runtime state and recovery

- `GenerationContext@1` is injected before the first model turn and reinjected after a context-window epoch change.
- `ContextVisibilityState` is epoch-scoped. An `unchanged` response is delivery optimization only and cannot create a
  Mutation Lease.
- `ObservationDeliveryState` is bounded. Checkpoint recovery verifies content, binding and attestation hashes before
  restoring progress.
- `state/context.md` stores independent design-profile and conversation-compact identity blocks; compaction does not
  recursively embed previous compact payloads.
- Workflow reconciliation is monotonic in Session Epoch and Workspace Revision. Late iframe or snapshot events are
  recorded and ignored.

## Execution profiles

Greenfield Static performs the authoritative generation build. Cold Dev restores dependencies, restarts Dev and runs
a light preflight. Warm HMR waits for the matching iframe ACK and durable snapshot without a default Production Build.
Build or validation failure enters `diagnostic_required`; a frozen-plan conflict enters terminal
`replan_required` and prohibits further mutation in that Run.

The per-Run efficiency endpoint derives counters and timings from durable Run/Event state. For Warm HMR it records the
elapsed time from the last source-mutation commit to the first `preview.dev_status` observation that proves the current
Workspace Revision is both iframe-ready and durable. It also reports first-build success, the latest required-fidelity
result and out-of-scope mutation failures. The OpenMetrics exporter aggregates the same data without Project, Run,
path or model labels; `draft_hmr_iframe_applied_seconds` is available for P50/P95 SLO monitoring.

The Orchestrator creates a replacement through the normal Run-start authority boundary with `predecessorRunId`, a
current EditBase and a newly stored EditImpactPlan hash. Runtime accepts only a Partial predecessor carrying persisted
`replan_required`, preserves project/phase, rejects duplicate successors, validates any required confirmation, creates
a fresh Run rather than copying GenerationContext, and records both lineage directions while holding the Run store
write lock. `CreateEditImpactPlanRequest.predecessorRunId` persists the orchestration binding: low-risk Plans dispatch
immediately, while confirmation-required Plans dispatch only after approval. Dispatch preserves the predecessor's
Sandbox, Model Resource, Content Plan, Design Profile and Content Sources, recompiles the fresh Run binding, resumes
the latest predecessor user intent, and is idempotent by the recorded successor identity.

## Compatibility boundary

Historical template versions retain the legacy DCP read protocol. `next-app@2` and the current Fumadocs template use
Runtime Attestation and do not require the model to read DCP files. HTTP, Shared, Canary and Release Evidence are
dual-read consumers during the recovery window. New evidence must contain both Generation Context hashes, the Runtime
Attestation hash, Template Version and efficiency counters. Legacy evidence must retain complete required-read proof.

Rollout comparison uses two fixed Runtime deployments and an append-only, hash-chained cohort ledger. Each pair binds
the same fixture, Model Resource/revision, resolved model version, Provider-parameter hash, Template Version,
Capability Snapshot and phase; only the Generation Context mode changes. Model execution is accepted only with an
internal-Gateway evidence hash. The ledger rejects sensitive payload fields and derives coverage only from completed,
fidelity-passing candidate samples.

The removal date for legacy read-only fields must be set only after no recoverable historical Run depends on them.
