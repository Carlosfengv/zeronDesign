#!/usr/bin/env node

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

const PROOF_SCHEMA = "generation-context-monitoring-proof@1";
const REQUIRED_METRICS = [
  "generation_context_compile_total",
  "agent_time_to_first_mutation_seconds_count",
  "draft_hmr_iframe_applied_seconds_count",
  "draft_snapshot_durable_seconds_count",
  "agent_successor_run_created_total",
];
const FORBIDDEN_LABEL = /^(?:project|project_?id|run|run_?id|path|file|model|model_?resource)$/i;

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function apiUrl(baseUrl, pathname, parameters = {}) {
  const url = new URL(pathname, baseUrl.endsWith("/") ? baseUrl : `${baseUrl}/`);
  for (const [key, value] of Object.entries(parameters)) url.searchParams.set(key, value);
  return url;
}

function endpointIdentity(baseUrl) {
  const url = new URL(baseUrl);
  if (!["http:", "https:"].includes(url.protocol)) throw new Error("Prometheus URL must use http or https");
  if (url.username || url.password || url.search || url.hash) {
    throw new Error("Prometheus URL must not contain credentials, query parameters, or a fragment");
  }
  return sha256(`${url.protocol}//${url.host}${url.pathname.replace(/\/$/, "")}`);
}

async function prometheusJson(fetchImpl, baseUrl, pathname, parameters, headers, timeoutMs) {
  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);
  try {
    const response = await fetchImpl(apiUrl(baseUrl, pathname, parameters), {
      headers,
      signal: controller.signal,
    });
    const body = await response.text();
    if (!response.ok) throw new Error(`Prometheus ${pathname} returned HTTP ${response.status}`);
    let document;
    try {
      document = JSON.parse(body);
    } catch {
      throw new Error(`Prometheus ${pathname} returned invalid JSON`);
    }
    if (document.status !== "success") throw new Error(`Prometheus ${pathname} did not return success`);
    return document.data;
  } finally {
    clearTimeout(timer);
  }
}

function targetProof(target, nowMs, maxScrapeAgeMs) {
  if (target.health !== "up") throw new Error(`Generation Context Prometheus target is not UP: ${target.health || "unknown"}`);
  if (String(target.lastError || "").trim()) throw new Error("Generation Context Prometheus target reports a scrape error");
  const lastScrapeMs = Date.parse(target.lastScrape);
  if (!Number.isFinite(lastScrapeMs) || nowMs - lastScrapeMs < 0 || nowMs - lastScrapeMs > maxScrapeAgeMs) {
    throw new Error("Generation Context Prometheus target has no fresh successful scrape");
  }
  const safeLabels = {};
  for (const key of ["namespace", "service", "job", "endpoint", "pod"]) {
    if (typeof target.labels?.[key] === "string" && target.labels[key]) safeLabels[key] = target.labels[key];
  }
  const identity = {
    scrapePool: target.scrapePool,
    scrapeUrl: target.scrapeUrl,
    labels: target.labels,
    discoveredLabels: target.discoveredLabels,
  };
  return {
    health: "up",
    lastScrape: new Date(lastScrapeMs).toISOString(),
    scrapeAgeSeconds: Math.round((nowMs - lastScrapeMs) / 1000),
    lastScrapeDurationSeconds:
      typeof target.lastScrapeDuration === "number" && Number.isFinite(target.lastScrapeDuration)
        ? target.lastScrapeDuration
        : null,
    labels: safeLabels,
    identitySha256: sha256(JSON.stringify(identity)),
  };
}

function metricProof(name, result) {
  if (!Array.isArray(result) || !result.length) throw new Error(`required live series is missing: ${name}`);
  const labelNames = new Set();
  let total = 0;
  let latestSampleAt = 0;
  for (const [index, series] of result.entries()) {
    if (series?.metric?.__name__ !== name) throw new Error(`${name} result[${index}] has the wrong metric name`);
    for (const label of Object.keys(series.metric || {})) {
      if (label === "__name__") continue;
      if (FORBIDDEN_LABEL.test(label)) throw new Error(`${name} exposes forbidden high-cardinality label: ${label}`);
      labelNames.add(label);
    }
    const timestamp = Number(series.value?.[0]);
    const value = Number(series.value?.[1]);
    if (!Number.isFinite(timestamp) || !Number.isFinite(value) || value < 0) {
      throw new Error(`${name} result[${index}] has an invalid sample`);
    }
    latestSampleAt = Math.max(latestSampleAt, timestamp);
    total += value;
  }
  if (total <= 0) throw new Error(`required live series has no canary observation: ${name}`);
  return {
    name,
    seriesCount: result.length,
    labelNames: [...labelNames].sort(),
    observedValueSum: total,
    latestSampleAt: new Date(latestSampleAt * 1000).toISOString(),
  };
}

export async function verifyGenerationContextMonitoring({
  baseUrl,
  bearerToken = "",
  fetchImpl = fetch,
  now = new Date(),
  timeoutMs = 15_000,
  maxScrapeAgeMs = 120_000,
  expectedNamespace = "anydesign-runtime",
  expectedService = "anydesign-runtime",
}) {
  const endpointSha256 = endpointIdentity(baseUrl);
  const headers = bearerToken ? { authorization: `Bearer ${bearerToken}` } : {};
  const [targets, build] = await Promise.all([
    prometheusJson(fetchImpl, baseUrl, "/api/v1/targets", { state: "active" }, headers, timeoutMs),
    prometheusJson(fetchImpl, baseUrl, "/api/v1/status/buildinfo", {}, headers, timeoutMs),
  ]);
  const matchingTargets = (targets?.activeTargets || []).filter((target) => {
    let pathname = "";
    try {
      pathname = new URL(target.scrapeUrl).pathname;
    } catch {
      return false;
    }
    return pathname === "/internal/metrics/generation-context"
      && target.labels?.namespace === expectedNamespace
      && target.labels?.service === expectedService;
  });
  if (matchingTargets.length !== 1) {
    throw new Error(`expected exactly one Generation Context target, found ${matchingTargets.length}`);
  }
  const target = targetProof(matchingTargets[0], now.getTime(), maxScrapeAgeMs);
  const queries = await Promise.all(REQUIRED_METRICS.map(async (name) => {
    const data = await prometheusJson(fetchImpl, baseUrl, "/api/v1/query", { query: name }, headers, timeoutMs);
    if (data?.resultType !== "vector") throw new Error(`${name} query did not return a vector`);
    return metricProof(name, data.result);
  }));
  return {
    schemaVersion: PROOF_SCHEMA,
    recordedAt: now.toISOString(),
    prometheusEndpointSha256: endpointSha256,
    prometheus: {
      version: String(build?.version || ""),
      revision: String(build?.revision || ""),
    },
    target,
    queries,
    passed: true,
  };
}

function readBearerToken() {
  const tokenFile = String(process.env.PROMETHEUS_AUTH_TOKEN_FILE || "").trim();
  if (!tokenFile) return "";
  const token = fs.readFileSync(tokenFile, "utf8").trim();
  if (!token) throw new Error("PROMETHEUS_AUTH_TOKEN_FILE is empty");
  return token;
}

function writeExclusive(file, value) {
  fs.mkdirSync(path.dirname(path.resolve(file)), { recursive: true });
  const descriptor = fs.openSync(file, "wx", 0o600);
  try {
    fs.writeFileSync(descriptor, `${JSON.stringify(value, null, 2)}\n`);
  } finally {
    fs.closeSync(descriptor);
  }
}

async function main() {
  const [baseUrl, outputFile] = process.argv.slice(2);
  if (!baseUrl || !outputFile) {
    throw new Error("usage: verify-generation-context-monitoring.mjs <prometheus-base-url> <proof.json>");
  }
  const proof = await verifyGenerationContextMonitoring({
    baseUrl,
    bearerToken: readBearerToken(),
    timeoutMs: Number.parseInt(process.env.GENERATION_MONITORING_TIMEOUT_MS || "15000", 10),
    maxScrapeAgeMs: Number.parseInt(process.env.GENERATION_MONITORING_MAX_SCRAPE_AGE_MS || "120000", 10),
    expectedNamespace: process.env.GENERATION_MONITORING_RUNTIME_NAMESPACE || "anydesign-runtime",
    expectedService: process.env.GENERATION_MONITORING_RUNTIME_SERVICE || "anydesign-runtime",
  });
  writeExclusive(outputFile, proof);
  process.stdout.write(`Generation Context monitoring proof passed: ${outputFile}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    process.stderr.write(`${error?.stack || error}\n`);
    process.exitCode = 1;
  });
}
