---
date: 2026-07-08
status: implemented
type: implementation-plan
topic: project-lifecycle-generation-edit-build
excludes:
  - DesignProfile integration
related:
  - ./2026-07-04-mvp-implementation-plan.md
  - ./2026-07-04-agent-harness-design.md
  - ./2026-07-04-rust-runtime-spec.md
  - ./2026-07-08-runtime-api-freeze.md
  - ./2026-07-08-phase-a-acceptance-report.md
---

# Project Lifecycle: Creation -> Generation -> Edit -> Build

## 1. Goal

This phase completes the full artifact lifecycle for Website and Docs outputs:

```text
Create project
  -> ingest prompt / markdown / attachment content
  -> generate confirmed artifact source
  -> build and promote preview
  -> edit through chat prompt
  -> rebuild and promote the next preview version
```

The goal is not only to produce a static preview. The generated artifact must
become an editable runtime project with stable state, source files, build
evidence, version history, and a reusable sandbox binding.

## 2. Explicit Non-Goal: DesignProfile

DesignProfile extraction, tokenization, design-system inference, and automatic
style-profile application are out of scope for this phase.

This phase may still generate styled Website or Docs output with Tailwind CSS,
Base UI / shadcn-style primitives, and template-specific CSS. Those styles are
template implementation details. They must not depend on a DesignProfile data
model or a DesignProfile runtime contract yet.

## 3. Current Gap

The runtime can already generate and promote preview artifacts. The immediate
gap is lifecycle continuity:

- `/internal/template-build` can create a visible artifact quickly, but it is an
  internal helper and should not become the product path.
- Standard edit runs require a valid `sandboxBindingId`.
- A template-generated artifact that has no persisted editable sandbox binding
  cannot enter the normal edit flow.
- A prompt edit that reruns a template renderer is not the same as editing the
  existing project source and rebuilding it.

The product path must make the generated Website or Docs project editable from
the first promoted version.

## 4. Target Runtime Behavior

### 4.1 Project Creation

Creating an artifact must create a runtime project record or project-lifecycle
state with:

- `projectId`
- `kind`: `website | docs`
- `templateKey`: `astro-website | fumadocs-docs`
- initial content sources
- selected runtime profile
- empty or pending `currentVersionId`
- empty or pending `sandboxBindingId`

The creation step does not need a product UI in this phase. It can be driven by
runtime API tests, CLI scripts, or a local test harness.

### 4.2 Generation Build

The build run must:

- receive confirmed input context, including content sources and brief/design
  markdown when available;
- claim or create an editable sandbox binding;
- initialize the project source under the declared `appRoot`;
- install dependencies through `package.install`;
- build through `project.build`;
- start preview through `preview.start`;
- emit `preview.candidate`;
- promote exactly one current version with `preview.updated`;
- complete only after the promoted preview exists.

The generated source must stay in the project workspace and become the base for
future edits.

### 4.3 Prompt Edit

An edit run must:

- start from `baseVersionId` and the project's existing `sandboxBindingId`;
- read `state/project.json`;
- treat `appRoot` as the only app root;
- inspect existing source files;
- apply focused source changes through `fs.read` and `fs.patch` / `fs.write`;
- install dependencies through `package.install` only when dependencies change;
- rebuild through `project.build`;
- start preview through `preview.start`;
- emit a candidate preview;
- promote a new current version with `preview.updated`.

Editing must modify the existing source tree. It must not regenerate the entire
artifact from the original template unless a repair/rebuild flow explicitly
requests that behavior.

### 4.4 Build Evidence

`project.build` is the formal build evidence for Website and Docs projects.

Shell commands may be useful for diagnostics, but lifecycle acceptance must not
depend on manually running `npm run build` through `shell.run`. The runtime
needs the structured `project.build` result, build log, preview metadata, and
promotion event ordering.

## 5. Lifecycle State

Add or normalize a lifecycle state shape that can be stored in
`state/project.json` and mirrored by the runtime store:

```json
{
  "projectId": "project_123",
  "kind": "website",
  "templateKey": "astro-website",
  "appRoot": "project",
  "sandboxBindingId": "sandbox_binding_123",
  "briefId": "brief_123",
  "currentVersionId": "version_123",
  "lastBuildRunId": "run_123",
  "lastEditRunId": null,
  "sourceSnapshotUri": "runtime://snapshots/project_123/version_123",
  "createdFrom": "public-runtime-run"
}
```

Required invariants:

- One project has one stable `appRoot`.
- One project has one editable current source snapshot.
- One project has one current promoted preview.
- `currentVersionId` changes only after `preview.updated`.
- Build/edit runs cannot complete before preview promotion.
- A project with a promoted version must expose enough metadata to start an edit
  run without guessing.

## 6. Harness Control-Plane Invariants

These invariants are required for the runtime harness to remain correct once
generation and edit are connected into one lifecycle.

### 6.1 Runtime Store Is The Control-Plane Source Of Truth

`state/project.json` is a workspace hint file for agents. It may include
`appRoot`, `templateKey`, framework, package manager, registry, and template
version.

The runtime store is the source of truth for control-plane fields:

- `sandboxBindingId`
- `currentVersionId`
- `sourceSnapshotUri`
- active mutable run lock
- latest successful build evidence
- promoted preview state

Agent-written files must not be trusted as the authority for binding ownership,
current version, or promotion state. If a value appears in both places, runtime
store wins.

### 6.2 Public API Must Expose The Editable Binding

The product path needs a stable way to obtain the binding required by the next
edit run. One of these contracts must be implemented before the lifecycle is
considered complete:

- extend `/preview/{projectId}/current` with `sandboxBindingId` and
  `sourceSnapshotUri`;
- add `GET /projects/{projectId}/runtime-state`;
- or extend build completion metadata with a public project runtime state
  payload.

The chosen contract must be represented in shared schemas. Product/BFF code
must not scrape internal events or call `/internal/template-build` to discover
the editable binding.

### 6.3 Edit Must Verify Or Restore The Base Snapshot

Edit runs cannot assume that the current sandbox workspace still matches the
requested `baseVersionId`.

Before editing, the runtime must either:

- verify the workspace source hash/snapshot marker matches `baseVersionId`; or
- restore the workspace from the version's `sourceSnapshotUri`.

If restore is not implemented in the first local milestone, the runtime must at
least reject mismatched or missing snapshot markers with a conflict instead of
editing an unknown workspace state.

### 6.4 Project-Level Mutable Run Lock

Only one mutable run may own a project's editable workspace at a time.

Mutable phases include:

- `build`
- `edit`
- `repair`
- future export phases that mutate source or build output

Starting a mutable run must acquire a project-level lock. The lock is released
when the run reaches a terminal status. If another mutable run is active, the
runtime must return conflict rather than allowing two runs to race on the same
workspace.

For edit runs, `baseVersionId` must equal the project's `currentVersionId`
unless the runtime explicitly supports branch/fork editing. This phase does not
include branch editing, so stale base versions must be rejected.

### 6.5 Promotion Must Bind To Build Evidence

Every candidate version must be tied to the latest successful `project.build`
inside the same run.

Candidate metadata must include:

- `sourceSnapshotUri`
- build log URI or build evidence id
- preview URL
- optional screenshot id
- creating run id

Promotion may only promote a candidate that belongs to the same run and matches
the latest successful build evidence. This prevents an agent from reporting an
old preview URL or promoting output that was not produced by the current build.

## 7. API Contract For This Phase

Use the public runtime API as the product path:

| Method | Path | Role |
|---|---|---|
| `POST` | `/runs` | Start brief/build/edit runs |
| `POST` | `/runs/{runId}/continue` | Send edit prompt or resume paused run |
| `GET` | `/runs/{runId}/events` | Observe lifecycle and preview events |
| `GET` | `/preview/{projectId}/current` | Resolve current promoted preview |
| `GET` | `/preview/{projectId}/{versionId}` | Inspect candidate or historical preview |
| `GET` | `/projects/{projectId}/conversation` | Read user-visible conversation history |
| `GET` | `/projects/{projectId}/runtime-state` | Resolve editable lifecycle metadata, if this endpoint is chosen |

`/internal/template-build` may remain as a local test or admin helper, but it
must not be required for the normal product lifecycle. If it is kept for
developer speed, it should either reuse the same lifecycle state creation path
or clearly mark its output as non-editable.

The final API choice for editable metadata must expose at least:

```json
{
  "projectId": "project_123",
  "currentVersionId": "version_123",
  "sandboxBindingId": "sandbox_binding_123",
  "sourceSnapshotUri": "runtime://snapshots/project_123/version_123",
  "appRoot": "project",
  "templateKey": "astro-website"
}
```

## 8. Implementation Plan

### M1. Persist Editable Project Lifecycle State

- Ensure build generation writes `state/project.json` with `projectId`,
  `templateKey`, and `appRoot`.
- Persist `projectId -> sandboxBindingId` in the runtime store.
- Persist `projectId -> currentVersionId -> sourceSnapshotUri`.
- Return or expose enough metadata for the next edit run to reference the same
  project and sandbox binding.
- Decide and implement the public editable-metadata contract:
  `/preview/{projectId}/current` extension or
  `/projects/{projectId}/runtime-state`.

Acceptance:

- A promoted generated project has a retrievable `sandboxBindingId`.
- `GET /preview/{projectId}/current` resolves the promoted version.
- The source snapshot and `appRoot` are known after generation.

### M2. Unify Template Build With Runtime Build Semantics

- Website generation should use the same build lifecycle whether invoked by
  product API, test harness, or internal helper.
- Docs generation should follow the same lifecycle once the Fumadocs template
  loop is enabled.
- Internal helper output should not bypass version, checkpoint, or sandbox
  state if the output is intended to be editable.

Acceptance:

- A generated Website project can immediately start a standard edit run.
- A generated Docs project follows the same lifecycle contract.
- No product-facing flow needs `/internal/template-build`.

### M3. Standard Edit Run End To End

- Start edit with `phase: "edit"`, `baseVersionId`, and `sandboxBindingId`.
- Reject stale `baseVersionId` unless branch editing has been explicitly added.
- Verify or restore the workspace source snapshot before applying changes.
- Continue the run with a user prompt.
- Verify the agent reads existing source and applies focused changes.
- Verify `project.build` runs after the edit.
- Verify `preview.updated` promotes a new version.

Acceptance:

- Editing a Website hero title changes the existing source and promoted preview.
- Editing a Docs page title or section changes existing MDX/content and promoted
  preview.
- The `/current` preview URL stays stable while its promoted version changes.

### M4. Docs Template Loop

- Add or finish `fumadocs-docs` project initialization.
- Ensure docs source files live under the same lifecycle-managed `appRoot`.
- Define the minimum Docs source contract:
  - MDX/page file location;
  - route/slug mapping;
  - navigation/sidebar source;
  - home/index page behavior;
  - metadata fields that edits are allowed to update.
- Build docs through `project.build`.
- Start preview and promote versions through the same preview pipeline.
- Support prompt edits to docs content and navigation.

Acceptance:

- Prompt / markdown input can generate a Docs project.
- A chat edit can modify an existing docs page and rebuild successfully.
- Website and Docs have the same lifecycle event shape.

### M5. Regression Suite

Add focused tests before calling the phase complete:

- API: build run creates promoted preview and persisted sandbox binding.
- API: public runtime-state or preview-current response exposes editable
  metadata needed for the next edit.
- API: edit run without binding still returns the existing validation error.
- API: lifecycle-created project can start edit with the stored binding.
- API: stale `baseVersionId` is rejected while branch editing is unsupported.
- API: concurrent mutable run on the same project is rejected.
- Agent: edit modifies existing files rather than template-regenerating.
- Agent: `run.completed` cannot appear before `preview.updated`.
- Agent: candidate promotion is tied to latest successful build evidence.
- Sandbox: nested package roots remain denied.
- Website E2E: create -> generate -> edit hero title -> build -> current preview
  changed.
- Docs E2E: create -> generate -> edit docs content -> build -> current preview
  changed.

## 9. Product Decisions

Recommended decisions for this phase:

- Use one long-lived sandbox binding per project in local runtime.
- Treat source snapshot verify/restore as part of edit correctness, not only
  production hardening. A local implementation may start with verification and
  conflict rejection before full restore exists.
- Keep `/internal/template-build` gated and non-product.
- Make the product/BFF responsible for storing and passing `sandboxBindingId`,
  unless runtime adds a public project lookup endpoint that can resolve it
  server-side.
- Keep DesignProfile out of the data model until the lifecycle is green.

## 10. Done Criteria

This phase is complete when both Website and Docs satisfy:

```text
Create
  -> Generate
  -> Build
  -> Promote current preview
  -> Prompt edit
  -> Rebuild
  -> Promote next current preview
```

Required proof:

- The flow uses public runtime APIs.
- The generated project has persisted editable lifecycle state.
- The edit flow uses the existing source tree.
- The edit flow verifies or restores the source snapshot for `baseVersionId`.
- Stale base versions and concurrent mutable runs are rejected.
- Build evidence comes from `project.build`.
- Candidate promotion is bound to the current run's latest successful build
  evidence.
- `preview.updated` precedes completion.
- `/preview/{projectId}/current` remains the stable read URL, and the chosen
  public runtime-state contract exposes the editable binding for the next edit.
- Regression tests cover Website and Docs lifecycle paths.

Only after this lifecycle is reliable should DesignProfile be introduced as a
style intelligence layer on top of the existing project/edit/build machinery.

## 11. Implementation Progress

Current implementation decisions:

- The editable metadata contract is `GET /projects/{projectId}/runtime-state`.
- Edit runs are created first and wait for `POST /runs/{runId}/continue`
  before spawning the edit agent.
- Same-project mutable runs are locked for top-level mutable phases.
- Edit runs reject stale `baseVersionId` and require a source snapshot on the
  current promoted version.
- Edit runs restore the workspace from the `baseVersionId` source snapshot
  before waiting for `continue`.
- Source snapshot write and restore now go through the `WorkspaceBackend`
  abstraction instead of assuming direct local filesystem access.
- The workspace-channel protocol includes `fs.copyDir`, allowing Kubernetes
  sandbox workspaces to perform byte-level directory snapshot copy/restore
  through the workspace server.
- `project.init` now writes a real `fumadocs-docs` source contract instead of
  an Astro placeholder when agents initialize Docs projects.
- `project.build` validates the Fumadocs Docs source contract before running
  the build command.
- `preview.report_candidate` must match the latest successful `project.build`
  `sourceSnapshotUri`.

Current regression coverage:

- Website public lifecycle:
  create/build -> runtime-state -> edit/continue -> rebuild -> promote.
- Docs public lifecycle:
  create/build -> runtime-state -> edit/continue -> rebuild -> promote.
- Edit-start race regression:
  edit run creation does not consume model turns before `continue`.
- Promotion evidence regression:
  candidate source snapshot mismatch is rejected.
- Snapshot restore regression:
  Website and Docs lifecycle tests intentionally corrupt the workspace before
  edit; the edit succeeds only after restoring the base version source snapshot.
- Workspace-channel restore primitive regression:
  `JsonWorkspaceChannelBackend.copy_dir_all` emits `fs.copyDir`, and the real
  `workspace-channel-server.js` copies directory snapshots while skipping
  excluded directories such as `node_modules`.
- Docs source contract regression:
  `project.init` writes Fumadocs package/source/layout/content files, a valid
  Fumadocs Docs contract can build, and missing contract files are rejected
  before `npm run build` starts.
- Kubernetes workspace-channel E2E:
  `infra/agent-sandbox/run-k8s-e2e.sh` validates read/write, `process.exec`,
  and `fs.copyDir` against a real k3d sandbox channel after rebuilding and
  importing the sandbox image.

Runtime-owned implementation status:

- No runtime-owned blocker remains for this phase.
- `/internal/template-build` remains a gated developer/admin helper; the
  product lifecycle regression path uses public runtime APIs.
- No separate product/BFF implementation exists in this repo beyond shared
  schemas and the mock BFF contract tests. Real UI/BFF wiring against
  `GET /projects/{projectId}/runtime-state` is a product integration follow-up
  outside the runtime harness.

Latest local verification:

- `cargo fmt --manifest-path services/runtime/Cargo.toml -- --check`
- `cargo test --manifest-path services/runtime/Cargo.toml --test http_api`
- `cargo test --manifest-path services/runtime/Cargo.toml --test sandbox_tools`
- `cargo test --manifest-path services/runtime/Cargo.toml --test agent_loop`
- `cargo test --manifest-path services/runtime/Cargo.toml --test preview_promotion`
- `cargo test --manifest-path services/runtime/Cargo.toml --test edit_agent`
- `cargo test --manifest-path services/runtime/Cargo.toml --test mock_bff_contract`
- `cargo test --manifest-path services/runtime/Cargo.toml --test astro_build_agent docs`
- `ANYDESIGN_E2E_RESET_WARM_POOL=1 bash infra/agent-sandbox/run-k8s-e2e.sh`
- `npm test --prefix packages/shared -- --run`
