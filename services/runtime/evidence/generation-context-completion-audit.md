# Generation Context completion audit

Date: 2026-07-20

## Locally complete

| Requirement area | Evidence |
|---|---|
| Content approval authority | Append-only `ContentPlanApproval@1` producer/store/API, exact identity verification, invalidation ordering and startup readiness checks |
| Context identity | Canonical `GenerationContext@1` payload, immutable payload cache, per-Run Binding, double-hash Golden Vector and tamper tests |
| Runtime integrity | Run-start and project Runtime Attestation, Template/App Root/Style Contract checks and template-default behavior |
| Edit safety | Template-derived Editable Surface, full-file/self-authored Mutation Leases, live CAS and per-mutation EditImpactPlan target validation |
| Workflow | Greenfield Static, Cold Dev and Warm HMR profiles; monotonic Epoch/Revision reconciliation; durable Draft boundary tests |
| Delivery and recovery | Bounded observation delivery state, unchanged stubs, compaction/reinjection, checkpoint hash verification and bounded source restore |
| Design behavior | Conservative required-rule selection, Observe override, Enforced conflict and terminal `replan_required` tests |
| Successor authority | `predecessorRunId` validates terminal replan state and replacement Plan, creates a fresh successor and records bidirectional lineage |
| Orchestrator dispatch | Replacement Plan binds to `predecessorRunId`; low-risk creation and confirmation-required approval automatically dispatch an idempotent successor through the normal Run-start boundary |
| Templates | `next-app@2`, current Fumadocs version and frozen historical Template Specs |
| Consumer migration | Runtime/HTTP/Shared plus Canary ledger/validator, real-provider summary, rollback and Release Evidence validator/aggregator dual-read paths |
| Provider harness boundary | Generation Context plus Website/Docs/Attachment real HTTP gates and formal CI use `internal_gateway`, bind only `modelResourceId`, and assert Gateway execution identity; the Provider Key is mounted only behind the Model Resource |
| Operations artifacts | Admin-protected low-cardinality OpenMetrics export, Prometheus Operator ServiceMonitor, Grafana sidecar ConfigMap/dashboard, create-only Prometheus Target/series proof verifier, Evidence Schema, architecture note and rollout/rollback runbook |
| Reproducibility | Versioned `BaselineEvidence@1`, checksum, deterministic baseline calculator, fixed dual-Runtime deployment overlays, prepared immutable cohort session, Runtime metric-to-sample mapper, real-Provider pair collector, append-only hash-chained paired-cohort ledger with exact dual-deployment/Provider identity validation, and seeded rollout evaluator |

## Verified commands

- Complete Runtime Rust suite: all non-ignored tests passed; real-provider/real-build canaries remain explicitly ignored.
- Shared typecheck and 38 tests passed.
- The complete local Runtime harness passed, including the real Fumadocs build, computed-style smoke tests, Provider
  Gateway dry-run boundary checks and all evidence/rollback validators.
- Legacy direct-Provider RC/matrix `release` modes fail closed and direct operators to the governed five-case Gateway
  runner; direct clients remain diagnostic-only.
- Canary, rollback, real-provider summary, Release Evidence, operations artifact, Prometheus proof, baseline, paired-cohort ledger and
  rollout-evaluator script tests passed.
- The real-Provider runner now persists Runtime-owned efficiency metrics, accepts the Runtime's explicit
  `resource:<modelResourceId>` selector without weakening Gateway execution identity, and the real-Provider pair collector
  rejects fixture intent, acceptance, Template, Provider revision, physical model or capability-snapshot drift.
- JSON artifacts parse successfully and the worktree diff passes whitespace validation.

## Verified k3d evidence

- The deployed Runtime completed approval → Plan change invalidation → stale confirmation rejection → reapproval,
  then recovered the exact verified Approval and Producer sequence after a Pod replacement. The create-only proof is
  `services/runtime/target/e2e-evidence/zerondesign-e2e/content-plan-approval-readiness.json`; Content Plan mutations
  used only internal Producer authorization and did not rotate the public Principal Secret. The versioned summary and
  source Evidence hash are `services/runtime/evidence/content-plan-approval-readiness-2026-07-21.json`.
- The complete authoritative `briefs.jsonl@revision=91` snapshot was audited without persisting raw Brief content:
  45 confirmed legacy candidates were all unmappable because none carried the four-field exact Content Plan identity;
  zero verified Approvals were created. The versioned per-candidate hashes and classification reasons are
  `services/runtime/evidence/content-plan-approval-migration-2026-07-21.json`.
- The rebuilt k3d cluster now has Ready control and candidate Runtime deployments on the same immutable image digest.
  Control is Generation Context `off`; candidate is `enabled`; both keep Content Plan attestation in Shadow while the
  candidate still requires exact Producer Approval. PostgreSQL databases, PVCs, object-storage prefixes and public
  Principal Secrets are isolated per side, and the primary Runtime remains unchanged. Versioned deployment identity is
  `services/runtime/evidence/generation-context-cohort-deployments-required-text-2026-07-21.json`.
- Fixed paired session `deepseek-v4-pro-20260721T103843Z` freezes `deepseek-v4-pro@4`, both Runtime deployment
  revisions, the fixture manifest, budgets and cohort-only Principal. Its first Greenfield pair passed Build and Required
  Fidelity on both sides. Candidate input tokens, first-build time, Cold Dev Ready and first-mutation turn were 29.06%,
  30.18%, 30.25% and 57.14% below control respectively. The versioned hashes-only summary is
  `services/runtime/evidence/generation-context-first-paired-sample-2026-07-21.json`.
- The governed `deepseek-v4-pro@4` Model Resource reconciled successfully against the configured upstream through
  Provider Gateway. The readiness evidence is
  `services/runtime/target/e2e-evidence/provider-resource-reconcile-key-rotation-20260720T155525Z.json`.
- A real DeepSeek candidate Greenfield run completed Build, Draft Preview acceptance and durable Draft isolation at
  `services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-candidate-canary-v5/suite-20260720170111505-accepted/real-provider-examples-summary.json`.
- A real DeepSeek Warm Edit completed source mutation, current-revision `preview.dev_status`, HMR iframe acknowledgement
  (`3.755s`), durable snapshot and `run.completed` in
  `services/runtime/target/e2e-evidence/zerondesign-e2e/real-provider-candidate-warm-hmr-v7/suite-20260720181338349-failed/warm-edit-real-20260720181338-ai-governance-console/edit-20260720181648573-failed/run-edit.events.ndjson`.
  The containing v7 suite retained `failed` because the then-current canary verifier required an unrelated application
  icon after all Warm Edit contract checks had passed; that verifier requirement has been removed, but v7 is not
  relabeled as an accepted suite.
- Prometheus Operator scraped the candidate Runtime target as `UP`, and all five required live series were non-zero.
  The create-only proof is
  `services/runtime/target/e2e-evidence/generation-context-monitoring-proof-20260720T182000Z.json`.
- Before the local cluster incident, an older primary/control/candidate deployment used
  `anydesign/runtime:6a5603c0a464-dirty-hmrreadyfix`. That historical deployment is superseded by the current isolated
  deployment evidence above and is not used for cohort identity.

## Paired-cohort collection incident

- An exploratory off/enabled pair proved that Runtime metrics can separate the legacy and Generation Context behavior,
  but it was not admitted to the formal ledger because the old wrapper rolled deployments between sides.
- `deepseek-v4-pro-20260720-formal-001` then froze `deepseek-v4-pro@4`, the same Runtime image, budgets and Principal for
  both sides. Its first pair remained incomplete: the control retained terminal failure metrics, while the candidate SSE
  transport terminated before a Runtime terminal/metrics result could be recovered. The collector correctly rejected it.
- The k3d node subsequently saturated, Kubernetes API calls timed out, and Docker Desktop returned HTTP 500 for container
  restart and image operations. The exact cluster and volume were deleted. Three clean bootstrap attempts then failed in
  Docker/k3s infrastructure before Runtime deployment: one corrupt/lazy Sandbox image export, one Docker Hub TLS timeout,
  and a fresh k3s API server stuck on repeated slow Kine/SQLite queries before node registration.
- A later recovery restored the interrupted kubeconfig and briefly reached an all-green authenticated `/readyz`; deleting
  and recreating CoreDNS also worked. Under Sandbox WarmPool load, however, the Docker API again returned HTTP 500 and
  Kubernetes TLS handshakes timed out. A further clean rebuild switched the single server to embedded etcd to remove
  Kine/SQLite from the path; etcd elected a leader, but Docker VM scheduling/I/O still delayed API initialization until
  kube-apiserver exited with `context deadline exceeded`. This proves the remaining blocker is below the Runtime and is
  not resolved by changing the k3s datastore.
- Host evidence and source changes remain intact under `services/runtime/target/e2e-evidence`; no incomplete pair has been
  appended to the formal ledger and no failed suite has been relabeled.
- The fixed-session pair runner now covers Greenfield, Warm copy/CSS and Warm structural collection. It freezes the same
  Warm marker and EditImpactPlan kind on both sides, retains terminal Edit evidence, validates both samples before an
  atomic dual-record ledger append, and has an offline end-to-end regression test. Prepared collection now resolves the
  target Deployment's Principal SecretRef, all buckets use Draft Preview acceptance, and a Dev snapshot cannot be
  mistaken for a Static Preview.
- Early append-only sessions exposed invalid acceptance methodology and are retained as incident evidence. The first
  product-valid candidate then split an exact requiredText across JSX nodes; the Generation Context prompt had omitted
  the legacy single-node invariant. The invariant and regression assertion were restored before building the current
  cohort image. No old ledger record was deleted, rewritten or relabeled.
- The current formal session has one complete, dual-accepted Greenfield pair and no pending half-pair. It proves the
  collection path and Next Template coverage, but remains below all quantitative sample-count and batch-count exits.

## Open completion gates

1. **Real Provider paired cohorts.** Provider readiness and one dual-accepted paired Next Greenfield sample are now
   proven through the governed Gateway. The required paired Next/Fumadocs,
   visual/non-visual and Greenfield/Cold/Warm/Repair sample matrix has not yet been collected at the plan's minimum
   counts and three-batch distribution. The rebuilt k3d Runtime is healthy and running Generation Context in Shadow.
2. **Quantitative exit thresholds.** The evaluator is executable and deterministic. The current formal ledger has one
   Greenfield pair in one batch versus the required 20 pairs across at least three batches, so it correctly reports
   `insufficient_evidence`; the observed single-pair reductions are directional evidence, not a rollout claim.

Until both remaining gates are closed, this work is implementation-complete for local contracts but not rollout-complete and
must not be represented as satisfying the plan's Definition of Done.
