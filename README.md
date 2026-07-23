# zeronDesign

## Runtime model providers

Runtime and all formal real-provider gates use the internal Provider Gateway:

```bash
MODEL_PROVIDER=internal_gateway
MODEL_GATEWAY_URL=http://localhost:9000
MODEL_GATEWAY_AUTH_TOKEN_FILE=/path/to/runtime-workload-token
AGENT_MODEL=deepseek-v4-pro
```

Runs submit only `modelResourceId`. Provider endpoint, physical model, capabilities and Provider API Key are resolved
by the Gateway from the versioned Model Resource. Configure or rotate the Provider Secret through the Gateway Admin
API or a mounted Secret; never pass it to Runtime or a Run request.

The direct OpenAI-compatible adapters below are retained only for isolated connectivity diagnostics and unit coverage.
They do not qualify as canary, paired-cohort, or release evidence:

```bash
# DeepSeek
MODEL_PROVIDER=deepseek
DEEPSEEK_API_KEY=...
# optional; defaults to deepseek-chat when MODEL_PROVIDER=deepseek
DEEPSEEK_MODEL=deepseek-chat
# optional; defaults to https://api.deepseek.com
DEEPSEEK_BASE_URL=https://api.deepseek.com

# Kimi / Moonshot global
MODEL_PROVIDER=kimi_global
KIMI_API_KEY=...
# optional; defaults to https://api.moonshot.ai/v1
KIMI_BASE_URL=https://api.moonshot.ai/v1

# Kimi / Moonshot China
MODEL_PROVIDER=kimi_cn
KIMI_CN_API_KEY=...
# optional; defaults to https://api.moonshot.cn/v1
KIMI_CN_BASE_URL=https://api.moonshot.cn/v1
```

`KIMI_API_KEY` can also be supplied as `MOONSHOT_API_KEY`. `kimi_cn` falls back to
`KIMI_API_KEY` / `MOONSHOT_API_KEY` when `KIMI_CN_API_KEY` is not set.

`AGENT_MODEL` or `MODEL_NAME` can override the model used by `/runs` for any
provider. The default remains `internal-balanced` for the internal gateway.

Real governed regression checks are gated behind ignored tests, so a live Gateway is never required by the default
suite. To rerun the Generation Context regression against an already configured Model Resource:

```bash
MODEL_GATEWAY_URL=http://localhost:9000 \
MODEL_GATEWAY_AUTH_TOKEN_FILE=/path/to/runtime-workload-token \
DEEPSEEK_E2E_MODEL=deepseek-v4-pro \
RUNTIME_PROVIDER_APPROVAL_ID=<approval-event-id> \
cargo test --manifest-path services/runtime/Cargo.toml \
  --test http_api real_provider_generation_context_greenfield_build \
  -- --ignored --nocapture
```

Optional OpenAI-compatible safeguards:

```bash
MODEL_STREAMING=true
MODEL_STRICT_TOOLS=true
```

Build/Edit runs use a hybrid runtime flow: template lifecycle tools initialize
and build the project deterministically, while the agent still uses the general
vibecoding tools (`fs.*`, `package.install`, diagnostics, browser checks) for
source, content, and style generation. The default path should avoid interactive
`npm`/`npx` scaffold commands; official scaffold commands are allowed only when
wrapped by a non-interactive runtime tool with the same permission and audit
checks.

`package.install` defaults to the configured internal registry/proxy. Local E2E
debugging may explicitly opt into the public npm registry, but production-like
sandboxes keep public internet access denied by policy.

Runtime policy defaults to `production`. Public registry access and other local
E2E exceptions require an explicit `local-e2e` test/admin configuration and must
be audited. Preview startup uses the locked app root from `state/project.json`,
so build and preview operate on the same project tree.

When the runtime runs on the desktop host while sandboxes run in k3d, the default
workspace channel DNS name (`*.svc.cluster.local`) is not resolvable from the
host. For local E2E debugging, port-forward a sandbox pod's channel port and
start the runtime with:

```bash
SANDBOX_CHANNEL_HOST_OVERRIDE=127.0.0.1
SANDBOX_CHANNEL_PORT_OVERRIDE=<local-port>
```
