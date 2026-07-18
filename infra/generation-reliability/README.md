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

Run the real Provider audit without putting a key on the command line:

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

The matrix allocates an unused localhost port for its Runtime port-forward.
Set `GENERATION_MATRIX_RUNTIME_PORT` only when a stable port is required; an
occupied explicit port fails closed instead of connecting to an unrelated
local service.

Use `GENERATION_MATRIX_RC_MODE=release` only for a clean checkout and a
successful real-provider run. No human approval reference is required.

Run the five governed real Website/Docs examples against an existing k3d
cluster and Provider Gateway:

```bash
GENERATION_REAL_CLUSTER=zerondesign-e2e \
bash infra/generation-reliability/run-real-provider-examples.sh
```

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

Successful runs write `generation-matrix-summary.json`. Failed runs write a
redacted `failure-*` directory containing cluster state, events, and bounded
container logs. Secret objects and raw environment variables are never
collected.
