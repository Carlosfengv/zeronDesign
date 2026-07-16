# Provider Gateway production runbook

## Preconditions

- The deployed Gateway image is built from the reviewed revision.
- `provider-gateway-postgres` exposes only the Gateway database URL; no
  Provider credential is present in a Deployment environment variable.
- `provider-gateway-encryption-key` contains a randomly generated value of at
  least 32 characters. It is mounted only into Gateway Pods and is backed up
  independently from PostgreSQL.
- The approved model resource is reconciled from GitOps and its SecretRef
  resolves only in the Gateway workload.
- The service mesh mTLS overlay uses the actual Runtime and Admin service
  account principals for the target cluster.

## Real Provider gate

Run the ignored Gateway gate from a secure CI secret store. Do not paste the
credential into shell history, a ticket, or an evidence file.

```sh
RUNTIME_PROVIDER_APPROVAL_ID='<approved-change-or-ticket>' \
DEEPSEEK_API_KEY='<injected-by-secret-store>' \
cargo test real_deepseek_v4_pro_turn_returns_token_usage_through_gateway \
  --manifest-path services/provider-gateway/Cargo.toml -- --ignored --nocapture
```

Record only the following output in the release evidence:

- Gateway and Runtime image digests;
- model resource id/revision, physical model and policy revision;
- input/output token counts and presence of Provider request id;
- Build/Edit/Repair run ids, artifact URL, HTTP status and visual evidence;
- terminal tool failure count and secret-scan result.

Never record the API key, prompt, generated source, Provider response body, or
Provider request id value.

## Token usage and alerts

Use these low-cardinality metrics:

- `provider_gateway_input_tokens_total`;
- `provider_gateway_output_tokens_total`;
- `provider_gateway_turn_total`;
- `provider_gateway_retry_total`;
- `provider_gateway_quota_rejection_total`;
- `provider_gateway_circuit_state`;
- `provider_gateway_queue_depth`.

Alert on a sustained increase in retryable failures, circuit-open state,
quota rejections, queue depth, or a loss of successful turn traffic. Do not
derive money or currency from token counts.

## Provider credential rotation

1. Submit the replacement key only through the audited Admin model-resource
   write endpoint using the existing `db:<alias>` reference.
2. Gateway encrypts the replacement before committing it to PostgreSQL; verify
   that API responses and audit events expose only `secretConfigured=true`.
3. Reconcile any associated resource revision and run the readiness endpoint.
4. Run one approved project canary and verify token usage plus execution
   snapshot revision.
5. Revoke the old secret version after the declared overlap window.

Rotating `provider-gateway-encryption-key` requires a controlled re-encryption
job for encrypted Provider secrets and unexpired idempotency responses. Do not
replace that key by only restarting the Deployment.

## Safe rollback

1. Stop adding projects to the Gateway canary allowlist.
2. Activate the previous model-selection-policy revision using the Admin API.
3. Disable the failing model resource revision if required.
4. Allow only turns that have not started an upstream attempt to select an
   approved alternative resource.
5. Preserve idempotency records, execution snapshots and audit events.

An uncertain upstream attempt must never be replayed through the direct client.
