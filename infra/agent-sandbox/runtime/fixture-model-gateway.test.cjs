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
const docsBuildScript = docsBuildTwo.toolCalls.find(
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
const websiteBuildScript = websiteBuildTwo.toolCalls.find(
  (call) => call.name === "fs.write" && call.input.path === "project/build.cjs",
)?.input.text;
assert.match(
  websiteBuildScript,
  /rmSync\('dist',\{recursive:true,force:true\}\)/,
  "the Website fixture must remove stale output inherited from a reused warm-pool workspace",
);
assert.match(websiteBuildScript, /<meta name=\\"viewport\\"/);
assert.match(websiteBuildScript, /html,body\{margin:0;max-width:100%;overflow-x:hidden\}/);

function collectToolNames(runId, projectId, phase, turns) {
  return Array.from({ length: turns }, (_, index) =>
    responseForBody(request(runId, projectId, phase, index + 1)),
  ).flatMap((response) => response.toolCalls?.map((call) => call.name) || []);
}

function assertDependentToolsAreSerialized(runId, projectId, phase, turns) {
  const dependentTools = new Set([
    "project.build",
    "preview.start",
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

for (const [surface, projectId] of [
  ["website", "rc-website-publish-contract"],
  ["docs", "rc-docs-publish-contract"],
]) {
  const buildTools = collectToolNames(
    `run-${surface}-publish-contract`,
    projectId,
    "build",
    9,
  );
  const editTools = collectToolNames(
    `run-${surface}-edit-publish-contract`,
    projectId,
    "edit",
    7,
  );
  assert.ok(buildTools.includes("preview.publish"));
  assert.ok(editTools.includes("preview.publish"));
  assert.ok(!buildTools.includes("preview.report_candidate"));
  assert.ok(!editTools.includes("preview.report_candidate"));
  assertDependentToolsAreSerialized(
    `run-${surface}-serialized-build`,
    `${projectId}-serialized`,
    "build",
    9,
  );
  assertDependentToolsAreSerialized(
    `run-${surface}-serialized-edit`,
    `${projectId}-serialized`,
    "edit",
    7,
  );
}

process.stdout.write("Fixture model gateway contract passed\n");
