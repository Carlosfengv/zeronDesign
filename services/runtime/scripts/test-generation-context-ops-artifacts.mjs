#!/usr/bin/env node

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const schema = JSON.parse(await readFile(
  new URL("../evidence/generation-context-run-evidence.schema.json", import.meta.url),
  "utf8",
));
assert.equal(schema.$schema, "https://json-schema.org/draft/2020-12/schema");
assert.equal(schema.oneOf.length, 2);
assert(schema.oneOf.some(branch => branch.title === "GenerationContext@1"));
assert(schema.oneOf.some(branch => branch.title === "Legacy DCP read protocol"));
for (const field of ["contextContentHash", "runContextBindingHash", "runtimeAttestationHash"]) {
  assert(schema.$defs.generationContext.required.includes(field));
}

const dashboard = JSON.parse(await readFile(
  new URL("../../../infra/generation-reliability/generation-context-dashboard.json", import.meta.url),
  "utf8",
));
assert(dashboard.panels.length >= 8);
const queries = dashboard.panels.flatMap(panel => panel.targets ?? []).map(target => target.expr);
for (const metric of [
  "generation_context_compile_total",
  "agent_time_to_first_mutation_seconds_bucket",
  "cold_dev_ready_seconds_bucket",
  "draft_hmr_iframe_applied_seconds_bucket",
  "agent_time_to_first_greenfield_build_seconds_bucket",
  "draft_snapshot_durable_seconds_bucket",
  "agent_duplicate_read_total",
  "agent_out_of_scope_mutation_total",
  "agent_successor_run_created_total",
]) {
  assert(queries.some(query => query.includes(metric)), `dashboard is missing ${metric}`);
}
for (const query of queries) {
  assert(!/(projectId|project_id|runId|run_id|path=)/.test(query), "dashboard query uses a forbidden high-cardinality label");
}

const serviceMonitor = await readFile(
  new URL("../../../infra/generation-reliability/generation-context-servicemonitor.yaml", import.meta.url),
  "utf8",
);
assert.match(serviceMonitor, /kind: ServiceMonitor/);
assert.match(serviceMonitor, /path: \/internal\/metrics\/generation-context/);
assert.match(serviceMonitor, /authorization:\s*\n\s*type: Bearer/);
assert.match(serviceMonitor, /name: anydesign-runtime-internal-admin\s*\n\s*key: token/);
assert(!/bearerToken:\s*\S+/.test(serviceMonitor), "ServiceMonitor must not contain an inline bearer token");

const kustomization = await readFile(
  new URL("../../../infra/generation-reliability/kustomization.yaml", import.meta.url),
  "utf8",
);
assert.match(kustomization, /generation-context-servicemonitor\.yaml/);
assert.match(kustomization, /generation-context-dashboard\.json/);
assert.match(kustomization, /grafana_dashboard: "1"/);

process.stdout.write("generation-context operations artifact tests passed\n");
