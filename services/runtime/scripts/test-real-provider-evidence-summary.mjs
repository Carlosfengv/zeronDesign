#!/usr/bin/env node
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";

const repoRoot = path.resolve(path.dirname(new URL(import.meta.url).pathname), "../../..");
const summaryScript = path.join(
  repoRoot,
  "services/runtime/scripts/summarize-real-provider-evidence.mjs",
);
const extractArtifactScript = path.join(
  repoRoot,
  "services/runtime/scripts/extract-provider-artifact-url.mjs",
);
const verifyComputedStyleScript = path.join(
  repoRoot,
  "services/runtime/scripts/verify-computed-style.mjs",
);

function applyOverrides(target, overrides) {
  if (!overrides) {
    return target;
  }
  for (const [key, value] of Object.entries(overrides)) {
    if (value === undefined) {
      delete target[key];
    } else {
      target[key] = value;
    }
  }
  return target;
}

function runtimeState(versionId) {
  return {
    currentVersionId: versionId,
    sourceSnapshotUri: `runtime://snapshots/${versionId}`,
    sandboxBindingId: "binding-1",
    appRoot: "project",
    templateKey: "next-app",
    styleContractPath: "/workspace/state/style-contract.json",
    styleContract: {
      tokenFile: "project/src/styles/tokens.css",
      globalCssFile: "project/src/styles/global.css",
      componentRoot: "project/src/components/ui",
      tailwind: {
        version: "4",
        entryImport: '@import "tailwindcss"',
        themeSource: "css-variables",
      },
      tokens: {
        "color.primary": "--runtime-primary",
      },
    },
    latestBuild: {
      status: "success",
      sourceSnapshotUri: `runtime://snapshots/${versionId}`,
    },
    dependencyState: {
      needsRestore: false,
    },
    preview: {
      status: "running",
    },
  };
}

function websiteDcpEvidence(readFiles = ["inputs/design-profile.json"]) {
  return {
    contentHash: "a".repeat(64),
    artifactManifestHash: "b".repeat(64),
    briefHash: "c".repeat(64),
    verificationPolicyId: "website-verification@1",
    effectiveCompatibilityMode: "enforced",
    requiredReadPaths: ["inputs/design-profile.json"],
    readFiles,
  };
}

function websiteGenerationContextEvidence(stage) {
  const index = { build: 0, edit: 1, repair: 2 }[stage];
  return {
    templateVersion: "next-app@2",
    generationContext: {
      schemaVersion: "generation-context-status@1",
      runContractVersion: "generation-context@1",
      status: "compiled",
      contextContentHash: String(index + 1).repeat(64),
      runContextBindingHash: String(index + 4).repeat(64),
      runtimeAttestationHash: String(index + 7).repeat(64),
    },
    attestation: {
      state: "verified",
      runtimeAttestationHash: String(index + 7).repeat(64),
    },
    efficiency: {
      schemaVersion: "run-efficiency-metrics@1",
      uniqueContextReads: 0,
      uniqueSourceReads: 3,
      duplicateReads: 0,
      duplicateReadTokens: 0,
      unchangedReadStubs: 0,
      postCompactSourceRestores: 0,
      prebuildLists: 1,
    },
  };
}

function completeProviderLog(overrides = {}) {
  const projects = ["real-http-website", "real-http-docs"];
  const lines = [];
  for (const project of projects) {
    const stages =
      project === "real-http-website" ? ["build", "edit", "repair"] : ["build", "edit"];
    for (const stage of stages) {
      const versionId = `version-${project}-${stage}`;
      const runId = `run-${project}-${stage}`;
      const previewUrl = `http://preview.local/${project}/${stage}`;
      const eventOverrides = overrides.events?.[`${project}:${stage}`] ?? {};
      const evidenceOverrides = overrides.evidence?.[`${project}:${stage}`] ?? {};
      const evidenceWrapperOverrides = overrides.evidenceWrapper?.[`${project}:${stage}`] ?? {};
      const streamBeginOverrides = overrides.streamBegin?.[`${project}:${stage}`] ?? {};
      const streamEndOverrides = overrides.streamEnd?.[`${project}:${stage}`] ?? {};
      const streamBeginRunPart =
        streamBeginOverrides.runId === null
          ? ""
          : ` run=${streamBeginOverrides.runId ?? runId}`;
      const streamBeginLine =
        `REAL_PROVIDER_STREAM_BEGIN project=${project} stage=${stage}${streamBeginRunPart}`;
      lines.push(streamBeginLine);
      if (streamBeginOverrides.duplicate === true) {
        lines.push(streamBeginLine);
      }
      const eventPayloads = {
        candidate: {
          type: "preview.candidate",
          runId,
          url: previewUrl,
          versionId,
          screenshotId: `shot-${stage}`,
        },
        updated: {
          type: "preview.updated",
          runId,
          url: previewUrl,
          versionId,
          screenshotId: `shot-${stage}`,
        },
        completed: {
          type: "run.completed",
          runId,
          status: "completed",
          summary: "done",
        },
      };
      applyOverrides(eventPayloads.candidate, eventOverrides.candidate);
      applyOverrides(eventPayloads.updated, eventOverrides.updated);
      applyOverrides(eventPayloads.completed, eventOverrides.completed);
      const eventOrder = eventOverrides.order ?? ["candidate", "updated", "completed"];
      for (const eventName of eventOrder) {
        const rawEvent = eventOverrides.raw?.[eventName];
        lines.push(
          `REAL_PROVIDER_EVENT ${JSON.stringify(rawEvent ?? eventPayloads[eventName])}`,
        );
      }
      const streamEndLine =
        `REAL_PROVIDER_STREAM_END project=${project} stage=${stage} run=${streamEndOverrides.runId ?? runId}`;
      lines.push(streamEndLine);
      if (streamEndOverrides.duplicate === true) {
        lines.push(streamEndLine);
      }

      const evidence = {
        runtimeState: runtimeState(versionId),
        currentPreview: {
          status: "promoted",
          versionId,
          previewUrl,
        },
        sourceSnapshotUri: `runtime://snapshots/${versionId}`,
        artifactPath: `/artifacts/${project}/current`,
        localArtifactUrl: `file:///tmp/${project}/${stage}/index.html`,
        artifactServed: true,
        artifactByteLength: 4096,
        previewUpdatedBeforeCompleted: true,
      };
      if (stage === "edit") {
        evidence.initialVersionId = `version-${project}-build`;
        evidence.editedVersionId = versionId;
        evidence.initialSourceSnapshotUri = `runtime://snapshots/version-${project}-build`;
        evidence.editedSourceSnapshotUri = `runtime://snapshots/${versionId}`;
        evidence.sourceSnapshotChanged = true;
        evidence.artifactContainsExpectedText = true;
        evidence.artifactContainsEditMarker = true;
        evidence.expectedArtifactText = `expected text for ${project} ${stage}`;
      }
      if (stage === "repair") {
        evidence.baseVersionId = `version-${project}-edit`;
        evidence.repairedVersionId = versionId;
        evidence.baseSourceSnapshotUri = `runtime://snapshots/version-${project}-edit`;
        evidence.repairedSourceSnapshotUri = `runtime://snapshots/${versionId}`;
        evidence.sourceSnapshotChanged = true;
        evidence.artifactContainsExpectedText = true;
        evidence.expectedArtifactText = `expected text for ${project} ${stage}`;
        evidence.reviewRunId = `run-${project}-review`;
        evidence.findingId = `finding-${project}`;
        evidence.findingStatus = "fixed";
        evidence.candidateVersionId = `version-${project}-edit`;
        evidence.findingSource = "harness-seeded-review";
      }
      applyOverrides(evidence, evidenceOverrides);
      const evidenceLine =
        `REAL_PROVIDER_EVIDENCE ${JSON.stringify({
          project,
          stage,
          runId,
          provider: {
            name: "deepseek",
            model: "deepseek-chat",
            approvalReference: "approval-123",
          },
          evidence,
          ...evidenceWrapperOverrides,
        })}`;
      lines.push(evidenceLine);
      if (evidenceWrapperOverrides.duplicate === true) {
        lines.push(evidenceLine);
      }
    }
  }
  return `${lines.join("\n")}\n`;
}

function singleStageProviderLog(project, stage) {
  const lines = [];
  let keepStream = false;
  for (const line of completeProviderLog().trimEnd().split("\n")) {
    if (line.startsWith("REAL_PROVIDER_STREAM_BEGIN ")) {
      keepStream = line.includes(`project=${project} `) && line.includes(`stage=${stage} `);
    }
    if (keepStream) {
      lines.push(line);
    }
    if (line.startsWith("REAL_PROVIDER_STREAM_END ") && keepStream) {
      keepStream = false;
      continue;
    }
    if (line.startsWith("REAL_PROVIDER_EVIDENCE ")) {
      const wrapper = JSON.parse(line.slice("REAL_PROVIDER_EVIDENCE ".length));
      if (wrapper.project === project && wrapper.stage === stage) {
        lines.push(line);
      }
    }
  }
  return `${lines.join("\n")}\n`;
}

function withoutStageProviderLog(project, stage) {
  const lines = [];
  let skipStream = false;
  for (const line of completeProviderLog().trimEnd().split("\n")) {
    if (line.startsWith("REAL_PROVIDER_STREAM_BEGIN ")) {
      skipStream = line.includes(`project=${project} `) && line.includes(`stage=${stage} `);
      if (skipStream) continue;
    }
    if (skipStream) {
      if (line.startsWith("REAL_PROVIDER_STREAM_END ")) skipStream = false;
      continue;
    }
    if (line.startsWith("REAL_PROVIDER_EVIDENCE ")) {
      const wrapper = JSON.parse(line.slice("REAL_PROVIDER_EVIDENCE ".length));
      if (wrapper.project === project && wrapper.stage === stage) continue;
    }
    lines.push(line);
  }
  return `${lines.join("\n")}\n`;
}

function writeCase(dir, name, providerLog, computedStyle) {
  const caseDir = path.join(dir, name);
  fs.mkdirSync(caseDir, { recursive: true });
  const providerPath = path.join(caseDir, "provider-lifecycle.log");
  fs.writeFileSync(providerPath, providerLog);
  let computedStylePath = null;
  if (computedStyle) {
    computedStylePath = path.join(caseDir, "computed-style.log");
    const computedStyleResults = Array.isArray(computedStyle) ? computedStyle : [computedStyle];
    fs.writeFileSync(
      computedStylePath,
      `${computedStyleResults.map((result) => JSON.stringify(result)).join("\n")}\n`,
    );
  }
  return { caseDir, providerPath, computedStylePath };
}

function runSummary(paths, extraArgs = []) {
  const args = [summaryScript, "--log", paths.providerPath, ...extraArgs];
  if (paths.computedStylePath) {
    args.push("--computed-style-log", paths.computedStylePath);
  }
  const result = spawnSync(process.execPath, args, {
    encoding: "utf8",
  });
  const payload = JSON.parse(result.stdout);
  return { result, payload };
}

function assertPass(name, paths, extraArgs = []) {
  const { result, payload } = runSummary(paths, extraArgs);
  assert.equal(result.status, 0, `${name} should pass: ${result.stderr || result.stdout}`);
  assert.equal(payload.ok, true, `${name} summary ok`);
  return payload;
}

function assertFail(name, paths, expectedError, extraArgs = []) {
  const { result, payload } = runSummary(paths, extraArgs);
  assert.notEqual(result.status, 0, `${name} should fail`);
  assert.equal(payload.ok, false, `${name} summary not ok`);
  assert(
    payload.errors.some((error) => error.includes(expectedError)),
    `${name} should include ${JSON.stringify(expectedError)} in ${JSON.stringify(payload.errors)}`,
  );
  return payload;
}

function assertCliFail(name, paths, expectedError, extraArgs = []) {
  const args = [summaryScript, "--log", paths.providerPath, ...extraArgs];
  const result = spawnSync(process.execPath, args, {
    encoding: "utf8",
  });
  assert.notEqual(result.status, 0, `${name} should fail`);
  const output = `${result.stderr}\n${result.stdout}`;
  assert(
    output.includes(expectedError),
    `${name} should include ${JSON.stringify(expectedError)} in ${JSON.stringify(output)}`,
  );
}

function assertExtractorFail(name, args, expectedError) {
  const result = spawnSync(process.execPath, [extractArtifactScript, ...args], {
    encoding: "utf8",
  });
  assert.notEqual(result.status, 0, `${name} should fail`);
  const output = `${result.stderr}\n${result.stdout}`;
  assert(
    output.includes(expectedError),
    `${name} should include ${JSON.stringify(expectedError)} in ${JSON.stringify(output)}`,
  );
}

function verifierSpawnOptions() {
  const env = { ...process.env };
  const bundledNodeModules = path.join(
    os.homedir(),
    ".cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules",
  );
  if (!env.NODE_PATH && fs.existsSync(bundledNodeModules)) {
    env.NODE_PATH = bundledNodeModules;
  }
  return {
    encoding: "utf8",
    env,
  };
}

function assertVerifierCliFail(name, args, expectedError) {
  const result = spawnSync(
    process.execPath,
    [verifyComputedStyleScript, ...args],
    verifierSpawnOptions(),
  );
  assert.notEqual(result.status, 0, `${name} should fail`);
  const output = `${result.stderr}\n${result.stdout}`;
  assert(
    output.includes(expectedError),
    `${name} should include ${JSON.stringify(expectedError)} in ${JSON.stringify(output)}`,
  );
}

function assertVerifierPass(name, args) {
  const result = spawnSync(
    process.execPath,
    [verifyComputedStyleScript, ...args],
    verifierSpawnOptions(),
  );
  assert.equal(result.status, 0, `${name} should pass: ${result.stderr || result.stdout}`);
  const payload = JSON.parse(result.stdout);
  assert.equal(payload.ok, true, `${name} verifier ok`);
  return payload;
}

function main() {
  execFileSync(process.execPath, ["--check", summaryScript], { stdio: "inherit" });
  execFileSync(process.execPath, ["--check", extractArtifactScript], { stdio: "inherit" });
  execFileSync(process.execPath, ["--check", verifyComputedStyleScript], { stdio: "inherit" });
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "runtime-evidence-summary-test-"));

  const passing = writeCase(root, "passing", completeProviderLog(), {
    ok: true,
    url: "file:///tmp/real-http-website/edit/index.html",
    selector: ":root",
    property: "--runtime-primary",
    expected: "#f97316",
    actual: "#f97316",
  });
  const passingPayload = assertPass("passing provider evidence", passing, [
    "--require-computed-style",
  ]);
  assert.equal(passingPayload.stages.length, 5);
  assert.equal(passingPayload.computedStyle.ok, true);
  assert.equal(passingPayload.requireComputedStyle, true);
  assert.deepEqual(passingPayload.computedStyleTarget, {
    project: "real-http-website",
    stage: "edit",
  });
  const filteredStage = writeCase(
    root,
    "filtered-single-stage",
    singleStageProviderLog("real-http-website", "edit"),
  );
  const filteredPayload = assertPass("filtered single stage evidence", filteredStage, [
    "--project",
    "real-http-website",
    "--stage",
    "edit",
  ]);
  assert.deepEqual(filteredPayload.requiredProjects, ["real-http-website"]);
  assert.deepEqual(filteredPayload.requiredStages, ["edit"]);
  const dcpProviderEvidence = writeCase(
    root,
    "website-dcp-provider-evidence",
    completeProviderLog({
      evidence: {
        "real-http-website:build": { designContext: websiteDcpEvidence() },
        "real-http-website:edit": { designContext: websiteDcpEvidence() },
        "real-http-website:repair": { designContext: websiteDcpEvidence() },
      },
    }),
  );
  const dcpPayload = assertPass(
    "website DCP provider evidence",
    dcpProviderEvidence,
    ["--require-dcp-project", "real-http-website"],
  );
  assert.deepEqual(dcpPayload.requiredDcpProjects, ["real-http-website"]);
  const generationContextProviderEvidence = writeCase(
    root,
    "website-generation-context-provider-evidence",
    completeProviderLog({
      evidence: {
        "real-http-website:build": websiteGenerationContextEvidence("build"),
        "real-http-website:edit": websiteGenerationContextEvidence("edit"),
        "real-http-website:repair": websiteGenerationContextEvidence("repair"),
      },
    }),
  );
  assertPass(
    "website Generation Context provider evidence",
    generationContextProviderEvidence,
    ["--require-dcp-project", "real-http-website"],
  );
  const missingGenerationBinding = websiteGenerationContextEvidence("edit");
  delete missingGenerationBinding.generationContext.runContextBindingHash;
  assertFail(
    "missing Generation Context binding hash",
    writeCase(
      root,
      "missing-generation-context-binding",
      completeProviderLog({
        evidence: {
          "real-http-website:build": websiteGenerationContextEvidence("build"),
          "real-http-website:edit": missingGenerationBinding,
          "real-http-website:repair": websiteGenerationContextEvidence("repair"),
        },
      }),
    ),
    "runContextBindingHash must be sha256",
    ["--require-dcp-project", "real-http-website"],
  );
  const providerRepairPayload = assertPass(
    "enforced Website provider Repair evidence",
    dcpProviderEvidence,
    [
      "--require-dcp-project",
      "real-http-website",
      "--require-repair-project",
      "real-http-website",
      "--require-approval-reference",
      "approval-123",
    ],
  );
  assert.deepEqual(providerRepairPayload.requiredRepairProjects, ["real-http-website"]);
  assert.equal(providerRepairPayload.requiredApprovalReference, "approval-123");
  assertFail(
    "missing Website provider Repair stage",
    writeCase(
      root,
      "missing-provider-repair",
      withoutStageProviderLog("real-http-website", "repair"),
    ),
    "real-http-website:repair: missing stage",
    ["--require-repair-project", "real-http-website"],
  );
  assertFail(
    "unfixed Website provider Repair finding",
    writeCase(
      root,
      "unfixed-provider-repair",
      completeProviderLog({
        evidence: {
          "real-http-website:build": { designContext: websiteDcpEvidence() },
          "real-http-website:edit": { designContext: websiteDcpEvidence() },
          "real-http-website:repair": {
            designContext: websiteDcpEvidence(),
            findingStatus: "repairing",
          },
        },
      }),
    ),
    "repair findingStatus is not fixed",
    [
      "--require-dcp-project",
      "real-http-website",
      "--require-repair-project",
      "real-http-website",
    ],
  );
  assertFail(
    "provider approval reference mismatch",
    dcpProviderEvidence,
    "provider approvalReference does not match required approval",
    ["--require-approval-reference", "approval-other"],
  );
  assertFail(
    "missing Website DCP provider evidence",
    passing,
    "DCP evidence missing",
    ["--require-dcp-project", "real-http-website"],
  );
  assertFail(
    "missing required DCP Build read",
    writeCase(
      root,
      "missing-dcp-read",
      completeProviderLog({
        evidence: {
          "real-http-website:build": { designContext: websiteDcpEvidence([]) },
          "real-http-website:edit": { designContext: websiteDcpEvidence() },
          "real-http-website:repair": { designContext: websiteDcpEvidence() },
        },
      }),
    ),
    "DCP Build required reads were not all recorded",
    ["--require-dcp-project", "real-http-website"],
  );
  assertPass(
    "provider evidence with edit marker field only",
    writeCase(
      root,
      "edit-marker-field-only",
      completeProviderLog({
        evidence: {
          "real-http-website:edit": {
            artifactContainsExpectedText: undefined,
            artifactContainsEditMarker: true,
          },
          "real-http-docs:edit": {
            artifactContainsExpectedText: undefined,
            artifactContainsEditMarker: true,
          },
        },
      }),
    ),
  );
  assertFail(
    "single stage evidence without filters",
    filteredStage,
    "real-http-website:build: missing stage",
  );
  assertCliFail("empty project filter", passing, "--project requires a non-empty value", [
    "--project",
    "",
  ]);
  assertCliFail("empty stage filter", passing, "--stage requires a non-empty value", [
    "--stage",
    "",
  ]);
  assertCliFail("empty provider log path", passing, "--log requires a non-empty value", [
    "--log",
    "",
  ]);
  assertCliFail("missing provider log path", passing, "--log requires a non-empty value", [
    "--log",
    "--out",
  ]);
  assertCliFail("missing output path", passing, "--out requires a non-empty value", [
    "--out",
    "--stage",
  ]);
  assertCliFail(
    "missing computed-style log path",
    passing,
    "--computed-style-log requires a non-empty value",
    ["--computed-style-log", "--out"],
  );
  assertCliFail(
    "missing computed-style project value",
    passing,
    "--computed-style-project requires a non-empty value",
    ["--computed-style-project", "--stage", "edit"],
  );
  assertPass(
    "passing provider evidence with served artifact URL",
    writeCase(root, "passing-http-artifact-url", completeProviderLog(), {
      ok: true,
      url: "http://127.0.0.1:18082/artifacts/real-http-website/current",
      selector: ":root",
      property: "--runtime-primary",
      expected: "#f97316",
      actual: "#f97316",
    }),
    ["--require-computed-style"],
  );
  const customTargetPayload = assertPass(
    "passing provider evidence with custom computed-style target",
    writeCase(root, "passing-custom-style-target", completeProviderLog(), {
      ok: true,
      url: "file:///tmp/real-http-docs/edit/index.html",
      selector: ":root",
      property: "--runtime-primary",
      expected: "#f97316",
      actual: "#f97316",
    }),
    [
      "--require-computed-style",
      "--computed-style-project",
      "real-http-docs",
      "--computed-style-stage",
      "edit",
    ],
  );
  assert.deepEqual(customTargetPayload.computedStyleTarget, {
    project: "real-http-docs",
    stage: "edit",
  });
  const extractedArtifactUrl = execFileSync(
    process.execPath,
    [
      extractArtifactScript,
      "--log",
      passing.providerPath,
      "--project",
      "real-http-website",
      "--stage",
      "edit",
    ],
    { encoding: "utf8" },
  ).trim();
  assert.equal(extractedArtifactUrl, "file:///tmp/real-http-website/edit/index.html");
  const extractedDocsArtifactUrl = execFileSync(
    process.execPath,
    [
      extractArtifactScript,
      "--log",
      passing.providerPath,
      "--project",
      "real-http-docs",
      "--stage",
      "edit",
    ],
    { encoding: "utf8" },
  ).trim();
  assert.equal(extractedDocsArtifactUrl, "file:///tmp/real-http-docs/edit/index.html");
  assertExtractorFail("extract missing log path", ["--log", "--project"], "--log requires");
  assertExtractorFail(
    "extract empty project",
    ["--log", passing.providerPath, "--project", ""],
    "--project requires",
  );
  assertExtractorFail(
    "extract missing stage",
    ["--log", passing.providerPath, "--stage", "--project"],
    "--stage requires",
  );
  const invalidExtractArtifactUrl = writeCase(
    root,
    "extract-invalid-artifact-url",
    completeProviderLog({
      evidence: {
        "real-http-website:edit": {
          localArtifactUrl: "not a url",
          artifactUrl: "",
        },
      },
    }),
  );
  assertExtractorFail(
    "extract invalid artifact url",
    ["--log", invalidExtractArtifactUrl.providerPath],
    "Artifact URL is invalid",
  );
  assertVerifierCliFail("verifier missing url value", ["--url", "--expected"], "--url requires");
  assertVerifierCliFail(
    "verifier missing selector value",
    ["--url", "file:///tmp/index.html", "--selector", "--expected"],
    "--selector requires",
  );
  assertVerifierCliFail(
    "verifier missing expected value",
    ["--url", "file:///tmp/index.html", "--expected", ""],
    "--expected requires",
  );
  assertVerifierCliFail(
    "verifier invalid url",
    ["--url", "not a url", "--expected", "#f97316"],
    "--url must be a parseable URL",
  );
  assertVerifierCliFail(
    "verifier invalid timeout",
    ["--url", "file:///tmp/index.html", "--expected", "#f97316", "--timeout-ms", "10ms"],
    "--timeout-ms must be a positive integer",
  );
  const cssCustomPropertyFixture = path.join(root, "css-custom-property-fixture");
  fs.mkdirSync(cssCustomPropertyFixture);
  const cssCustomPropertyHtml = path.join(cssCustomPropertyFixture, "index.html");
  fs.writeFileSync(
    cssCustomPropertyHtml,
    "<style>:root { --runtime-primary: #f97316; }</style><main>custom property probe</main>",
  );
  const cssCustomPropertyPayload = assertVerifierPass("verifier CSS custom property value", [
    "--url",
    pathToFileURL(cssCustomPropertyHtml).href,
    "--selector",
    ":root",
    "--property",
    "--runtime-primary",
    "--expected",
    "#f97316",
  ]);
  assert.equal(cssCustomPropertyPayload.property, "--runtime-primary");

  assertFail(
    "computed style unrelated artifact",
    writeCase(root, "computed-style-unrelated-artifact", completeProviderLog(), {
      ok: true,
      url: "file:///tmp/unrelated-artifact/index.html",
      selector: ":root",
      property: "--runtime-primary",
      expected: "#f97316",
      actual: "#f97316",
    }),
    "computed-style URL does not match provider artifact evidence",
    ["--require-computed-style"],
  );

  assertFail(
    "computed style wrong provider stage",
    writeCase(root, "computed-style-wrong-stage", completeProviderLog(), {
      ok: true,
      url: "file:///tmp/real-http-website/build/index.html",
      selector: ":root",
      property: "--runtime-primary",
      expected: "#f97316",
      actual: "#f97316",
    }),
    "computed-style URL does not match provider artifact evidence",
    ["--require-computed-style"],
  );

  assertFail(
    "computed style unrelated served artifact",
    writeCase(root, "computed-style-unrelated-served-artifact", completeProviderLog(), {
      ok: true,
      url: "http://127.0.0.1:18082/artifacts/not-the-provider-project/current",
      selector: ":root",
      property: "--runtime-primary",
      expected: "#f97316",
      actual: "#f97316",
    }),
    "computed-style URL does not match provider artifact evidence",
    ["--require-computed-style"],
  );

  const missingStyleContract = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        runtimeState: {
          ...runtimeState("version-real-http-website-build"),
          styleContractPath: null,
          styleContract: null,
        },
      },
    },
  });
  assertFail(
    "missing style contract",
    writeCase(root, "missing-style-contract", missingStyleContract),
    "styleContractPath missing",
  );

  const invalidStyleContractTokenMapping = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        runtimeState: {
          ...runtimeState("version-real-http-website-build"),
          styleContract: {
            ...runtimeState("version-real-http-website-build").styleContract,
            tokens: {
              "color.primary": "   ",
            },
          },
        },
      },
    },
  });
  assertFail(
    "invalid style contract token mapping",
    writeCase(root, "invalid-style-contract-token-mapping", invalidStyleContractTokenMapping),
    "styleContract token mapping invalid",
  );

  const missingStyleContractTailwindMetadata = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        runtimeState: {
          ...runtimeState("version-real-http-website-build"),
          styleContract: {
            ...runtimeState("version-real-http-website-build").styleContract,
            tailwind: {
              version: "4",
              entryImport: "",
              themeSource: "css-variables",
            },
          },
        },
      },
    },
  });
  assertFail(
    "missing style contract tailwind metadata",
    writeCase(
      root,
      "missing-style-contract-tailwind-metadata",
      missingStyleContractTailwindMetadata,
    ),
    "styleContract tailwind entryImport missing",
  );

  const versionDrift = completeProviderLog({
    events: {
      "real-http-website:edit": {
        updated: {
          versionId: "wrong-version",
        },
      },
    },
  });
  assertFail(
    "version drift",
    writeCase(root, "version-drift", versionDrift),
    "preview.updated versionId does not match",
  );

  const missingUpdatedVersion = completeProviderLog({
    events: {
      "real-http-website:edit": {
        updated: {
          versionId: undefined,
        },
      },
    },
  });
  assertFail(
    "missing preview updated version",
    writeCase(root, "missing-updated-version", missingUpdatedVersion),
    "preview.updated missing versionId",
  );

  const missingUpdatedUrl = completeProviderLog({
    events: {
      "real-http-website:edit": {
        updated: {
          url: undefined,
        },
      },
    },
  });
  assertFail(
    "missing preview updated url",
    writeCase(root, "missing-updated-url", missingUpdatedUrl),
    "preview.updated missing url",
  );

  const missingCurrentPreviewVersion = completeProviderLog({
    evidence: {
      "real-http-website:edit": {
        currentPreview: {
          status: "promoted",
          previewUrl: "http://preview.local/real-http-website/edit",
        },
      },
    },
  });
  assertFail(
    "missing current preview version",
    writeCase(root, "missing-current-preview-version", missingCurrentPreviewVersion),
    "current preview versionId missing",
  );

  const blankCurrentPreviewUrl = completeProviderLog({
    evidence: {
      "real-http-website:edit": {
        currentPreview: {
          status: "promoted",
          versionId: "version-real-http-website-edit",
          previewUrl: "   ",
        },
      },
    },
  });
  assertFail(
    "blank current preview url",
    writeCase(root, "blank-current-preview-url", blankCurrentPreviewUrl),
    "current preview previewUrl missing",
  );

  const invalidCurrentPreviewUrl = completeProviderLog({
    evidence: {
      "real-http-website:edit": {
        currentPreview: {
          status: "promoted",
          versionId: "version-real-http-website-edit",
          previewUrl: "not a url",
        },
      },
    },
  });
  assertFail(
    "invalid current preview url",
    writeCase(root, "invalid-current-preview-url", invalidCurrentPreviewUrl),
    "current preview previewUrl invalid",
  );

  const candidateVersionDrift = completeProviderLog({
    events: {
      "real-http-website:edit": {
        candidate: {
          versionId: "wrong-candidate-version",
        },
      },
    },
  });
  assertFail(
    "candidate version drift",
    writeCase(root, "candidate-version-drift", candidateVersionDrift),
    "preview.candidate versionId does not match",
  );

  const previewUrlDrift = completeProviderLog({
    events: {
      "real-http-website:build": {
        updated: {
          url: "http://preview.local/wrong/current",
        },
      },
    },
  });
  assertFail(
    "preview url drift",
    writeCase(root, "preview-url-drift", previewUrlDrift),
    "preview.updated url does not match",
  );

  const invalidPreviewUpdatedUrl = completeProviderLog({
    events: {
      "real-http-website:build": {
        updated: {
          url: "not a url",
        },
      },
    },
  });
  assertFail(
    "invalid preview updated url",
    writeCase(root, "invalid-preview-updated-url", invalidPreviewUpdatedUrl),
    "preview.updated url invalid",
  );

  const invalidPreviewCandidateUrl = completeProviderLog({
    events: {
      "real-http-website:build": {
        candidate: {
          url: "not a url",
        },
      },
    },
  });
  assertFail(
    "invalid preview candidate url",
    writeCase(root, "invalid-preview-candidate-url", invalidPreviewCandidateUrl),
    "preview.candidate url invalid",
  );

  const candidateAfterUpdated = completeProviderLog({
    events: {
      "real-http-website:build": {
        order: ["updated", "candidate", "completed"],
      },
    },
  });
  assertFail(
    "candidate after updated",
    writeCase(root, "candidate-after-updated", candidateAfterUpdated),
    "preview.candidate appears after preview.updated",
  );

  const eventRunIdDrift = completeProviderLog({
    events: {
      "real-http-website:build": {
        updated: {
          runId: "run-wrong",
        },
      },
    },
  });
  assertFail(
    "event run id drift",
    writeCase(root, "event-run-id-drift", eventRunIdDrift),
    "event preview.updated runId does not match stream runId",
  );

  const evidenceRunIdDrift = completeProviderLog({
    evidenceWrapper: {
      "real-http-website:build": {
        runId: "run-wrong",
      },
    },
  });
  assertFail(
    "evidence run id drift",
    writeCase(root, "evidence-run-id-drift", evidenceRunIdDrift),
    "evidence runId does not match stream runId",
  );

  const streamEndRunIdDrift = completeProviderLog({
    streamEnd: {
      "real-http-website:build": {
        runId: "run-wrong",
      },
    },
  });
  assertFail(
    "stream end run id drift",
    writeCase(root, "stream-end-run-id-drift", streamEndRunIdDrift),
    "stream end runId does not match stream begin runId",
  );

  const malformedStreamBegin = completeProviderLog({
    streamBegin: {
      "real-http-website:build": {
        runId: null,
      },
    },
  });
  assertFail(
    "malformed stream begin",
    writeCase(root, "malformed-stream-begin", malformedStreamBegin),
    "malformed stream lines",
  );

  const duplicateStreamBegin = completeProviderLog({
    streamBegin: {
      "real-http-website:build": {
        duplicate: true,
      },
    },
  });
  assertFail(
    "duplicate stream begin",
    writeCase(root, "duplicate-stream-begin", duplicateStreamBegin),
    "duplicate stream begin lines",
  );

  const duplicateStreamEnd = completeProviderLog({
    streamEnd: {
      "real-http-website:build": {
        duplicate: true,
      },
    },
  });
  assertFail(
    "duplicate stream end",
    writeCase(root, "duplicate-stream-end", duplicateStreamEnd),
    "duplicate stream end lines",
  );

  const malformedEventWrapper = completeProviderLog({
    events: {
      "real-http-website:build": {
        raw: {
          updated: {
            type: "preview.updated",
            url: "http://preview.local/real-http-website/build",
          },
        },
      },
    },
  });
  assertFail(
    "malformed event wrapper",
    writeCase(root, "malformed-event-wrapper", malformedEventWrapper),
    "malformed stream event lines",
  );

  const duplicatePreviewUpdated = completeProviderLog({
    events: {
      "real-http-website:build": {
        order: ["candidate", "updated", "updated", "completed"],
      },
    },
  });
  assertFail(
    "duplicate preview updated",
    writeCase(root, "duplicate-preview-updated", duplicatePreviewUpdated),
    "duplicate preview.updated events",
  );

  const malformedEvidenceWrapper = completeProviderLog({
    evidenceWrapper: {
      "real-http-website:build": {
        project: undefined,
      },
    },
  });
  assertFail(
    "malformed evidence wrapper",
    writeCase(root, "malformed-evidence-wrapper", malformedEvidenceWrapper),
    "malformed evidence lines",
  );

  const duplicateEvidence = completeProviderLog({
    evidenceWrapper: {
      "real-http-website:build": {
        duplicate: true,
      },
    },
  });
  assertFail(
    "duplicate evidence",
    writeCase(root, "duplicate-evidence", duplicateEvidence),
    "duplicate evidence lines",
  );

  const sourceSnapshotDrift = completeProviderLog({
    evidence: {
      "real-http-website:edit": {
        editedSourceSnapshotUri: "runtime://snapshots/wrong-edit-snapshot",
      },
    },
  });
  assertFail(
    "source snapshot drift",
    writeCase(root, "source-snapshot-drift", sourceSnapshotDrift),
    "editedSourceSnapshotUri does not match",
  );

  const invalidRuntimeStateSourceSnapshot = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        runtimeState: {
          ...runtimeState("version-real-http-website-build"),
          sourceSnapshotUri: "not a uri",
        },
      },
    },
  });
  assertFail(
    "invalid runtime-state source snapshot",
    writeCase(root, "invalid-runtime-state-source-snapshot", invalidRuntimeStateSourceSnapshot),
    "runtime-state sourceSnapshotUri invalid",
  );

  const missingInitialVersion = completeProviderLog({
    evidence: {
      "real-http-website:edit": {
        initialVersionId: undefined,
      },
    },
  });
  assertFail(
    "missing initial version",
    writeCase(root, "missing-initial-version", missingInitialVersion),
    "edit initialVersionId missing",
  );

  const missingEditedVersion = completeProviderLog({
    evidence: {
      "real-http-website:edit": {
        editedVersionId: undefined,
      },
    },
  });
  assertFail(
    "missing edited version",
    writeCase(root, "missing-edited-version", missingEditedVersion),
    "edit editedVersionId missing",
  );

  const invalidInitialSourceSnapshot = completeProviderLog({
    evidence: {
      "real-http-website:edit": {
        initialSourceSnapshotUri: "not a uri",
      },
    },
  });
  assertFail(
    "invalid initial source snapshot",
    writeCase(root, "invalid-initial-source-snapshot", invalidInitialSourceSnapshot),
    "edit initialSourceSnapshotUri invalid",
  );

  const missingEvidenceSourceSnapshot = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        sourceSnapshotUri: undefined,
      },
    },
  });
  assertFail(
    "missing evidence source snapshot",
    writeCase(root, "missing-evidence-source-snapshot", missingEvidenceSourceSnapshot),
    "evidence sourceSnapshotUri missing",
  );

  const latestBuildSnapshotDrift = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        runtimeState: {
          ...runtimeState("version-real-http-website-build"),
          latestBuild: {
            status: "success",
            sourceSnapshotUri: "runtime://snapshots/wrong-build-snapshot",
          },
        },
      },
    },
  });
  assertFail(
    "latest build snapshot drift",
    writeCase(root, "latest-build-snapshot-drift", latestBuildSnapshotDrift),
    "latestBuild sourceSnapshotUri does not match",
  );

  const dependencyNeedsRestore = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        runtimeState: {
          ...runtimeState("version-real-http-website-build"),
          dependencyState: {
            needsRestore: true,
          },
        },
      },
    },
  });
  assertFail(
    "dependency still needs restore",
    writeCase(root, "dependency-needs-restore", dependencyNeedsRestore),
    "dependencyState needsRestore is not false",
  );

  const previewStopped = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        runtimeState: {
          ...runtimeState("version-real-http-website-build"),
          preview: {
            status: "stopped",
          },
        },
      },
    },
  });
  assertPass(
    "preview stopped after promotion",
    writeCase(root, "preview-stopped-after-promotion", previewStopped),
  );

  const previewInvalid = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        runtimeState: {
          ...runtimeState("version-real-http-website-build"),
          preview: {
            status: "failed",
          },
        },
      },
    },
  });
  assertFail(
    "preview invalid status",
    writeCase(root, "preview-invalid-status", previewInvalid),
    "preview status is not valid",
  );

  const artifactNotServed = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        artifactServed: false,
        artifactByteLength: 0,
      },
    },
  });
  assertFail(
    "artifact not served",
    writeCase(root, "artifact-not-served", artifactNotServed),
    "artifact was not served",
  );

  const artifactPathWrongProject = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        artifactPath: "/artifacts/real-http-docs/current",
      },
    },
  });
  assertFail(
    "artifact path wrong project",
    writeCase(root, "artifact-path-wrong-project", artifactPathWrongProject),
    "artifact path does not belong to project",
  );

  const missingArtifactVerificationUrl = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        localArtifactUrl: "",
        artifactUrl: "",
      },
    },
  });
  assertFail(
    "missing artifact verification url",
    writeCase(root, "missing-artifact-verification-url", missingArtifactVerificationUrl),
    "artifact verification URL missing",
  );

  const invalidLocalArtifactUrl = completeProviderLog({
    evidence: {
      "real-http-website:build": {
        localArtifactUrl: "not a url",
        artifactUrl: "",
      },
    },
  });
  assertFail(
    "invalid local artifact url",
    writeCase(root, "invalid-local-artifact-url", invalidLocalArtifactUrl),
    "localArtifactUrl invalid",
  );

  const missingScreenshot = completeProviderLog({
    events: {
      "real-http-website:build": {
        candidate: {
          screenshotId: null,
        },
      },
    },
  });
  assertFail(
    "missing screenshot",
    writeCase(root, "missing-screenshot", missingScreenshot),
    "preview.candidate missing screenshotId",
  );

  const blankScreenshot = completeProviderLog({
    events: {
      "real-http-website:build": {
        candidate: {
          screenshotId: "   ",
        },
      },
    },
  });
  assertFail(
    "blank screenshot",
    writeCase(root, "blank-screenshot", blankScreenshot),
    "preview.candidate missing screenshotId",
  );

  const missingErrorKind = completeProviderLog({
    events: {
      "real-http-website:build": {
        candidate: {
          type: "tool.failed",
          tool: "fs.patch",
          toolUseId: "tool-1",
          error: "recoverable without kind",
          recoverable: true,
          metadata: {
            recoverable: true,
          },
        },
      },
    },
  });
  assertFail(
    "recoverable missing errorKind",
    writeCase(root, "missing-error-kind", missingErrorKind),
    "recoverable tool.failed missing metadata.errorKind",
  );

  const blankErrorKind = completeProviderLog({
    events: {
      "real-http-website:build": {
        candidate: {
          type: "tool.failed",
          tool: "fs.patch",
          toolUseId: "tool-blank-kind",
          error: "recoverable with blank kind",
          recoverable: true,
          metadata: {
            recoverable: true,
            errorKind: "   ",
          },
        },
      },
    },
  });
  assertFail(
    "recoverable blank errorKind",
    writeCase(root, "blank-error-kind", blankErrorKind),
    "recoverable tool.failed missing metadata.errorKind",
  );

  const blankRecoverySuggestionKind = completeProviderLog({
    events: {
      "real-http-website:build": {
        candidate: {
          type: "tool.recovery_suggested",
          tool: "fs.patch",
          toolUseId: "tool-blank-suggestion-kind",
          errorKind: "   ",
        },
      },
    },
  });
  assertFail(
    "blank recovery suggestion errorKind",
    writeCase(root, "blank-recovery-suggestion-kind", blankRecoverySuggestionKind),
    "tool.recovery_suggested missing errorKind",
  );

  const missingDependencyFailure = completeProviderLog({
    events: {
      "real-http-website:build": {
        candidate: {
          type: "tool.failed",
          tool: "project.build",
          toolUseId: "tool-build",
          error: "next: command not found",
          recoverable: true,
          metadata: {
            recoverable: true,
            errorKind: "build.missing_dependency",
          },
        },
      },
    },
  });
  assertFail(
    "build missing dependency failure",
    writeCase(root, "missing-dependency", missingDependencyFailure),
    "provider run encountered build.missing_dependency",
  );

  const nonRecoverableMissingDependencyFailure = completeProviderLog({
    events: {
      "real-http-website:build": {
        candidate: {
          type: "tool.failed",
          tool: "project.build",
          toolUseId: "tool-build",
          error: "next: not found",
          recoverable: false,
          metadata: {
            recoverable: false,
            errorKind: "build.missing_dependency",
          },
        },
      },
    },
  });
  assertFail(
    "non-recoverable build missing dependency failure",
    writeCase(root, "non-recoverable-missing-dependency", nonRecoverableMissingDependencyFailure),
    "provider run encountered build.missing_dependency",
  );

  const failedArtifactEditMarker = completeProviderLog({
    evidence: {
      "real-http-website:edit": {
        artifactContainsEditMarker: false,
      },
    },
  });
  assertFail(
    "failed artifact edit marker",
    writeCase(root, "failed-artifact-edit-marker", failedArtifactEditMarker),
    "edited artifact text assertion did not pass",
  );

  const missingExpectedArtifactText = completeProviderLog({
    evidence: {
      "real-http-website:edit": {
        expectedArtifactText: undefined,
      },
    },
  });
  assertFail(
    "missing expected artifact text",
    writeCase(root, "missing-expected-artifact-text", missingExpectedArtifactText),
    "edit expectedArtifactText missing",
  );

  const blankExpectedArtifactText = completeProviderLog({
    evidence: {
      "real-http-website:edit": {
        expectedArtifactText: "   ",
      },
    },
  });
  assertFail(
    "blank expected artifact text",
    writeCase(root, "blank-expected-artifact-text", blankExpectedArtifactText),
    "edit expectedArtifactText missing",
  );

  const computedStyleMismatch = writeCase(root, "computed-style-mismatch", completeProviderLog(), {
    ok: false,
    url: "http://127.0.0.1/artifacts/real-http-website/current",
    selector: ":root",
    property: "--runtime-primary",
    expected: "#f97316",
    actual: "#000000",
  });
  assertFail(
    "computed style mismatch",
    computedStyleMismatch,
    "computed-style verification did not report ok=true",
    ["--require-computed-style"],
  );

  const incompleteComputedStyle = writeCase(root, "computed-style-incomplete", completeProviderLog(), {
    ok: true,
  });
  assertFail(
    "computed style incomplete",
    incompleteComputedStyle,
    "computed-style result missing url",
    ["--require-computed-style"],
  );

  const invalidComputedStyleUrl = writeCase(root, "computed-style-invalid-url", completeProviderLog(), {
    ok: true,
    url: "not a url",
    selector: ":root",
    property: "--runtime-primary",
    expected: "#f97316",
    actual: "#f97316",
  });
  assertFail(
    "computed style invalid url",
    invalidComputedStyleUrl,
    "computed-style result url invalid",
    ["--require-computed-style"],
  );

  const duplicateComputedStyle = writeCase(root, "computed-style-duplicate", completeProviderLog(), [
    {
      ok: true,
      url: "http://127.0.0.1/artifacts/real-http-website/current",
      selector: ":root",
      property: "--runtime-primary",
      expected: "#f97316",
      actual: "#f97316",
    },
    {
      ok: true,
      url: "http://127.0.0.1/artifacts/real-http-website/current",
      selector: ":root",
      property: "--runtime-primary",
      expected: "#f97316",
      actual: "#f97316",
    },
  ]);
  assertFail(
    "computed style duplicate",
    duplicateComputedStyle,
    "computed-style log must contain exactly one JSON result",
    ["--require-computed-style"],
  );

  assertFail(
    "missing required computed style",
    writeCase(root, "missing-required-computed-style", completeProviderLog()),
    "computed-style evidence is required",
    ["--require-computed-style"],
  );

  console.log(JSON.stringify({ ok: true, tempDir: root }, null, 2));
}

main();
