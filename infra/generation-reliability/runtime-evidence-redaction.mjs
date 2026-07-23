import crypto from "node:crypto";

function sha256(value) {
  return crypto.createHash("sha256").update(value).digest("hex");
}

function textDigest(value, prefix) {
  if (typeof value !== "string") return {};
  return {
    [`${prefix}Sha256`]: sha256(value),
    [`${prefix}Bytes`]: Buffer.byteLength(value),
  };
}

function copyDefined(source, fields) {
  return Object.fromEntries(
    fields
      .filter((field) => source?.[field] !== undefined)
      .map((field) => [field, source[field]]),
  );
}

function sensitiveTextKind(key) {
  const normalized = key.toLowerCase();
  if (/^(?:system|user|input|original|raw)?prompt(?:text|content)?$/.test(normalized)) {
    return "prompt";
  }
  if (/^(?:sourcecode|sourcetext|sourcecontent|fullsource|rawsource)$/.test(normalized)) {
    return "source";
  }
  return null;
}

export function extractBuildEvidence(events) {
  const effect = [...events]
    .reverse()
    .filter((event) => event.type === "tool.completed")
    .map((event) => event.metadata?.postToolUseSuccess)
    .find((metadata) =>
      metadata?.effect === "build_state_updated"
      || metadata?.effect === "candidate_state_updated");
  if (!effect) return null;
  const evidence = {
    buildId: effect.buildId || null,
    sourceSnapshotUri: effect.sourceSnapshotUri || null,
    sourceFingerprint: effect.sourceFingerprint || null,
    candidateManifestHash: effect.candidateManifestHash || null,
    artifactRouteManifestPath: effect.artifactRouteManifestPath || null,
    artifactRouteManifestHash: effect.artifactRouteManifestHash || null,
  };
  const hash = /^[a-f0-9]{64}$/;
  if (!evidence.buildId
    || !hash.test(evidence.sourceFingerprint || "")
    || !hash.test(evidence.candidateManifestHash || "")
    || !hash.test(evidence.artifactRouteManifestHash || "")
    || evidence.artifactRouteManifestPath !== ".anydesign-artifact-routes.json") {
    return null;
  }
  return evidence;
}

export function sanitizeModelExecutionEvent(event) {
  if (event?.type !== "model.execution" || !event.snapshot) return event;
  const { providerRequestId, ...snapshot } = event.snapshot;
  return {
    ...event,
    snapshot: {
      ...snapshot,
      providerRequestIdPresent:
        snapshot.providerRequestIdPresent === true
        || (typeof providerRequestId === "string" && providerRequestId.length > 0),
    },
  };
}

export function sanitizePersistedRuntimeEvent(input) {
  const event = sanitizeModelExecutionEvent(input);
  const base = copyDefined(event, ["type", "runId", "sequence", "timestamp"]);
  switch (event?.type) {
    case "model.execution":
      return {
        ...base,
        snapshot: copyDefined(event.snapshot, [
          "id",
          "modelResourceId",
          "modelResourceRevision",
          "physicalModel",
          "displayName",
          "providerId",
          "providerAttemptCount",
          "selectionPolicyId",
          "selectionPolicyRevision",
          "selectionReason",
          "capabilitySnapshotHash",
          "providerRequestIdPresent",
        ]),
      };
    case "model.usage":
      return {
        ...base,
        ...copyDefined(event, [
          "turn",
          "inputTokens",
          "cachedInputTokens",
          "outputTokens",
          "estimated",
        ]),
      };
    case "prompt.composition":
      return {
        ...base,
        ...copyDefined(event, [
          "turn",
          "staticPrefixHash",
          "toolSetHashVersion",
          "toolSetHash",
          "estimatedInputTokens",
        ]),
      };
    case "agent.message":
      return { ...base, ...textDigest(event.text, "text") };
    case "tool.output":
      return {
        ...base,
        ...copyDefined(event, ["tool", "toolUseId", "stream"]),
        ...textDigest(event.text, "text"),
      };
    case "tool.failed":
      return {
        ...base,
        ...copyDefined(event, ["tool", "toolUseId", "recoverable"]),
        errorKind: event.metadata?.errorKind ?? null,
        ...textDigest(event.error, "error"),
      };
    case "tool.started":
    case "tool.completed":
      return {
        ...base,
        ...copyDefined(event, ["tool", "toolName", "toolUseId", "recoverable"]),
        errorKind: event.metadata?.errorKind ?? null,
      };
    case "run.completed":
      return {
        ...base,
        ...copyDefined(event, ["status"]),
        ...textDigest(event.summary, "summary"),
      };
    case "metric.recorded":
      return {
        ...base,
        ...copyDefined(event, ["name", "value", "unit", "phase"]),
      };
    default:
      return {
        ...base,
        ...copyDefined(event, [
          "status",
          "phase",
          "stage",
          "turn",
          "substantive",
          "estimated",
          "name",
        ]),
      };
  }
}

export function redactEvidenceObject(value, redactMessage = false) {
  if (Array.isArray(value)) {
    return value.map((item) => redactEvidenceObject(item, redactMessage));
  }
  if (!value || typeof value !== "object") return value;
  const result = {};
  const errorLike = redactMessage
    || (typeof value.name === "string" && typeof value.message === "string");
  for (const [key, child] of Object.entries(value)) {
    const sensitiveText = sensitiveTextKind(key);
    if (sensitiveText && typeof child === "string") {
      Object.assign(result, textDigest(child, key));
      continue;
    }
    if (sensitiveText) {
      result[`${key}Present`] = child !== null && child !== undefined;
      continue;
    }
    if (key === "summary" && typeof child === "string") {
      Object.assign(result, textDigest(child, "summary"));
      continue;
    }
    if (key === "message" && errorLike && typeof child === "string") {
      Object.assign(result, textDigest(child, "message"));
      continue;
    }
    if (key === "providerRequestId") {
      result.providerRequestIdPresent = typeof child === "string" && child.length > 0;
      continue;
    }
    if (["authorization", "apiKey", "providerKey", "screenshotPixels"].includes(key)) {
      result[`${key}Present`] = child !== null && child !== undefined && child !== "";
      continue;
    }
    result[key] = redactEvidenceObject(
      child,
      key === "error" || key === "cleanupError",
    );
  }
  return result;
}

export function evidenceRedactionViolations(value, location = "$") {
  if (Array.isArray(value)) {
    return value.flatMap((item, index) =>
      evidenceRedactionViolations(item, `${location}[${index}]`));
  }
  if (!value || typeof value !== "object") return [];
  const violations = [];
  const forbidden = new Set([
    "authorization",
    "apikey",
    "providerkey",
    "providerrequestid",
    "screenshotpixels",
  ]);
  for (const [key, child] of Object.entries(value)) {
    const childLocation = `${location}.${key}`;
    if (forbidden.has(key.toLowerCase()) || sensitiveTextKind(key)) violations.push(childLocation);
    if (key === "summary" && typeof child === "string") violations.push(childLocation);
    if (key === "message" && /(?:^|\.)(?:error|cleanupError)(?:\.|$)/.test(location)) {
      violations.push(childLocation);
    }
    if (value.type === "agent.message" && key === "text") violations.push(childLocation);
    if (value.type === "tool.output" && key === "text") violations.push(childLocation);
    if (value.type === "tool.failed" && key === "error") violations.push(childLocation);
    violations.push(...evidenceRedactionViolations(child, childLocation));
  }
  return [...new Set(violations)];
}
