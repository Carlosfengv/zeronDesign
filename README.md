# zeronDesign

## Runtime model providers

The runtime keeps the existing internal model gateway as the default:

```bash
MODEL_PROVIDER=internal_gateway
MODEL_GATEWAY_URL=http://localhost:9000
```

Real OpenAI-compatible providers can be enabled with environment variables:

```bash
# DeepSeek
MODEL_PROVIDER=deepseek
DEEPSEEK_API_KEY=...
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
