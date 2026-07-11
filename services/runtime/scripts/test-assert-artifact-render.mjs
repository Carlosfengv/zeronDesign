#!/usr/bin/env node

import assert from "node:assert/strict";
import { validateComputedStyleResult } from "./assert-artifact-render.mjs";

const valid = validateComputedStyleResult({
  ok: true,
  results: {
    "artifact-body-display": { values: ["block"] },
    "artifact-body-color": { values: ["rgb(20, 30, 40)"] },
    "artifact-body-font": { values: ["Inter, sans-serif"] },
  },
});
assert.equal(valid.passed, true);
assert.equal(valid.display, "block");
assert.throws(() => validateComputedStyleResult({
  ok: true,
  results: {
    "artifact-body-display": { values: ["none"] },
    "artifact-body-color": { values: ["rgb(0, 0, 0)"] },
    "artifact-body-font": { values: ["sans-serif"] },
  },
}), /not rendered/);
assert.throws(() => validateComputedStyleResult({ ok: false, error: "browser failed" }), /browser failed/);

console.log("artifact render assertion tests passed");
