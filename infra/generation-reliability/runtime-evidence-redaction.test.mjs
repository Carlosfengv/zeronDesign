import assert from "node:assert/strict";
import {
  evidenceRedactionViolations,
  extractBuildEvidence,
  redactEvidenceObject,
  sanitizeModelExecutionEvent,
  sanitizePersistedRuntimeEvent,
} from "./runtime-evidence-redaction.mjs";

const secret = "sk-secret-material-must-not-persist";
const source = "export default function Page() { return <main>private source</main>; }";
const message = sanitizePersistedRuntimeEvent({
  type: "agent.message",
  runId: "run-1",
  text: source,
});
assert.equal(message.text, undefined);
assert.equal(message.textBytes, Buffer.byteLength(source));
assert.match(message.textSha256, /^[a-f0-9]{64}$/);

const output = sanitizePersistedRuntimeEvent({
  type: "tool.output",
  runId: "run-1",
  tool: "fs.read",
  stream: "stdout",
  text: `${secret}\n${source}`,
});
assert.equal(output.text, undefined);
assert.match(output.textSha256, /^[a-f0-9]{64}$/);
assert.ok(!JSON.stringify(output).includes(secret));
assert.ok(!JSON.stringify(output).includes(source));

const failed = sanitizePersistedRuntimeEvent({
  type: "tool.failed",
  tool: "fs.patch",
  error: `failed around ${source}`,
  metadata: { errorKind: "source.patch_failed", suggestedAction: secret },
});
assert.equal(failed.error, undefined);
assert.equal(failed.errorKind, "source.patch_failed");
assert.ok(!JSON.stringify(failed).includes(secret));
assert.ok(!JSON.stringify(failed).includes(source));

const execution = sanitizeModelExecutionEvent({
  type: "model.execution",
  snapshot: {
    modelResourceId: "model-1",
    displayName: "Model One",
    providerRequestId: secret,
  },
});
assert.equal(execution.snapshot.providerRequestId, undefined);
assert.equal(execution.snapshot.providerRequestIdPresent, true);
const persistedExecution = sanitizePersistedRuntimeEvent(execution);
assert.equal(persistedExecution.snapshot.modelResourceId, "model-1");
assert.equal(persistedExecution.snapshot.displayName, "Model One");
assert.equal(persistedExecution.snapshot.providerRequestIdPresent, true);
assert.ok(!JSON.stringify(persistedExecution).includes(secret));

const composition = sanitizePersistedRuntimeEvent({
  type: "prompt.composition",
  turn: 1,
  staticPrefixHash: "a".repeat(64),
  toolSetHashVersion: "tool-definition-set@1",
  toolSetHash: "b".repeat(64),
  estimatedInputTokens: 100,
});
assert.equal(composition.toolSetHashVersion, "tool-definition-set@1");
assert.equal(composition.toolSetHash, "b".repeat(64));

const evidence = redactEvidenceObject({
  prompt: "private user prompt",
  systemPrompt: "private system prompt",
  userPromptText: "private alternate user prompt",
  sourceText: source,
  rawSource: source,
  run: { summary: `terminal output containing ${source}` },
  error: { name: "Error", message: `failure containing ${secret}` },
  expectedText: "public acceptance marker",
});
const serialized = JSON.stringify(evidence);
assert.equal(evidence.prompt, undefined);
assert.equal(evidence.systemPrompt, undefined);
assert.match(evidence.systemPromptSha256, /^[a-f0-9]{64}$/);
assert.equal(evidence.userPromptText, undefined);
assert.match(evidence.userPromptTextSha256, /^[a-f0-9]{64}$/);
assert.equal(evidence.sourceText, undefined);
assert.match(evidence.sourceTextSha256, /^[a-f0-9]{64}$/);
assert.equal(evidence.rawSource, undefined);
assert.match(evidence.rawSourceSha256, /^[a-f0-9]{64}$/);
assert.equal(evidence.run.summary, undefined);
assert.equal(evidence.error.message, undefined);
assert.equal(evidence.expectedText, "public acceptance marker");
assert.ok(!serialized.includes("private user prompt"));
assert.ok(!serialized.includes(source));
assert.ok(!serialized.includes(secret));
assert.deepEqual(evidenceRedactionViolations(evidence), []);
assert.deepEqual(evidenceRedactionViolations({
  systemPrompt: "private",
  userPromptText: "private",
  sourceText: source,
  rawSource: source,
}), [
  "$.systemPrompt",
  "$.userPromptText",
  "$.sourceText",
  "$.rawSource",
]);

assert.deepEqual(extractBuildEvidence([{
  type: "tool.completed",
  tool: "preview.publish",
  metadata: { postToolUseSuccess: {
    effect: "candidate_state_updated",
    buildId: "build-1",
    sourceSnapshotUri: "runtime://source-snapshots/project/build-1",
    sourceFingerprint: "a".repeat(64),
    candidateManifestHash: "b".repeat(64),
    artifactRouteManifestPath: ".anydesign-artifact-routes.json",
    artifactRouteManifestHash: "c".repeat(64),
  } },
}]), {
  buildId: "build-1",
  sourceSnapshotUri: "runtime://source-snapshots/project/build-1",
  sourceFingerprint: "a".repeat(64),
  candidateManifestHash: "b".repeat(64),
  artifactRouteManifestPath: ".anydesign-artifact-routes.json",
  artifactRouteManifestHash: "c".repeat(64),
});

process.stdout.write("Runtime evidence redaction tests passed\n");
