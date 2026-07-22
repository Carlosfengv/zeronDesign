const assert = require("node:assert/strict");
const {
  resetFixtureState,
  responseForBody,
} = require("./fixture-model-gateway.js");

function request(runId, projectId, phase, turn) {
  return {
    schemaVersion: "provider-gateway-turn-request@1",
    scope: {
      runId,
      projectId,
      phase,
      turn,
    },
    input: {
      systemPrompt: `Project: ${projectId}`,
      messages: [],
      tools: [],
      deferredTools: [],
    },
  };
}

resetFixtureState();

const websiteBriefOne = responseForBody(
  request("run-website-brief", "rc-website-fixture", "brief", 1),
);
const docsBriefOne = responseForBody(
  request("run-docs-brief", "rc-docs-fixture", "brief", 1),
);
assert.deepEqual(
  websiteBriefOne.toolCalls.map((call) => call.name),
  ["content.list_sources", "content.read_source"],
);
assert.deepEqual(
  docsBriefOne.toolCalls.map((call) => call.name),
  ["content.list_sources", "content.read_source"],
);
assert.ok(websiteBriefOne.toolCalls.every((call) => call.id.startsWith("run-website-brief:")));
assert.ok(docsBriefOne.toolCalls.every((call) => call.id.startsWith("run-docs-brief:")));
assert.equal(
  new Set([...websiteBriefOne.toolCalls, ...docsBriefOne.toolCalls].map((call) => call.id)).size,
  4,
  "fixture tool-call ids must be unique across concurrent Runs",
);

const websiteBriefTwo = responseForBody(
  request("run-website-brief", "rc-website-fixture", "brief", 2),
);
const docsBriefTwo = responseForBody(
  request("run-docs-brief", "rc-docs-fixture", "brief", 2),
);
assert.equal(websiteBriefTwo.toolCalls[0].name, "brief.write_draft");
assert.equal(websiteBriefTwo.toolCalls[0].input.recommendedTemplate, "next-app");
assert.equal(docsBriefTwo.toolCalls[0].name, "brief.write_draft");
assert.equal(docsBriefTwo.toolCalls[0].input.recommendedTemplate, "fumadocs-docs");

const duplicateDocsBriefTwo = responseForBody(
  request("run-docs-brief", "rc-docs-fixture", "brief", 2),
);
assert.deepEqual(duplicateDocsBriefTwo, docsBriefTwo);

const docsBuildOne = responseForBody(
  request("run-docs-build", "rc-docs-fixture", "build", 1),
);
assert.equal(docsBuildOne.toolCalls[0].name, "project.init");
assert.equal(docsBuildOne.toolCalls[0].input.template, "fumadocs-docs");

const docsBuildTwo = responseForBody(
  request("run-docs-build", "rc-docs-fixture", "build", 2),
);
assert.deepEqual(
  docsBuildTwo.toolCalls.map((call) => [call.name, call.input.path]),
  [
    ["fs.read", "project/package.json"],
    ["fs.read", "project/content/docs/index.mdx"],
  ],
);
const docsBuildThree = responseForBody(
  request("run-docs-build", "rc-docs-fixture", "build", 3),
);
const docsBuildScript = docsBuildThree.toolCalls.find(
  (call) => call.name === "fs.write" && call.input.path === "project/build.cjs",
)?.input.text;
assert.match(
  docsBuildScript,
  /rmSync\('out',\{recursive:true,force:true\}\)/,
  "the Docs fixture must remove stale output inherited from a reused warm-pool workspace",
);
assert.match(
  docsBuildScript,
  /out\/docs\.html[\s\S]*shell[\s\S]*RC Docs Overview/,
  "the validated /docs route must render the navigation and search shell",
);
assert.match(
  docsBuildScript,
  /href="\/docs\/#overview"/,
  "the Docs fixture must link to the canonical /docs/ fragment route",
);
assert.doesNotMatch(
  docsBuildScript,
  /href="\.\/docs#overview"/,
  "a route-relative Docs link would resolve to the broken /docs/docs route",
);

responseForBody(request("run-website-css", "rc-website-css", "build", 1));
const websiteBuildTwo = responseForBody(
  request("run-website-css", "rc-website-css", "build", 2),
);
assert.deepEqual(
  websiteBuildTwo.toolCalls.map((call) => [call.name, call.input.path]),
  [["fs.read", "project/app/page.tsx"]],
);
const websiteBuildThree = responseForBody(
  request("run-website-css", "rc-website-css", "build", 3),
);
const websiteBuildScript = websiteBuildThree.toolCalls.find(
  (call) => call.name === "fs.write" && call.input.path === "project/build.cjs",
)?.input.text;
assert.equal(websiteBuildScript, undefined, "next-app must retain its Runtime-owned build contract");
assert.ok(
  !websiteBuildThree.toolCalls.some(
    (call) => call.name === "fs.write" && call.input.path === "project/package.json",
  ),
  "next-app must not mutate its protected package contract",
);
const websitePageSource = websiteBuildThree.toolCalls.find(
  (call) => call.name === "fs.write" && call.input.path === "project/app/page.tsx",
)?.input.text;
assert.match(websitePageSource, /RC Website Built/);
assert.match(websitePageSource, /<nav aria-label="Primary">/);

function collectToolNames(runId, projectId, phase, turns) {
  return Array.from({ length: turns }, (_, index) =>
    responseForBody(request(runId, projectId, phase, index + 1)),
  ).flatMap((response) => response.toolCalls?.map((call) => call.name) || []);
}

function assertDependentToolsAreSerialized(runId, projectId, phase, turns) {
  const dependentTools = new Set([
    "project.build",
    "preview.start",
    "draft.snapshot_create",
    "browser.open",
    "browser.screenshot",
    "preview.publish",
    "run.complete",
  ]);
  for (let index = 0; index < turns; index += 1) {
    const response = responseForBody(request(runId, projectId, phase, index + 1));
    const calls = (response.toolCalls || []).filter((call) => dependentTools.has(call.name));
    assert.ok(
      calls.length <= 1,
      `dependent lifecycle tools must be serialized: ${calls.map((call) => call.name).join(",")}`,
    );
  }
}

const websiteBuildTools = collectToolNames(
  "run-website-draft-contract",
  "rc-website-draft-contract",
  "build",
  8,
);
const websiteEditTools = collectToolNames(
  "run-website-edit-draft-contract",
  "rc-website-edit-draft-contract",
  "edit",
  6,
);
for (const tools of [websiteBuildTools, websiteEditTools]) {
  assert.ok(tools.includes("draft.snapshot_create"));
  assert.ok(tools.includes("run.complete"));
  assert.ok(!tools.includes("preview.publish"));
  assert.ok(!tools.includes("browser.open"));
  assert.ok(!tools.includes("browser.screenshot"));
}
assertDependentToolsAreSerialized(
  "run-website-serialized-build",
  "rc-website-serialized-build",
  "build",
  8,
);
assertDependentToolsAreSerialized(
  "run-website-serialized-edit",
  "rc-website-serialized-edit",
  "edit",
  6,
);

const docsBuildTools = collectToolNames(
  "run-docs-publish-contract",
  "rc-docs-publish-contract",
  "build",
  9,
);
const docsEditTools = collectToolNames(
  "run-docs-edit-publish-contract",
  "rc-docs-edit-publish-contract",
  "edit",
  7,
);
for (const tools of [docsBuildTools, docsEditTools]) {
  assert.ok(tools.includes("preview.publish"));
  assert.ok(!tools.includes("draft.snapshot_create"));
  assert.ok(!tools.includes("preview.report_candidate"));
}
assertDependentToolsAreSerialized(
  "run-docs-serialized-build",
  "rc-docs-serialized-build",
  "build",
  9,
);
assertDependentToolsAreSerialized(
  "run-docs-serialized-edit",
  "rc-docs-serialized-edit",
  "edit",
  7,
);

process.stdout.write("Fixture model gateway contract passed\n");
