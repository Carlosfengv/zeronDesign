export async function confirmSandboxRelease({
  requestRelease,
  maxAttempts,
  retryCooldownMs,
  requiredSuccessfulResponses = 2,
  delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds)),
}) {
  if (typeof requestRelease !== "function") {
    throw new Error("requestRelease must be a function");
  }
  if (!Number.isSafeInteger(maxAttempts) || maxAttempts < requiredSuccessfulResponses) {
    throw new Error("maxAttempts must cover every required successful response");
  }
  if (!Number.isSafeInteger(requiredSuccessfulResponses) || requiredSuccessfulResponses < 1) {
    throw new Error("requiredSuccessfulResponses must be a positive integer");
  }
  if (!Number.isSafeInteger(retryCooldownMs) || retryCooldownMs < 0) {
    throw new Error("retryCooldownMs must be a non-negative integer");
  }
  const attempts = [];
  let successfulResponses = 0;
  for (let attempt = 1; attempt <= maxAttempts; attempt += 1) {
    const release = await requestRelease();
    const normalized = {
      ok: release?.ok === true,
      status: Number.isSafeInteger(release?.status) ? release.status : null,
      error: release?.ok === true ? null : String(release?.error || "release_request_failed"),
    };
    attempts.push({ attempt, ...normalized });
    if (normalized.ok) {
      successfulResponses += 1;
      if (successfulResponses >= requiredSuccessfulResponses) {
        return {
          required: true,
          released: true,
          attempts,
          maxAttempts,
          requiredSuccessfulResponses,
        };
      }
    }
    if (attempt < maxAttempts && retryCooldownMs > 0) {
      await delay(retryCooldownMs);
    }
  }
  return {
    required: true,
    released: false,
    attempts,
    maxAttempts,
    requiredSuccessfulResponses,
  };
}

export function attachSandboxReleaseEvidence(evidence, sandboxRelease) {
  const finalized = { ...evidence, sandboxRelease };
  if (sandboxRelease?.required === true && sandboxRelease?.released !== true) {
    return {
      ...finalized,
      status: "failed",
      cleanupError: {
        name: "Error",
        classification: "sandbox_release_failed",
        message: `sandbox release failed after ${sandboxRelease?.attempts?.length || 0} attempts`,
      },
    };
  }
  return finalized;
}
