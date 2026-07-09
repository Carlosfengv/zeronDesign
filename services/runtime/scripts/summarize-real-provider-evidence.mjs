#!/usr/bin/env node
import fs from "node:fs";

function parseArgs(argv) {
  const args = {
    requiredProjects: ["real-http-website", "real-http-docs"],
    requiredStages: ["build", "edit"],
    providerOptional: false,
    requireComputedStyle: false,
    computedStyleProject: "real-http-website",
    computedStyleStage: "edit",
  };
  const requireValue = (arg, value) => {
    if (typeof value !== "string" || value.trim() === "" || value.startsWith("--")) {
      throw new Error(`${arg} requires a non-empty value`);
    }
    return value;
  };
  let projectFilterProvided = false;
  let stageFilterProvided = false;
  for (let index = 2; index < argv.length; index += 1) {
    const arg = argv[index];
    const next = argv[index + 1];
    if (arg === "--log") {
      args.log = requireValue(arg, next);
      index += 1;
    } else if (arg === "--out") {
      args.out = requireValue(arg, next);
      index += 1;
    } else if (arg === "--computed-style-log") {
      args.computedStyleLog = requireValue(arg, next);
      index += 1;
    } else if (arg === "--provider-optional") {
      args.providerOptional = true;
    } else if (arg === "--require-computed-style") {
      args.requireComputedStyle = true;
    } else if (arg === "--computed-style-project") {
      args.computedStyleProject = requireValue(arg, next);
      index += 1;
    } else if (arg === "--computed-style-stage") {
      args.computedStyleStage = requireValue(arg, next);
      index += 1;
    } else if (arg === "--project") {
      if (!projectFilterProvided) {
        args.requiredProjects = [];
        projectFilterProvided = true;
      }
      args.requiredProjects.push(requireValue(arg, next));
      index += 1;
    } else if (arg === "--stage") {
      if (!stageFilterProvided) {
        args.requiredStages = [];
        stageFilterProvided = true;
      }
      args.requiredStages.push(requireValue(arg, next));
      index += 1;
    } else {
      throw new Error(`Unknown argument: ${arg}`);
    }
  }
  if (!args.log && !args.computedStyleLog) {
    throw new Error("Missing --log or --computed-style-log");
  }
  if (!args.log && !args.providerOptional) {
    throw new Error("Missing --log");
  }
  args.requiredProjects = [...new Set(args.requiredProjects)];
  args.requiredStages = [...new Set(args.requiredStages)];
  return args;
}

function parseComputedStyleLog(path) {
  if (!path) {
    return null;
  }
  const text = fs.readFileSync(path, "utf8");
  const lines = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  const parsed = [];
  const errors = [];
  for (const line of lines) {
    try {
      parsed.push(JSON.parse(line));
    } catch {
      errors.push(`malformed computed-style log line: ${line.slice(0, 120)}`);
    }
  }
  const latest = parsed.at(-1) ?? null;
  if (!latest) {
    errors.push("computed-style log has no JSON result");
  } else if (parsed.length !== 1) {
    errors.push(`computed-style log must contain exactly one JSON result, got ${parsed.length}`);
  } else if (latest.ok !== true) {
    errors.push("computed-style verification did not report ok=true");
  } else {
    for (const field of ["url", "selector", "property", "expected", "actual"]) {
      if (typeof latest[field] !== "string" || latest[field].trim() === "") {
        errors.push(`computed-style result missing ${field}`);
      }
    }
    if (hasText(latest.url) && !isValidUrl(latest.url)) {
      errors.push("computed-style result url invalid");
    }
  }
  return {
    log: path,
    ok: errors.length === 0,
    errors,
    result: latest,
    resultCount: parsed.length,
  };
}

function stageKey(project, stage) {
  return `${project}:${stage}`;
}

function hasText(value) {
  return typeof value === "string" && value.trim() !== "";
}

function isValidUrl(value) {
  if (!hasText(value)) {
    return false;
  }
  try {
    new URL(value.trim());
    return true;
  } catch {
    return false;
  }
}

function editArtifactTextAssertionPassed(evidence) {
  const fields = ["artifactContainsExpectedText", "artifactContainsEditMarker"];
  const present = fields.filter((field) => field in evidence);
  if (present.length === 0) {
    return false;
  }
  return present.every((field) => evidence[field] === true);
}

function stageRecord(summary, project, stage) {
  const key = stageKey(project, stage);
  if (!summary.stages[key]) {
    summary.stages[key] = {
      project,
      stage,
      streamBegin: false,
      streamEnd: false,
      streamBeginCount: 0,
      streamEndCount: 0,
      eventCount: 0,
      malformedEventLines: 0,
      events: [],
      evidenceCount: 0,
      evidence: [],
    };
  }
  return summary.stages[key];
}

function parseKeyValueLine(line) {
  const values = {};
  for (const part of line.split(/\s+/)) {
    const separator = part.indexOf("=");
    if (separator === -1) {
      continue;
    }
    values[part.slice(0, separator)] = part.slice(separator + 1);
  }
  return values;
}

function parseLog(text) {
  const summary = {
    stages: {},
    malformedStreamLines: 0,
    malformedEvidenceLines: 0,
    unscopedEventLines: 0,
  };
  let currentStage = null;

  for (const line of text.split(/\r?\n/)) {
    if (line.startsWith("REAL_PROVIDER_STREAM_BEGIN ")) {
      const values = parseKeyValueLine(line.slice("REAL_PROVIDER_STREAM_BEGIN ".length));
      if (!values.project || !values.stage || !values.run) {
        summary.malformedStreamLines += 1;
        continue;
      }
      currentStage = stageRecord(summary, values.project, values.stage);
      currentStage.streamBegin = true;
      currentStage.streamBeginCount += 1;
      currentStage.runId = values.run;
      continue;
    }

    if (line.startsWith("REAL_PROVIDER_EVENT ")) {
      if (currentStage) {
        currentStage.eventCount += 1;
        try {
          const event = JSON.parse(line.slice("REAL_PROVIDER_EVENT ".length));
          if (
            !event ||
            typeof event !== "object" ||
            Array.isArray(event) ||
            typeof event.type !== "string" ||
            event.type.trim() === "" ||
            typeof event.runId !== "string" ||
            event.runId.trim() === ""
          ) {
            currentStage.malformedEventLines += 1;
            continue;
          }
          currentStage.events.push(event);
        } catch {
          currentStage.malformedEventLines += 1;
        }
      } else {
        summary.unscopedEventLines += 1;
      }
      continue;
    }

    if (line.startsWith("REAL_PROVIDER_EVIDENCE ")) {
      try {
        const evidence = JSON.parse(line.slice("REAL_PROVIDER_EVIDENCE ".length));
        if (
          typeof evidence.project !== "string" ||
          evidence.project.trim() === "" ||
          typeof evidence.stage !== "string" ||
          evidence.stage.trim() === "" ||
          typeof evidence.runId !== "string" ||
          evidence.runId.trim() === "" ||
          !evidence.evidence ||
          typeof evidence.evidence !== "object" ||
          Array.isArray(evidence.evidence)
        ) {
          summary.malformedEvidenceLines += 1;
          continue;
        }
        const record = stageRecord(summary, evidence.project, evidence.stage);
        record.evidenceCount += 1;
        record.evidence.push(evidence);
      } catch {
        summary.malformedEvidenceLines += 1;
      }
      continue;
    }

    if (line.startsWith("REAL_PROVIDER_STREAM_END ")) {
      const values = parseKeyValueLine(line.slice("REAL_PROVIDER_STREAM_END ".length));
      if (!values.project || !values.stage || !values.run) {
        summary.malformedStreamLines += 1;
        continue;
      }
      const record = stageRecord(summary, values.project, values.stage);
      record.streamEnd = true;
      record.streamEndCount += 1;
      record.streamEndRunId = values.run;
      if (currentStage?.project === values.project && currentStage?.stage === values.stage) {
        currentStage = null;
      }
    }
  }

  return summary;
}

function validateStage(record) {
  const errors = [];
  if (!record.streamBegin) {
    errors.push("missing stream begin");
  }
  if (!record.streamEnd) {
    errors.push("missing stream end");
  }
  if (record.streamBeginCount > 1) {
    errors.push(`duplicate stream begin lines: ${record.streamBeginCount}`);
  }
  if (record.streamEndCount > 1) {
    errors.push(`duplicate stream end lines: ${record.streamEndCount}`);
  }
  if (record.eventCount === 0) {
    errors.push("missing stream events");
  }
  if (record.malformedEventLines > 0) {
    errors.push(`malformed stream event lines: ${record.malformedEventLines}`);
  }
  if (record.evidenceCount === 0) {
    errors.push("missing evidence");
  }
  if (record.evidenceCount > 1) {
    errors.push(`duplicate evidence lines: ${record.evidenceCount}`);
  }
  if (!record.runId) {
    errors.push("missing stream run id");
  }
  if (record.streamEnd && !record.streamEndRunId) {
    errors.push("missing stream end run id");
  }
  if (
    record.runId &&
    record.streamEndRunId &&
    record.streamEndRunId !== record.runId
  ) {
    errors.push("stream end runId does not match stream begin runId");
  }

  const eventTypes = record.events.map((event) => event.type);
  const candidateEvents = record.events.filter((event) => event.type === "preview.candidate");
  const updatedEvents = record.events.filter((event) => event.type === "preview.updated");
  const completedEvents = record.events.filter((event) => event.type === "run.completed");
  const toolFailures = record.events.filter((event) => event.type === "tool.failed");
  const recoverableToolFailures = record.events.filter(
    (event) => event.type === "tool.failed" && event.recoverable === true,
  );
  const recoverySuggestions = record.events.filter(
    (event) => event.type === "tool.recovery_suggested",
  );
  const candidateIndex = eventTypes.indexOf("preview.candidate");
  const completedIndex = eventTypes.indexOf("run.completed");
  const updatedIndex = eventTypes.indexOf("preview.updated");
  if (candidateEvents.length === 0) {
    errors.push("missing preview.candidate event");
  }
  if (candidateEvents.length > 1) {
    errors.push(`duplicate preview.candidate events: ${candidateEvents.length}`);
  }
  if (updatedEvents.length === 0) {
    errors.push("missing preview.updated event");
  }
  if (updatedEvents.length > 1) {
    errors.push(`duplicate preview.updated events: ${updatedEvents.length}`);
  }
  if (completedIndex === -1) {
    errors.push("missing run.completed event");
  }
  if (completedEvents.length > 1) {
    errors.push(`duplicate run.completed events: ${completedEvents.length}`);
  }
  if (updatedIndex !== -1 && completedIndex !== -1 && updatedIndex > completedIndex) {
    errors.push("preview.updated appears after run.completed");
  }
  if (candidateIndex !== -1 && updatedIndex !== -1 && candidateIndex > updatedIndex) {
    errors.push("preview.candidate appears after preview.updated");
  }
  for (const event of record.events) {
    if (record.runId && event.runId && event.runId !== record.runId) {
      errors.push(`event ${event.type} runId does not match stream runId`);
    }
  }
  for (const event of candidateEvents) {
    if (!hasText(event.screenshotId)) {
      errors.push("preview.candidate missing screenshotId");
    }
  }
  for (const event of updatedEvents) {
    if (!hasText(event.screenshotId)) {
      errors.push("preview.updated missing screenshotId");
    }
  }
  for (const event of completedEvents) {
    if (event.status !== "completed") {
      errors.push(`run.completed status is not completed: ${event.status}`);
    }
  }
  for (const event of recoverableToolFailures) {
    if (!hasText(event.metadata?.errorKind)) {
      errors.push(`recoverable tool.failed missing metadata.errorKind for ${event.tool}`);
    }
  }
  for (const event of toolFailures) {
    if (event.metadata?.errorKind === "build.missing_dependency") {
      errors.push("provider run encountered build.missing_dependency");
    }
  }
  for (const event of recoverySuggestions) {
    if (!hasText(event.errorKind)) {
      errors.push(`tool.recovery_suggested missing errorKind for ${event.tool}`);
    }
  }
  for (const entry of record.evidence) {
    if (record.runId && entry.runId && entry.runId !== record.runId) {
      errors.push("evidence runId does not match stream runId");
    }
  }

  const evidence = record.evidence.at(-1)?.evidence;
  if (!evidence) {
    return errors;
  }

  if (evidence.previewUpdatedBeforeCompleted !== true) {
    errors.push("preview.updated ordering not proven");
  }
  if (evidence.currentPreview?.status !== "promoted") {
    errors.push("current preview is not promoted");
  }
  if (!hasText(evidence.currentPreview?.versionId)) {
    errors.push("current preview versionId missing");
  }
  if (!hasText(evidence.currentPreview?.previewUrl)) {
    errors.push("current preview previewUrl missing");
  } else if (!isValidUrl(evidence.currentPreview.previewUrl)) {
    errors.push("current preview previewUrl invalid");
  }
  if (
    evidence.currentPreview?.versionId &&
    evidence.runtimeState?.currentVersionId &&
    evidence.currentPreview.versionId !== evidence.runtimeState.currentVersionId
  ) {
    errors.push("current preview versionId does not match runtime-state currentVersionId");
  }
  for (const event of updatedEvents) {
    if (!hasText(event.versionId)) {
      errors.push("preview.updated missing versionId");
    }
    if (!hasText(event.url)) {
      errors.push("preview.updated missing url");
    } else if (!isValidUrl(event.url)) {
      errors.push("preview.updated url invalid");
    }
    if (
      hasText(event.versionId) &&
      evidence.runtimeState?.currentVersionId &&
      event.versionId !== evidence.runtimeState.currentVersionId
    ) {
      errors.push("preview.updated versionId does not match runtime-state currentVersionId");
    }
    if (
      hasText(event.url) &&
      evidence.currentPreview?.previewUrl &&
      event.url !== evidence.currentPreview.previewUrl
    ) {
      errors.push("preview.updated url does not match current preview previewUrl");
    }
  }
  for (const event of candidateEvents) {
    if (!hasText(event.versionId)) {
      errors.push("preview.candidate missing versionId");
    }
    if (
      hasText(event.versionId) &&
      evidence.runtimeState?.currentVersionId &&
      event.versionId !== evidence.runtimeState.currentVersionId
    ) {
      errors.push("preview.candidate versionId does not match runtime-state currentVersionId");
    }
    if (event.url !== undefined && !hasText(event.url)) {
      errors.push("preview.candidate url empty");
    } else if (event.url !== undefined && !isValidUrl(event.url)) {
      errors.push("preview.candidate url invalid");
    }
    if (
      hasText(event.url) &&
      evidence.currentPreview?.previewUrl &&
      event.url !== evidence.currentPreview.previewUrl
    ) {
      errors.push("preview.candidate url does not match current preview previewUrl");
    }
  }
  if (!hasText(evidence.runtimeState?.currentVersionId)) {
    errors.push("runtime-state currentVersionId missing");
  }
  if (!hasText(evidence.runtimeState?.sourceSnapshotUri)) {
    errors.push("runtime-state sourceSnapshotUri missing");
  } else if (!isValidUrl(evidence.runtimeState.sourceSnapshotUri)) {
    errors.push("runtime-state sourceSnapshotUri invalid");
  }
  if (!hasText(evidence.sourceSnapshotUri)) {
    errors.push("evidence sourceSnapshotUri missing");
  } else if (!isValidUrl(evidence.sourceSnapshotUri)) {
    errors.push("evidence sourceSnapshotUri invalid");
  }
  if (
    evidence.sourceSnapshotUri &&
    evidence.runtimeState?.sourceSnapshotUri &&
    evidence.sourceSnapshotUri !== evidence.runtimeState.sourceSnapshotUri
  ) {
    errors.push("evidence sourceSnapshotUri does not match runtime-state sourceSnapshotUri");
  }
  if (!hasText(evidence.runtimeState?.sandboxBindingId)) {
    errors.push("runtime-state sandboxBindingId missing");
  }
  if (!hasText(evidence.runtimeState?.appRoot)) {
    errors.push("runtime-state appRoot missing");
  }
  if (!hasText(evidence.runtimeState?.templateKey)) {
    errors.push("runtime-state templateKey missing");
  }
  if (!hasText(evidence.runtimeState?.styleContractPath)) {
    errors.push("runtime-state styleContractPath missing");
  }
  if (!hasText(evidence.runtimeState?.styleContract?.tokenFile)) {
    errors.push("runtime-state styleContract tokenFile missing");
  }
  if (!hasText(evidence.runtimeState?.styleContract?.globalCssFile)) {
    errors.push("runtime-state styleContract globalCssFile missing");
  }
  if (!hasText(evidence.runtimeState?.styleContract?.componentRoot)) {
    errors.push("runtime-state styleContract componentRoot missing");
  }
  if (!hasText(evidence.runtimeState?.styleContract?.tailwind?.version)) {
    errors.push("runtime-state styleContract tailwind version missing");
  }
  if (!hasText(evidence.runtimeState?.styleContract?.tailwind?.entryImport)) {
    errors.push("runtime-state styleContract tailwind entryImport missing");
  }
  if (!hasText(evidence.runtimeState?.styleContract?.tailwind?.themeSource)) {
    errors.push("runtime-state styleContract tailwind themeSource missing");
  }
  if (
    !evidence.runtimeState?.styleContract?.tokens ||
    Object.keys(evidence.runtimeState.styleContract.tokens).length === 0
  ) {
    errors.push("runtime-state styleContract tokens missing");
  } else {
    for (const [token, variable] of Object.entries(evidence.runtimeState.styleContract.tokens)) {
      if (!hasText(token) || !hasText(variable)) {
        errors.push("runtime-state styleContract token mapping invalid");
        break;
      }
    }
  }
  if (evidence.runtimeState?.latestBuild?.status !== "success") {
    errors.push("runtime-state latestBuild success missing");
  }
  if (!hasText(evidence.runtimeState?.latestBuild?.sourceSnapshotUri)) {
    errors.push("runtime-state latestBuild sourceSnapshotUri missing");
  } else if (!isValidUrl(evidence.runtimeState.latestBuild.sourceSnapshotUri)) {
    errors.push("runtime-state latestBuild sourceSnapshotUri invalid");
  } else if (
    evidence.runtimeState?.sourceSnapshotUri &&
    evidence.runtimeState.latestBuild.sourceSnapshotUri !== evidence.runtimeState.sourceSnapshotUri
  ) {
    errors.push("runtime-state latestBuild sourceSnapshotUri does not match sourceSnapshotUri");
  }
  if (!evidence.runtimeState?.dependencyState) {
    errors.push("runtime-state dependencyState missing");
  } else if (evidence.runtimeState.dependencyState.needsRestore !== false) {
    errors.push("runtime-state dependencyState needsRestore is not false");
  }
  if (!evidence.runtimeState?.preview?.status) {
    errors.push("runtime-state preview status missing");
  } else if (!["running", "stopped"].includes(evidence.runtimeState.preview.status)) {
    errors.push(
      `runtime-state preview status is not valid: ${evidence.runtimeState.preview.status}`,
    );
  }
  if (!hasText(evidence.artifactPath)) {
    errors.push("artifact path missing");
  } else {
    const normalizedArtifactPath = normalizeUrlPath(evidence.artifactPath.trim());
    const expectedProjectArtifactPrefix = `/artifacts/${record.project}/`;
    if (!normalizedArtifactPath.startsWith(expectedProjectArtifactPrefix)) {
      errors.push("artifact path does not belong to project");
    }
  }
  const localArtifactUrl =
    typeof evidence.localArtifactUrl === "string" ? evidence.localArtifactUrl.trim() : "";
  const artifactUrl =
    typeof evidence.artifactUrl === "string" ? evidence.artifactUrl.trim() : "";
  if (!localArtifactUrl && !artifactUrl) {
    errors.push("artifact verification URL missing");
  }
  if (localArtifactUrl && !isValidUrl(localArtifactUrl)) {
    errors.push("localArtifactUrl invalid");
  }
  if (artifactUrl && !isValidUrl(artifactUrl)) {
    errors.push("artifactUrl invalid");
  }
  if (evidence.artifactServed !== true) {
    errors.push("artifact was not served");
  }
  if (
    !Number.isFinite(evidence.artifactByteLength) ||
    evidence.artifactByteLength <= 0
  ) {
    errors.push("artifact byte length missing or empty");
  }

  if (record.stage === "edit") {
    if (evidence.sourceSnapshotChanged !== true) {
      errors.push("edit source snapshot did not change");
    }
    if (!hasText(evidence.initialVersionId)) {
      errors.push("edit initialVersionId missing");
    }
    if (!hasText(evidence.editedVersionId)) {
      errors.push("edit editedVersionId missing");
    }
    if (!hasText(evidence.initialSourceSnapshotUri)) {
      errors.push("edit initialSourceSnapshotUri missing");
    } else if (!isValidUrl(evidence.initialSourceSnapshotUri)) {
      errors.push("edit initialSourceSnapshotUri invalid");
    }
    if (!hasText(evidence.editedSourceSnapshotUri)) {
      errors.push("edit editedSourceSnapshotUri missing");
    } else if (!isValidUrl(evidence.editedSourceSnapshotUri)) {
      errors.push("edit editedSourceSnapshotUri invalid");
    }
    if (
      hasText(evidence.initialSourceSnapshotUri) &&
      hasText(evidence.editedSourceSnapshotUri) &&
      evidence.initialSourceSnapshotUri === evidence.editedSourceSnapshotUri
    ) {
      errors.push("edit source snapshot URI did not change");
    }
    if (
      evidence.editedSourceSnapshotUri &&
      evidence.runtimeState?.sourceSnapshotUri &&
      evidence.editedSourceSnapshotUri !== evidence.runtimeState.sourceSnapshotUri
    ) {
      errors.push("edit editedSourceSnapshotUri does not match runtime-state sourceSnapshotUri");
    }
    if (
      hasText(evidence.initialVersionId) &&
      hasText(evidence.editedVersionId) &&
      evidence.initialVersionId === evidence.editedVersionId
    ) {
      errors.push("edit version did not change");
    }
    if (
      hasText(evidence.editedVersionId) &&
      evidence.runtimeState?.currentVersionId &&
      evidence.editedVersionId !== evidence.runtimeState.currentVersionId
    ) {
      errors.push("edit editedVersionId does not match runtime-state currentVersionId");
    }
    if (!editArtifactTextAssertionPassed(evidence)) {
      errors.push("edited artifact text assertion did not pass");
    }
    if (!hasText(evidence.expectedArtifactText)) {
      errors.push("edit expectedArtifactText missing");
    }
  }

  return errors;
}

function validateSummary(summary, args) {
  const errors = [];
  if (!args.log && args.providerOptional) {
    return errors;
  }
  for (const project of args.requiredProjects) {
    for (const stage of args.requiredStages) {
      const key = stageKey(project, stage);
      const record = summary.stages[key];
      if (!record) {
        errors.push(`${key}: missing stage`);
        continue;
      }
      for (const error of validateStage(record)) {
        errors.push(`${key}: ${error}`);
      }
    }
  }
  if (summary.malformedEvidenceLines > 0) {
    errors.push(`malformed evidence lines: ${summary.malformedEvidenceLines}`);
  }
  if (summary.malformedStreamLines > 0) {
    errors.push(`malformed stream lines: ${summary.malformedStreamLines}`);
  }
  if (summary.unscopedEventLines > 0) {
    errors.push(`unscoped event lines: ${summary.unscopedEventLines}`);
  }
  return errors;
}

function artifactUrlsFromSummary(summary, project, stage) {
  const urls = new Set();
  const paths = new Set();
  const records =
    project && stage
      ? [summary.stages[stageKey(project, stage)]].filter(Boolean)
      : Object.values(summary.stages);
  for (const record of records) {
    for (const entry of record.evidence) {
      const evidence = entry.evidence ?? {};
      for (const field of ["localArtifactUrl", "artifactUrl"]) {
        if (typeof evidence[field] === "string" && evidence[field].trim()) {
          urls.add(evidence[field].trim());
        }
      }
      if (typeof evidence.artifactPath === "string" && evidence.artifactPath.trim()) {
        paths.add(normalizeUrlPath(evidence.artifactPath.trim()));
      }
    }
  }
  return { urls, paths };
}

function normalizeUrlPath(path) {
  if (!path) {
    return "";
  }
  const normalized = path.startsWith("/") ? path : `/${path}`;
  return normalized.length > 1 ? normalized.replace(/\/+$/, "") : normalized;
}

function urlPath(url) {
  try {
    return normalizeUrlPath(new URL(url).pathname);
  } catch {
    return "";
  }
}

function validateComputedStyleArtifactUrl(summary, computedStyle, args) {
  if (!args.log || !computedStyle?.result || computedStyle.result.ok !== true) {
    return [];
  }
  const url = computedStyle.result.url;
  if (typeof url !== "string" || !url.trim()) {
    return [];
  }
  const artifactEvidence = artifactUrlsFromSummary(
    summary,
    args.computedStyleProject,
    args.computedStyleStage,
  );
  if (artifactEvidence.urls.size === 0 && artifactEvidence.paths.size === 0) {
    return [
      `computed-style target ${args.computedStyleProject}:${args.computedStyleStage} has no artifact evidence`,
    ];
  }
  const computedUrl = url.trim();
  const computedPath = urlPath(computedUrl);
  if (
    artifactEvidence.urls.has(computedUrl) ||
    (computedPath && artifactEvidence.paths.has(computedPath))
  ) {
    return [];
  }
  return ["computed-style URL does not match provider artifact evidence"];
}

function main() {
  const args = parseArgs(process.argv);
  const text = args.log ? fs.readFileSync(args.log, "utf8") : "";
  const summary = parseLog(text);
  const computedStyle = parseComputedStyleLog(args.computedStyleLog);
  const errors = validateSummary(summary, args);
  if (args.requireComputedStyle && !computedStyle) {
    errors.push("computed-style evidence is required");
  }
  if (computedStyle?.errors.length) {
    errors.push(...computedStyle.errors);
  }
  errors.push(...validateComputedStyleArtifactUrl(summary, computedStyle, args));
  const result = {
    ok: errors.length === 0,
    log: args.log ?? null,
    computedStyleLog: args.computedStyleLog ?? null,
    requireComputedStyle: args.requireComputedStyle,
    requiredProjects: args.requiredProjects,
    requiredStages: args.requiredStages,
    computedStyleTarget: {
      project: args.computedStyleProject,
      stage: args.computedStyleStage,
    },
    errors,
    stages: Object.values(summary.stages),
    computedStyle,
    malformedStreamLines: summary.malformedStreamLines,
    malformedEvidenceLines: summary.malformedEvidenceLines,
    unscopedEventLines: summary.unscopedEventLines,
  };
  const json = `${JSON.stringify(result, null, 2)}\n`;
  if (args.out) {
    fs.writeFileSync(args.out, json);
  }
  process.stdout.write(json);
  if (!result.ok) {
    process.exit(1);
  }
}

main();
