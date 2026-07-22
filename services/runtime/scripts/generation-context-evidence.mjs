function hasText(value) {
  return typeof value === "string" && value.trim().length > 0;
}

function sha256(value) {
  return typeof value === "string" && /^[a-f0-9]{64}$/.test(value);
}

function nonNegativeInteger(value) {
  return Number.isInteger(value) && value >= 0;
}

export function generationContextProtocol(value) {
  return value?.generationContext ? "generation-context" : "legacy";
}

export function validateGenerationContextRunEvidence(value, label = "run") {
  const errors = [];
  const context = value?.generationContext;
  if (!context) {
    errors.push(`${label}.generationContext is required`);
    return errors;
  }

  const isRunSummary = context.schemaVersion === "generation-context@1";
  const isStatus = context.schemaVersion === "generation-context-status@1";
  if (!isRunSummary && !isStatus) {
    errors.push(`${label}.generationContext.schemaVersion must be generation-context@1 or generation-context-status@1`);
  }
  if (isStatus && context.runContractVersion !== "generation-context@1") {
    errors.push(`${label}.generationContext.runContractVersion must be generation-context@1`);
  }
  if (context.status !== "compiled") {
    errors.push(`${label}.generationContext.status must be compiled`);
  }
  for (const field of ["contextContentHash", "runContextBindingHash", "runtimeAttestationHash"]) {
    if (!sha256(context[field])) errors.push(`${label}.generationContext.${field} must be sha256`);
  }
  if (!hasText(value?.templateVersion)) {
    errors.push(`${label}.templateVersion is required for generation-context evidence`);
  }

  const attestation = value?.attestation ?? context?.attestation;
  if (attestation !== undefined && attestation !== null) {
    if (attestation.state !== "verified") {
      errors.push(`${label}.attestation.state must be verified`);
    }
    if (!sha256(attestation.runtimeAttestationHash)
      || attestation.runtimeAttestationHash !== context.runtimeAttestationHash) {
      errors.push(`${label}.attestation.runtimeAttestationHash must match generationContext`);
    }
  }

  const efficiency = value?.efficiency;
  if (!efficiency || efficiency.schemaVersion !== "run-efficiency-metrics@1") {
    errors.push(`${label}.efficiency must use run-efficiency-metrics@1`);
  } else if (efficiency.calculatorVersion !== undefined) {
    if (efficiency.calculatorVersion !== "run-efficiency-calculator@1") {
      errors.push(`${label}.efficiency.calculatorVersion must be run-efficiency-calculator@1`);
    }
    for (const field of [
      "prebuildFsReadCount",
      "prebuildFsListCount",
      "prebuildFsSearchCount",
      "contextReadDeliveries",
      "sourceReadDeliveries",
      "fullReadDeliveries",
      "duplicateFullReadDeliveries",
      "duplicateFullReadRateBasisPoints",
      "duplicateReadEstimatedTokens",
    ]) {
      if (!nonNegativeInteger(efficiency[field])) {
        errors.push(`${label}.efficiency.${field} must be a non-negative integer`);
      }
    }
    if (efficiency.duplicateFullReadRateBasisPoints > 10_000) {
      errors.push(`${label}.efficiency.duplicateFullReadRateBasisPoints cannot exceed 10000`);
    }
  } else {
    for (const field of [
      "uniqueContextReads",
      "uniqueSourceReads",
      "duplicateReads",
      "duplicateReadTokens",
      "unchangedReadStubs",
      "postCompactSourceRestores",
      "prebuildLists",
    ]) {
      if (!nonNegativeInteger(efficiency[field])) {
        errors.push(`${label}.efficiency.${field} must be a non-negative integer`);
      }
    }
  }
  return errors;
}

export function validateLegacyReadEvidence(value, label = "run") {
  const errors = [];
  if (!sha256(value?.designContextContentHash)) {
    errors.push(`${label}.designContextContentHash must be sha256`);
  }
  const requiredReads = Array.isArray(value?.requiredReadPaths) ? value.requiredReadPaths : [];
  const readFiles = Array.isArray(value?.readFiles) ? value.readFiles : [];
  if (!requiredReads.length || requiredReads.some(path => !hasText(path) || !readFiles.includes(path))) {
    errors.push(`${label} must list every required DCP read and its recorded read evidence`);
  }
  if (value?.requiredReadsPassed !== true) {
    errors.push(`${label}.requiredReadsPassed must be true for legacy evidence`);
  }
  return errors;
}

export function validateDualReadRunEvidence(value, label = "run") {
  return generationContextProtocol(value) === "generation-context"
    ? validateGenerationContextRunEvidence(value, label)
    : validateLegacyReadEvidence(value, label);
}
