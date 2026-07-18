import assert from "node:assert/strict";
import { verifyRuntimeVersion } from "./verify-runtime-version.mjs";

const shortCommit = "ff7e7dc8713b";
const fullCommit = "ff7e7dc8713ba72110d0cc68fd99b8e855931258";
const imageRef = "anydesign/runtime:reliability-test";

for (const repositoryCommit of [shortCommit, fullCommit]) {
  const version = {
    service: "anydesign-runtime",
    repositoryCommit,
    repositoryDirty: true,
    imageRef,
  };
  assert.equal(
    verifyRuntimeVersion(version, shortCommit, fullCommit, imageRef),
    version,
  );
}

assert.throws(
  () => verifyRuntimeVersion(
    { repositoryCommit: "000000000000", imageRef },
    shortCommit,
    fullCommit,
    imageRef,
  ),
  /Runtime version mismatch/,
);
assert.throws(
  () => verifyRuntimeVersion(
    { repositoryCommit: fullCommit, imageRef: "anydesign/runtime:wrong" },
    shortCommit,
    fullCommit,
    imageRef,
  ),
  /Runtime version mismatch/,
);
assert.throws(
  () => verifyRuntimeVersion({}, "too-short", fullCommit, imageRef),
  /Expected short commit/,
);

process.stdout.write("Runtime version contract tests passed\n");
