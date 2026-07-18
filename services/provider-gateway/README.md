# Provider Gateway MVP

This service is the internal model-access boundary for zeronDesign Runtime.
It currently implements the MVP data-plane contract:

- versioned `POST /v1/agent/turn` requests and responses;
- `ModelResource` + `ModelSelectionPolicy` configuration;
- explicit selection allowlists and capability matching;
- OpenAI-compatible Provider calls via `secretRef` (`file:` or local-development `env:` backends);
- per-turn idempotency, bounded retry, controlled automatic switching, and low-sensitivity execution snapshots;
- a persistent per-`modelResourceId + revision` circuit that opens after three retryable upstream failures and probes again after a 30-second cooldown;
- independent per-resource concurrency bulkheads (`defaults.maxConcurrentRequests`), returning `gateway_overloaded` with a retry hint when full;
- policy-level project concurrency and durable daily input-token budgets (`limits.maxConcurrentTurns`, `limits.dailyInputTokens`), returning `gateway_quota_exceeded` before an upstream call when exhausted;
- bounded message count, system-prompt/string size, and JSON depth, plus fail-closed validation of Provider tool IDs, names, arguments, and Runtime tool registry membership;
- bearer workload-token validation when `PROVIDER_GATEWAY_RUNTIME_TOKEN_FILE` is configured.
- separate Admin bearer-token validation through `PROVIDER_GATEWAY_ADMIN_TOKEN_FILE`.
- Prometheus text metrics at `GET /metrics`, with only approved low-cardinality labels (Provider type, model resource alias, phase, status, and retry/switch reason).

For a Build, Edit, or Repair Run, Runtime accepts `inputContext.modelResourceId` on `POST /runs`. It forwards only that resource ID as `routing.modelResourceId`; omitting it keeps automatic selection. Gateway remains the authorization point: an explicit resource outside the active policy allowlist is rejected, and neither endpoint nor credential can be supplied by a Run request.

Automatic policy resolution is deterministic and scoped by project, workspace, organization, phase, and agent profile. More specific scope wins in that order; phase/profile selectors then distinguish policies at the same scope. This lets Build and Edit select different approved resources without creating a permanent model binding on the work or Run.

`deadlineAt` is enforced for every upstream attempt. The effective timeout is the smaller of the approved model resource timeout and the remaining turn deadline; retries never extend a Runtime turn beyond its declared deadline.

Retryable Provider failures use bounded exponential backoff with deterministic jitter. A valid Provider `Retry-After` value takes precedence (capped at 60 seconds) and is returned to Runtime as `retryAfterMs`; Gateway does not sleep or retry past the turn deadline.

Run locally with a reviewed JSON configuration:

```sh
PROVIDER_GATEWAY_CONFIG_FILE=../../infra/provider-gateway/model-resources.example.json \
PROVIDER_GATEWAY_RUNTIME_TOKEN_FILE=/path/to/runtime-token \
provider-gateway
```

The example references a mounted secret file and contains no credential value. `fixture` resources are intentionally not callable by this production binary.

SQLite is implemented for development. Production uses a `postgres://` or
`postgresql://` URL and the included PostgreSQL migration. Idempotency,
execution snapshots, resource/policy revisions, audit events, circuit state,
token quota, project concurrency leases, and encrypted Provider secrets use
separate tables with transactional row/CAS updates. SQLite must not be used
with more than one replica.

Production PostgreSQL also requires
`PROVIDER_GATEWAY_ENCRYPTION_KEY_FILE`. Gateway uses that independently mounted
key to encrypt short-lived idempotency responses and `db:` Provider secrets
before writing them to PostgreSQL. The encryption key itself must not be stored
in PostgreSQL.

The production Deployment reads `PROVIDER_GATEWAY_DATABASE_URL` from the
`provider-gateway-postgres` Secret, starts at two replicas, and is protected by
a PDB/HPA. The PostgreSQL workload must carry the
`app.kubernetes.io/name=provider-gateway-postgres` label in `data-system`, or
the default-deny NetworkPolicy will prevent database access.

For clusters using the repository's existing Istio/SPIFFE PKI, apply
`infra/provider-gateway/mtls-istio.yaml` after replacing its Runtime service
account principal with the deployed identity. It enforces mTLS at the mesh
boundary while the Gateway retains its runtime bearer validation as a second
authorization check.

`/health/ready` requires durable storage to be readable and at least one enabled policy candidate whose `secretRef` resolves. It does not send a billable Provider request; use the Admin readiness probe for that explicit action.

The Admin API lives under `/internal/provider-gateway/admin/v1`. Every write requires an Admin bearer token, `Idempotency-Key`, `x-operator-id`, `x-change-reason`, and `x-change-reference`; the latter three are appended to the audit trail. The idempotency key is stored with a request hash and completed response, so a retry returns the original result instead of creating another revision or writing the API key again. `POST /model-resources` accepts a write-only `apiKey`; production resources use a `db:<alias>` secret reference, and Gateway encrypts the key before writing it to the shared PostgreSQL secret table. Development may still use `file:` references. The key is excluded from API responses, model-resource rows, and audit events. Resource responses also omit `secretRef`, returning only `auth.secretConfigured`.

Completed turn responses are encrypted and retained for a 24-hour idempotency
replay window. After expiry, Gateway removes the replay body and returns
`idempotency_result_expired` rather than issuing a potentially billable
duplicate Provider request. Low-sensitivity execution snapshots and token usage
remain available independently.

`GET /audit-events` is Admin-token protected and supports bounded `limit` (1–100), `beforeId`, `eventType`, and `subjectId` filters. It returns low-sensitivity change/readiness/execution summaries in newest-first order. The response recursively removes secret references, API keys, authorization, endpoints, cookies, and system prompts, including from historical records created by older Gateway versions.

`POST /model-selection-policies/{id}/activate` activates a historical policy revision using `expectedRevision` plus `revisionToActivate`. It has the same Admin idempotency and change-context requirements as other writes, validates the historical policy against current resources, emits rollback audit events, and never reuses revision numbers after a rollback.

For GitOps-managed resources, `POST /configuration/reconcile` rereads `PROVIDER_GATEWAY_CONFIG_FILE`, validates the complete mounted configuration, and creates a new revision only for resources or policies whose non-revision fields changed. It is Admin-token/idempotency/change-context protected, records the configuration digest in audit, and refreshes only future turns; it never rewrites existing execution snapshots. The reconcile call is deliberately explicit so a ConfigMap mount update alone cannot silently change routing.

The local k3d DeepSeek source of truth is
`infra/provider-gateway/model-resources.deepseek-v4-pro.json`. It contains only a
mounted file Secret reference, never the credential. Apply and verify it with
`infra/provider-gateway/reconcile-k3d-model-resources.sh`; the script annotates
the Deployment with the configuration digest, performs an audited reconcile,
and fails unless the database current resource and policy revisions exactly
match the declaration. Set `PROVIDER_GATEWAY_RUN_READINESS_PROBE=1` for the
minimal paid Provider probe used by real release evidence.

`POST /model-resources/{id}/validate` reruns schema, URL, and DNS policy checks on the current resource revision and records a low-sensitivity validation audit event. `POST /model-resources/{id}/readiness` requires `expectedRevision`, the normal Admin change context, and an idempotency key; it sends a fixed minimal `Reply with READY.` probe, applies the normal DNS/circuit/bulkhead defenses, and audits only the resource version, Provider request ID, and usage summary.

Model resource endpoints are validated for HTTPS (outside tests), safe URL shape, and literal private/loopback/metadata IP addresses. Before every real Provider call, Gateway resolves the endpoint and rejects private, loopback, link-local, and metadata IP results. The Provider HTTP client never follows redirects. Production deployment must still enforce the approved-host egress policy as a second line of defense; this MVP does not claim that a preflight DNS lookup alone prevents DNS rebinding.

The Gateway records normalized input, output, and cached-input token usage. Token quota is enforced from the configured input-token budget; it deliberately has no pricing catalog or currency fields.

`/metrics` is an internal operational endpoint, not an Admin API. It intentionally never emits organization, project, run, request, prompt, tool, endpoint, or credential data. The Deployment NetworkPolicy permits a monitoring namespace to scrape it; deployments using a different monitoring namespace must update that selector as part of their reviewed manifest change.

In addition to turn/usage/retry metrics, Gateway emits `provider_gateway_circuit_state{model_resource}` (`0=closed`, `1=half_open`, `2=open`) and `provider_gateway_queue_depth{model_resource}` from the active resource revision. These labels use only approved model resource IDs.

For trace correlation Gateway forwards its generated `X-Request-ID` to the selected Provider and records any Provider request ID returned in the response. The user prompt, tool input, API key, and endpoint are never added to ordinary metrics or error bodies.

On `SIGTERM`/Ctrl-C the process first marks readiness unavailable and returns retryable `gateway_draining` for new turns, waits five seconds for endpoint propagation, then starts Axum graceful shutdown. The Kubernetes manifest reserves 45 seconds for already accepted turns to finish; durable idempotency records prevent uncertain attempts from being replayed after restart.
