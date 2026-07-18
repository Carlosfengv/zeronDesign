const http = require("http");

const runs = new Map();
const responses = new Map();

function tool(id, name, input = {}) {
  return { id, name, input };
}

function staticWebsiteBuildScript(title, fontSize) {
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>${title}</title><style>*,*::before,*::after{box-sizing:border-box}html,body{margin:0;max-width:100%;overflow-x:hidden}body{font:16px sans-serif;background:#fff;color:#111}h1{font-size:${fontSize}px;overflow-wrap:anywhere}</style></head><body><main><h1>${title}</h1></main></body></html>`;
  return `const fs=require('fs');fs.rmSync('dist',{recursive:true,force:true});fs.mkdirSync('dist',{recursive:true});fs.writeFileSync('dist/index.html',${JSON.stringify(html)});`;
}

function projectId(body) {
  return String(
    body.projectId
      || body.scope?.projectId
      || systemPrompt(body).match(/^Project:\s*(.+)$/m)?.[1]
      || "",
  );
}

function runId(body) {
  return String(body.runId || body.scope?.runId || "");
}

function phase(body) {
  return String(body.phase || body.scope?.phase || "build");
}

function turn(body) {
  const value = Number(body.turn ?? body.scope?.turn);
  return Number.isSafeInteger(value) && value > 0 ? value : 1;
}

function systemPrompt(body) {
  return String(body.systemPrompt || body.input?.systemPrompt || "");
}

function isEnforcedDcpFixture(body) {
  return projectId(body).includes("dcp-enforced");
}

function runtimeIdentityValue(body, key) {
  const match = systemPrompt(body).match(new RegExp(`^${key}:\\s*(.+)$`, "m"));
  return match?.[1]?.trim() || "";
}

function briefResponse(body, state) {
  const docs = projectId(body).toLowerCase().includes("docs");
  if (state.turn++ === 0) {
    return {
      type: "tool_calls",
      toolCalls: [
        tool("fixture-content-list", "content.list_sources"),
        tool("fixture-content-read", "content.read_source", { id: "source-1" }),
      ],
    };
  }
  return {
    type: "tool_calls",
    toolCalls: [
      tool("fixture-brief", "brief.write_draft", {
        projectType: docs ? "docs" : "website",
        audience: docs ? "developer operators" : "product teams",
        contentHierarchy: docs ? ["overview", "lifecycle"] : ["hero", "proof"],
        pageStructure: docs
          ? [{ title: "Overview", level: 1, content: "Runtime lifecycle" }]
          : [{ title: "Home", purpose: "Explain the product", keyContent: ["hero", "proof"] }],
        visualDirection: "quiet technical confidence",
        recommendedTemplate: docs ? "fumadocs-docs" : "astro-website",
        assumptions: [],
        missingInformation: [],
        acceptanceCriteria: {
          locale: "en",
          requiredRoutes: [docs ? "/docs/" : "/"],
          requiredText: [],
          forbiddenText: ["Lorem ipsum"],
        },
      }),
      tool("fixture-confirm", "brief.request_confirmation", {
        message: "Confirm this deterministic RC brief.",
      }),
    ],
  };
}

function buildResponse(body, state) {
  if (isEnforcedDcpFixture(body)) return enforcedDcpBuildResponse(state);
  state.docs ||= projectId(body).toLowerCase().includes("docs");
  const turn = state.turn++;
  if (turn === 0) {
    return {
      type: "tool_calls",
      toolCalls: [
        tool("fixture-init", "project.init", {
          template: state.docs ? "fumadocs-docs" : "astro-website",
        }),
      ],
    };
  }
  const docs = state.docs;
  const buildScript = docs
    ? "const fs=require('fs');fs.rmSync('out',{recursive:true,force:true});fs.mkdirSync('out',{recursive:true});const head='<meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>RC Docs</title><style>*,*::before,*::after{box-sizing:border-box}body{margin:0;font:16px sans-serif;background:#fff;color:#111;max-width:100%;overflow-x:hidden}h1{font-size:40px;overflow-wrap:anywhere}</style>';const shell='<nav><a href=\"/docs/#overview\">Overview</a></nav><label>Search <input type=\"search\" aria-label=\"Search docs\"></label>';fs.writeFileSync('out/index.html','<!doctype html><html lang=\"en\"><head>'+head+'</head><body>'+shell+'<h1>RC Docs</h1></body></html>');fs.writeFileSync('out/docs.html','<!doctype html><html lang=\"en\"><head>'+head+'</head><body>'+shell+'<h1 id=\"overview\">RC Docs Overview</h1></body></html>');"
    : staticWebsiteBuildScript("RC Website", 48);
  const calls = [
    tool("fixture-package", "fs.write", {
      path: "project/package.json",
      text: '{"scripts":{"build":"node build.cjs"}}',
    }),
    tool("fixture-build-script", "fs.write", { path: "project/build.cjs", text: buildScript }),
  ];
  if (docs) {
    calls.push(tool("fixture-docs-source", "fs.write", {
      path: "project/content/docs/index.mdx",
      text: "---\ntitle: Overview\n---\n\n# RC Docs Overview",
    }));
  }
  if (turn === 1) {
    return { type: "tool_calls", toolCalls: calls };
  }
  if (turn === 2) {
    return {
      type: "tool_calls",
      toolCalls: [
        tool("fixture-dependency", "project.ensure_dependencies", {
          mode: "add",
          packages: ["is-number@7.0.0"],
          cwd: "project",
        }),
      ],
    };
  }
  if (turn === 3) return { type: "tool_calls", toolCalls: [tool("fixture-build", "project.build", { cwd: "project" })] };
  if (turn === 4) return { type: "tool_calls", toolCalls: [tool("fixture-preview", "preview.start")] };
  if (turn === 5) return { type: "tool_calls", toolCalls: [tool("fixture-open", "browser.open", { url: "http://127.0.0.1:4321" })] };
  if (turn === 6) return { type: "tool_calls", toolCalls: [tool("fixture-shot", "browser.screenshot", { screenshotId: docs ? "rc-docs" : "rc-website" })] };
  if (turn === 7) return { type: "tool_calls", toolCalls: [tool("fixture-promote", "preview.publish", { screenshotId: docs ? "rc-docs" : "rc-website" })] };
  return { type: "tool_calls", toolCalls: [tool("fixture-complete", "run.complete", {
    status: "completed",
    summary: `${docs ? "Docs" : "Website"} deployed Runtime RC gate complete`,
  })] };
}

function enforcedDcpBuildResponse(state) {
  const turn = state.turn++;
  if (turn === 0) {
    return {
      type: "tool_calls",
      toolCalls: [
        "inputs/brief.md",
        "inputs/design-profile.json",
        "inputs/design-profile-usage.md",
        "inputs/component-recipes.json",
        "inputs/template-style-contract.json",
      ].map((path, index) => tool(`fixture-dcp-build-read-${index}`, "fs.read", { path })),
    };
  }
  if (turn === 1) {
    return {
      type: "tool_calls",
      toolCalls: [tool("fixture-dcp-init", "project.init", { template: "astro-website" })],
    };
  }
  if (turn === 2) {
    const buildScript = staticWebsiteBuildScript("RC Enforced DCP Website", 48);
    return {
      type: "tool_calls",
      toolCalls: [
        tool("fixture-dcp-style-contract", "fs.read", { path: "state/style-contract.json" }),
        tool("fixture-dcp-package", "fs.write", {
          path: "project/package.json", text: '{"scripts":{"build":"node build.cjs"}}',
        }),
        tool("fixture-dcp-build-script", "fs.write", { path: "project/build.cjs", text: buildScript }),
      ],
    };
  }
  if (turn === 3) {
    return {
      type: "tool_calls",
      toolCalls: [
        tool("fixture-dcp-dependency", "project.ensure_dependencies", {
          mode: "add",
          packages: ["is-number@7.0.0"],
          cwd: "project",
        }),
      ],
    };
  }
  if (turn === 4) return { type: "tool_calls", toolCalls: [tool("fixture-dcp-build", "project.build", { cwd: "project" })] };
  if (turn === 5) return { type: "tool_calls", toolCalls: [tool("fixture-dcp-publish", "preview.publish", { screenshotId: "rc-enforced-dcp-build" })] };
  return { type: "tool_calls", toolCalls: [tool("fixture-dcp-complete", "run.complete", {
    status: "completed", summary: "Deployed Runtime enforced DCP fixture build complete",
  })] };
}

function editResponse(body, state) {
  if (isEnforcedDcpFixture(body)) return enforcedDcpEditResponse(state);
  state.docs ||= projectId(body).toLowerCase().includes("docs");
  const docs = state.docs;
  const turn = state.turn++;
  const buildScript = docs
    ? "const fs=require('fs');fs.rmSync('out',{recursive:true,force:true});fs.mkdirSync('out',{recursive:true});const head='<meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>RC Docs Edited</title><style>*,*::before,*::after{box-sizing:border-box}body{margin:0;font:16px sans-serif;background:#fff;color:#111;max-width:100%;overflow-x:hidden}h1{font-size:40px;overflow-wrap:anywhere}</style>';const shell='<nav><a href=\"/docs/#overview\">Overview</a></nav><label>Search <input type=\"search\" aria-label=\"Search docs\"></label>';fs.writeFileSync('out/index.html','<!doctype html><html lang=\"en\"><head>'+head+'</head><body>'+shell+'<h1>RC Docs Edited</h1></body></html>');fs.writeFileSync('out/docs.html','<!doctype html><html lang=\"en\"><head>'+head+'</head><body>'+shell+'<h1 id=\"overview\">RC Docs Overview Edited</h1></body></html>');"
    : staticWebsiteBuildScript("RC Website Edited", 48);
  if (turn === 0) return { type: "tool_calls", toolCalls: [tool("fixture-edit-script", "fs.write", { path: "project/build.cjs", text: buildScript })] };
  if (turn === 1) return { type: "tool_calls", toolCalls: [tool("fixture-edit-build", "project.build", { cwd: "project" })] };
  if (turn === 2) return { type: "tool_calls", toolCalls: [tool("fixture-edit-preview", "preview.start")] };
  if (turn === 3) return { type: "tool_calls", toolCalls: [tool("fixture-edit-open", "browser.open", { url: "http://127.0.0.1:4321" })] };
  if (turn === 4) return { type: "tool_calls", toolCalls: [tool("fixture-edit-shot", "browser.screenshot", { screenshotId: docs ? "rc-docs-edit" : "rc-website-edit" })] };
  if (turn === 5) return { type: "tool_calls", toolCalls: [tool("fixture-edit-promote", "preview.publish", { screenshotId: docs ? "rc-docs-edit" : "rc-website-edit" })] };
  return { type: "tool_calls", toolCalls: [tool("fixture-edit-complete", "run.complete", {
    status: "completed",
    summary: `${docs ? "Docs" : "Website"} deployed Runtime RC edit complete`,
  })] };
}

function enforcedDcpEditResponse(state) {
  const turn = state.turn++;
  if (turn === 0) {
    return {
      type: "tool_calls",
      toolCalls: [
        "inputs/design-profile.json",
        "inputs/design-profile-usage.md",
        "inputs/component-recipes.json",
      ].map((path, index) => tool(`fixture-dcp-edit-read-${index}`, "fs.read", { path })),
    };
  }
  const buildScript = staticWebsiteBuildScript("RC Enforced DCP Website Edited", 44);
  if (turn === 1) return { type: "tool_calls", toolCalls: [tool("fixture-dcp-edit-style-contract", "fs.read", { path: "state/style-contract.json" })] };
  if (turn === 2) return { type: "tool_calls", toolCalls: [tool("fixture-dcp-edit-script", "fs.write", { path: "project/build.cjs", text: buildScript })] };
  if (turn === 3) return { type: "tool_calls", toolCalls: [tool("fixture-dcp-edit-build", "project.build", { cwd: "project" })] };
  if (turn === 4) return { type: "tool_calls", toolCalls: [tool("fixture-dcp-edit-publish", "preview.publish", { screenshotId: "rc-enforced-dcp-edit" })] };
  return { type: "tool_calls", toolCalls: [tool("fixture-dcp-edit-complete", "run.complete", {
    status: "completed", summary: "Deployed Runtime enforced DCP fixture edit complete",
  })] };
}

function reviewResponse(body, state) {
  const candidateVersion = runtimeIdentityValue(body, "CandidateVersion");
  if (!candidateVersion) throw new Error("Review fixture requires CandidateVersion runtime identity");
  const providerDcp = projectId(body).includes("dcp-provider");
  const turn = state.turn++;
  if (turn === 0) return {
    type: "tool_calls",
    toolCalls: [tool("fixture-dcp-review-finding", "review.report_finding", {
        versionId: candidateVersion,
        severity: "blocking",
        category: "visual",
        summary: providerDcp
          ? "Replace the visible heading with the exact text RC Enforced DCP Provider Website Repaired, preserve the current design tokens, rebuild, verify the served artifact, and publish a new candidate"
          : "Repair the deployed enforced DCP lifecycle heading",
        repairable: true,
        evidence: { filePath: "project/build.cjs" },
      })],
  };
  return { type: "tool_calls", toolCalls: [tool("fixture-dcp-review-complete", "run.complete", {
    status: "completed", summary: "Deployed Runtime enforced DCP review complete",
  })] };
}

function repairResponse(body, state) {
  if (!isEnforcedDcpFixture(body)) throw new Error("Repair fixture is only defined for enforced DCP RC");
  const turn = state.turn++;
  if (turn === 0) {
    return {
      type: "tool_calls",
      toolCalls: [
        "inputs/design-profile-usage.md",
        "inputs/component-recipes.json",
        "state/style-contract.json",
      ].map((path, index) => tool(`fixture-dcp-repair-read-${index}`, "fs.read", { path })),
    };
  }
  const buildScript = staticWebsiteBuildScript("RC Enforced DCP Website Repaired", 42);
  if (turn === 1) return { type: "tool_calls", toolCalls: [tool("fixture-dcp-repair-script", "fs.write", { path: "project/build.cjs", text: buildScript })] };
  if (turn === 2) return { type: "tool_calls", toolCalls: [tool("fixture-dcp-repair-build", "project.build", { cwd: "project" })] };
  if (turn === 3) return { type: "tool_calls", toolCalls: [tool("fixture-dcp-repair-publish", "preview.publish", { screenshotId: "rc-enforced-dcp-repair" })] };
  return { type: "tool_calls", toolCalls: [tool("fixture-dcp-repair-complete", "run.complete", {
    status: "completed", summary: "Deployed Runtime enforced DCP fixture repair complete",
  })] };
}

function responseForBody(body) {
  const requestRunId = runId(body);
  if (!requestRunId) throw new Error("fixture request requires runId or scope.runId");
  const requestPhase = phase(body);
  const responseKey = `${requestRunId}:${turn(body)}`;
  const cached = responses.get(responseKey);
  if (cached) return cached;
  const state = runs.get(requestRunId) || { turn: 0, docs: false };
  const payload = requestPhase === "brief"
    ? briefResponse(body, state)
    : requestPhase === "review"
      ? reviewResponse(body, state)
      : requestPhase === "repair"
        ? repairResponse(body, state)
        : requestPhase === "edit"
          ? editResponse(body, state)
          : buildResponse(body, state);
  runs.set(requestRunId, state);
  responses.set(responseKey, payload);
  return payload;
}

function resetFixtureState() {
  runs.clear();
  responses.clear();
}

if (require.main === module) {
  http.createServer((request, response) => {
    if (request.method !== "POST" || request.url !== "/v1/agent/turn") {
      response.writeHead(request.url === "/health" ? 200 : 404);
      response.end(request.url === "/health" ? "ok" : "not found");
      return;
    }
    let raw = "";
    request.on("data", chunk => raw += chunk);
    request.on("end", () => {
      try {
        const payload = responseForBody(JSON.parse(raw));
        response.writeHead(200, { "content-type": "application/json" });
        response.end(JSON.stringify(payload));
      } catch (error) {
        response.writeHead(400, { "content-type": "application/json" });
        response.end(JSON.stringify({ error: String(error) }));
      }
    });
  }).listen(9000, "0.0.0.0");
}

module.exports = {
  phase,
  projectId,
  resetFixtureState,
  responseForBody,
  runId,
  systemPrompt,
  turn,
};
