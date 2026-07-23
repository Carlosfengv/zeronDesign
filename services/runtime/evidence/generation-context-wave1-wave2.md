# Generation Context Wave 1–5 Implementation Evidence

Date: 2026-07-20

## Implemented contracts

- `ContentPlanApproval@1` authoritative append-only transaction store and exact identity verification.
- `GenerationContext@1` immutable payload, Runtime attestation, content hash, and per-Run binding hash.
- `GenerationContextStatus@1` hash-only diagnostics endpoint.
- `ObservationReceipt@1` shadow delivery accounting and `RunEfficiencyMetrics@1` calculator.
- `EditableSurfaceMetadata` as a `TemplateSpec` identity input and a single Workspace-relative view helper.
- `next-app@2` current Template identity with exact `next-app@1` historical resolution retained.
- `fumadocs-docs@runtime-p6` current Template identity with exact p3/p4/p5 historical resolution retained.
- `RunStartAttestation` plus Workspace-derived Project Runtime/Style Contract checks; normal mutations no longer depend on model DCP reads.
- `MutationLease` for every existing-file writer, with `observed_full`/`self_authored` continuity and live content-hash CAS.
- Same-Epoch Source read de-duplication with successful `unchanged` stubs and semantic `(path, content hash)` observation accounting.
- Runtime-owned `greenfield_static`, `cold_dev`, and `warm_hmr` execution profiles and structured Workflow Progress.
- Checkpoint identity fields for both Context hashes, Runtime Attestation, Execution Profile, Workflow State, Window Epoch, target Session Epoch/Revision, and Observation Receipt schema.
- `replan_required` as a persisted terminal mutation state; stale Plans and Repair scope expansion cannot mutate, build, snapshot, or complete the current Run.
- A `predecessorRunId` Run-start path validates the persisted terminal state and newly authoritative EditImpactPlan,
  creates a fresh Context-capable successor, records bidirectional lineage and rejects duplicate successors. Repair can
  now bind a replacement Plan instead of inheriting the stale hash.
- GenerationContext-bound DesignProfile conflicts use a deterministic mode matrix: Observe records `design_constraint_overridden` and continues; Enforced records a structured `design_constraint_conflict`, enters `replan_required`, and rejects same-Run override of the frozen Binding. Legacy Runs retain their confirmation compatibility path.
- Identity-scoped `state/context.md` blocks and bounded post-compaction restoration of recent full Source observations.
- Dual-read Canary/Release Evidence consumers: historical Runs require complete DCP read proof, while new Runs require
  `contextContentHash`, `runContextBindingHash`, Runtime Attestation hash, Template Version and efficiency evidence.
- Versioned Generation Context Run Evidence schema, architecture note, operations runbook and low-cardinality dashboard
  definition.
- Deterministic paired-cohort rollout evaluator with seeded bootstrap intervals, per-bucket sample/batch minimums,
  absolute P50/P95 SLOs, relative-reduction gates and success/fidelity non-inferiority checks.

## Safety properties exercised

- A Build in enforced mode rejects missing, invalidated, or identity-mismatched Content Plan Approval before a Run is created.
- The compiler re-reads the authoritative Approval store and does not trust a caller-supplied Approval projection.
- Content Plan changes invalidate the prior Approval and change both Context and Run Binding hashes.
- Equal semantic inputs reuse the Context content hash across Runs; each Run still receives an independent binding hash.
- A bound Run rejects replacement with a different GenerationContext.
- Runtime attestation, Context payload, and Run binding tampering are detected before model injection.
- Shadow mode compiles and records without changing model requests. Enabled mode injects the Context before the first model turn and after compaction.
- Bound visual artifacts are integrity-checked and assembled as Provider image content blocks. Vision unavailability is recorded and falls back to the verified text Context without failing the main task.
- `project.inspect` and GenerationContext use the same Template-derived Editable Surface. Absolute paths, `..`, and duplicated protected-path declarations fail closed or normalize deterministically.
- Warm HMR does not invoke Production Build. Cold Dev restores dependencies, restarts Dev, waits for the current Revision ACK and durable snapshot, and also does not invoke Production Build.
- Late Session Epoch or Workspace Revision events cannot advance Workflow State. Restart reconciliation prefers current DraftPreviewSession/Project facts over stale checkpoint projections.
- Observation budgets are recommendation/telemetry only. Exceeding them keeps read/search/write tools callable and emits `run_observation_budget_warning` with `blocking=false`.
- Parallel duplicate Source reads serialize through the lease-writing executor: one returns full content and the other returns an `unchanged` receipt.
- Compaction increments Window Epoch, re-injects the same verified Binding, restores at most five recent full Source files under per-file and total token budgets, and never promotes partial/unchanged/injected views into mutation authority.
- Repeated `mutation.read_required` and `mutation.stale_lease` failures participate in the existing bounded recovery/Partial-run fuse.
- EditImpactPlan target validation runs for every Edit/Repair mutation. Consuming the one-time Plan approval does not
  disable scope validation for later mutations in the same Run.
- Real-provider evidence summarization, canary rollback, ledger finalization and release aggregation accept both protocol
  generations; malformed new hashes, reused Run bindings and incomplete efficiency evidence fail closed.

## Reproducible verification

- Rust GenerationContext unit tests: 5 passed.
- Runtime library tests: 244 passed, 2 ignored real-build canaries.
- AgentLoop integration tests: 61 passed, 1 ignored real-provider test.
- Content Plan Approval / GenerationContext Status HTTP integration tests: 2 passed.
- Observation Receipt integration tests: 2 passed, including provider-parallel duplicate reads.
- Template Registry integration tests: 13 passed.
- Sandbox tool integration tests: 89 passed, 1 ignored real Fumadocs build canary.
- Checkpoint integration tests: 26 passed.
- Shared TypeScript tests: 38 passed; typecheck passed.
- Rust/TypeScript GenerationContext Golden Vector matched both hashes.
- Baseline calculator tests passed and reproduced manifest payload hash `922e3d29c9e2c6a5fe1e6699d3787528be5264dea0f1fbb666582f6fc579e92a`.
- Canary ledger/validator, rollback, real-provider summary, release validator/aggregator and operations-artifact tests
  passed for both legacy and Generation Context evidence.
- Rollout evaluator tests passed for pass, regression, invalid and undersized `insufficient_evidence` outcomes.

## Evidence limitation

The checked-in baseline has `eligibleSampleCount=0` and is intentionally reported as `insufficient_evidence`. Historical real-provider artifacts were referenced from an unversioned local `target/` path and are no longer available, so they are excluded rather than reconstructed or treated as valid evidence.

No rollout or efficiency-exit claim is made from this record. One dual-accepted real-provider Next Greenfield pair now
exists, but the plan's required Fumadocs, visual/non-visual, full Greenfield Static/Cold Dev/Warm HMR/Repair matrix and
quantitative P50/P95 sample minimums have not been produced in this workspace.

Production enforcement therefore remains gated on versioned real-provider fixtures, deterministic Observe/Enforced conflict
fixtures from the authoritative intent/profile producer, and the plan's metric
thresholds. The deployed Producer's approval → plan-change rejection → reapproval and restart-recovery path is now proven
by `services/runtime/target/e2e-evidence/zerondesign-e2e/content-plan-approval-readiness.json`, with a versioned summary
and source hash at `services/runtime/evidence/content-plan-approval-readiness-2026-07-21.json`. Local unit and fixture
results are safety evidence, not a substitute for the remaining rollout gates.

Historical Approval migration is now audited against the complete Control Plane `briefs.jsonl@revision=91`. All 45
confirmed candidates lacked the exact four-field Content Plan identity and remain unverified; no Approval was synthesized.
The versioned evidence is `services/runtime/evidence/content-plan-approval-migration-2026-07-21.json`.

The current k3d control/candidate pair is deployed from one immutable Runtime image with isolated PostgreSQL databases,
PVCs, object-storage prefixes and public Principal Secrets. Candidate Generation Context is enabled while attestation
remains Shadow and exact Producer Approval remains required. The versioned deployment identity is
`services/runtime/evidence/generation-context-cohort-deployments-required-text-2026-07-21.json`. Quantitative paired evidence remains
open; deployment readiness is not a rollout-exit claim.

Fixed session `deepseek-v4-pro-20260721T103843Z` has one complete Greenfield pair with both sides Build/Fidelity passing
and no pending half-pair. Candidate input tokens and first-build time were 29.06% and 30.18% below control in this one
sample. Versioned hashes and metrics are
`services/runtime/evidence/generation-context-first-paired-sample-2026-07-21.json`. The evaluator remains
`insufficient_evidence` at 1/20 Greenfield pairs and 1/3 batches; this is not a quantitative exit claim.

Runtime intentionally does not synthesize a replacement Plan at the instant a frozen Plan enters `replan_required`.
It records the terminal state, blocks further mutation/build/snapshot/complete operations, and exposes a validated
Orchestrator successor action. The Orchestrator must first supply the newly authoritative EditImpactPlan; Runtime then
creates the fresh successor and lineage without copying or expanding the stale Plan. Automatic dispatch of that action
still belongs to the product Orchestrator rather than the model or mutation tool.
