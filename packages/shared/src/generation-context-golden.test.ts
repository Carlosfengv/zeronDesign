import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

const vector = JSON.parse(
  readFileSync(
    new URL("../fixtures/generation-context-golden-vector.json", import.meta.url),
    "utf8",
  ),
) as {
  compilerVersion: string;
  payload: unknown;
  contextContentHash: string;
  runBindingInput: {
    runId: string;
    projectId: string;
    workspaceNamespace: string;
    frozenResourcesHash: string;
    runtimeAttestationHash: string;
  };
  runContextBindingHash: string;
};

function canonical(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(canonical);
  if (value && typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, child]) => [key, canonical(child)]),
    );
  }
  return value;
}

function hash(value: unknown): string {
  return createHash("sha256").update(JSON.stringify(canonical(value))).digest("hex");
}

describe("GenerationContext golden vector", () => {
  it("matches Rust content and per-run binding hashes", () => {
    const contextContentHash = hash({
      domain: "generation-context-content-hash@1",
      compilerVersion: vector.compilerVersion,
      payload: vector.payload,
    });
    expect(contextContentHash).toBe(vector.contextContentHash);
    expect(
      hash({
        domain: "run-context-binding-hash@1",
        contextContentHash,
        ...vector.runBindingInput,
      }),
    ).toBe(vector.runContextBindingHash);
  });
});
