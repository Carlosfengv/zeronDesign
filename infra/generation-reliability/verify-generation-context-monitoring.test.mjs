#!/usr/bin/env node

import assert from "node:assert/strict";
import { verifyGenerationContextMonitoring } from "./verify-generation-context-monitoring.mjs";

const now = new Date("2026-07-20T12:00:00.000Z");
const metricNames = new Set([
  "generation_context_compile_total",
  "agent_time_to_first_mutation_seconds_count",
  "draft_hmr_iframe_applied_seconds_count",
  "draft_snapshot_durable_seconds_count",
  "agent_successor_run_created_total",
]);

function response(value) {
  return {
    ok: true,
    status: 200,
    async text() {
      return JSON.stringify({ status: "success", data: value });
    },
  };
}

function mockFetch({ health = "up", metricLabel = null, metricValue = "1", targetCount = 1 } = {}) {
  return async (url, options) => {
    assert.equal(options.headers.authorization, "Bearer workload-proof-token");
    if (url.pathname === "/api/v1/targets") {
      const target = {
        scrapePool: "serviceMonitor/monitoring/anydesign-runtime-generation-context/0",
        scrapeUrl: "http://10.0.0.7:8080/internal/metrics/generation-context",
        health,
        lastError: "",
        lastScrape: "2026-07-20T11:59:30.000Z",
        lastScrapeDuration: 0.02,
        labels: {
          namespace: "anydesign-runtime",
          service: "anydesign-runtime",
          job: "anydesign-runtime",
          endpoint: "http",
        },
        discoveredLabels: { __address__: "10.0.0.7:8080" },
      };
      return response({ activeTargets: Array.from({ length: targetCount }, () => target) });
    }
    if (url.pathname === "/api/v1/status/buildinfo") {
      return response({ version: "3.4.1", revision: "prom-revision" });
    }
    if (url.pathname === "/api/v1/query") {
      const name = url.searchParams.get("query");
      assert(metricNames.has(name));
      return response({
        resultType: "vector",
        result: [{
          metric: { __name__: name, phase: "edit", ...(metricLabel ? { [metricLabel]: "unsafe" } : {}) },
          value: [now.getTime() / 1000, metricValue],
        }],
      });
    }
    throw new Error(`unexpected URL: ${url}`);
  };
}

const proof = await verifyGenerationContextMonitoring({
  baseUrl: "http://prometheus.monitoring.svc:9090",
  bearerToken: "workload-proof-token",
  fetchImpl: mockFetch(),
  now,
});
assert.equal(proof.passed, true);
assert.equal(proof.target.health, "up");
assert.equal(proof.queries.length, 5);
assert(!JSON.stringify(proof).includes("workload-proof-token"));
assert(!JSON.stringify(proof).includes("10.0.0.7"));

await assert.rejects(
  verifyGenerationContextMonitoring({
    baseUrl: "http://prometheus.monitoring.svc:9090",
    bearerToken: "workload-proof-token",
    fetchImpl: mockFetch({ health: "down" }),
    now,
  }),
  /not UP/,
);
await assert.rejects(
  verifyGenerationContextMonitoring({
    baseUrl: "http://prometheus.monitoring.svc:9090",
    bearerToken: "workload-proof-token",
    fetchImpl: mockFetch({ metricLabel: "run_id" }),
    now,
  }),
  /forbidden high-cardinality label/,
);
await assert.rejects(
  verifyGenerationContextMonitoring({
    baseUrl: "http://prometheus.monitoring.svc:9090",
    bearerToken: "workload-proof-token",
    fetchImpl: mockFetch({ metricValue: "0" }),
    now,
  }),
  /no canary observation/,
);
await assert.rejects(
  verifyGenerationContextMonitoring({
    baseUrl: "http://prometheus.monitoring.svc:9090",
    bearerToken: "workload-proof-token",
    fetchImpl: mockFetch({ targetCount: 0 }),
    now,
  }),
  /exactly one/,
);

process.stdout.write("Generation Context monitoring proof tests passed.\n");
