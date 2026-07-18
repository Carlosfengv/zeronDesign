import { pathToFileURL } from "node:url";

export function verifyRuntimeVersion(version, expectedShortCommit, expectedFullCommit, expectedImageRef) {
  if (!version || typeof version !== "object" || Array.isArray(version)) {
    throw new Error("Runtime version response must be an object");
  }
  if (!/^[0-9a-f]{12}$/.test(expectedShortCommit)) {
    throw new Error("Expected short commit must contain exactly 12 lowercase hexadecimal characters");
  }
  if (!/^[0-9a-f]{40}$/.test(expectedFullCommit)
    || !expectedFullCommit.startsWith(expectedShortCommit)) {
    throw new Error("Expected full commit must be the 40-character expansion of the short commit");
  }
  if (typeof expectedImageRef !== "string" || expectedImageRef.length === 0) {
    throw new Error("Expected Runtime image ref is required");
  }
  if (![expectedShortCommit, expectedFullCommit].includes(version.repositoryCommit)
    || version.imageRef !== expectedImageRef) {
    throw new Error(`Runtime version mismatch: ${JSON.stringify(version)}`);
  }
  return version;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [versionJson, expectedShortCommit, expectedFullCommit, expectedImageRef] = process.argv.slice(2);
  let version;
  try {
    version = JSON.parse(versionJson);
  } catch (error) {
    throw new Error(`Runtime version response is not valid JSON: ${error.message}`);
  }
  verifyRuntimeVersion(version, expectedShortCommit, expectedFullCommit, expectedImageRef);
}
