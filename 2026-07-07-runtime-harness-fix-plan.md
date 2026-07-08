---
date: 2026-07-07
status: draft
type: fix-plan
topic: runtime-harness-stabilization
evidence:
  - run-28: deepseek-chat build stream, generated styled Astro site but promotion rejected by stale build log signal
  - run-1: deepseek-v4-pro brief stream, completed brief confirmation
  - run-31: deepseek-v4-pro build stream, cancelled after cwd split, interactive Astro add, and default Astro output
references:
  - ./2026-07-04-mvp-implementation-plan.md
  - ./2026-07-04-rust-agent-harness-delivery-review.md
  - ./2026-07-04-rust-runtime-spec.md
  - /Users/carlos/Downloads/claude-code-main/src/utils/cwd.ts
  - /Users/carlos/Downloads/claude-code-main/src/utils/Shell.ts
  - /Users/carlos/Downloads/claude-code-main/src/tasks/LocalShellTask/LocalShellTask.tsx
---

# Runtime Harness 修复计划

## 1. 结论

这次 `design.md -> Astro website` 的真实 API 测试说明：当前失败主要不是单一模型能力问题，而是 runtime harness 的工具契约、工作目录、非交互命令、依赖安装和 preview promotion 判定不够稳。

`deepseek-chat` 曾经生成了可访问且有样式的 Astro website，但 promotion gate 因历史 build log 误判而没有正确收尾。`deepseek-v4-pro` 在同一用例里反复把 `project/` 和 `project/project/` 混用，并在 `npm exec astro add tailwind` 的交互提示中卡住，最后把工作区 flatten 成默认 Astro 页面。

证据说明：原始 SSE 文件当前保存在 `/tmp`，只适合作为本机短期调试材料。进入实现评审前，应把关键 stream 摘要固化到仓库内的 `evidence/` 或测试 fixture 中，至少包含 run id、模型名、关键事件 id、错误摘要和最终状态，避免 `/tmp` 清理后失去复盘依据。

因此修复方向应该是：

- 不继续依赖 prompt 强提醒模型。
- 把项目根目录、依赖安装、框架初始化、非交互执行、build 状态和 preview promotion 下沉成 runtime 强约束。
- 参考 Claude Code 的 cwd 状态机、shell watchdog、权限/工具 schema 分离思路，但结合本项目 artifact runtime 的长生命周期和 preview promotion 做更窄、更专用的实现。

## 2. 失败现象归因

### 2.1 工作目录分裂

现象：

- 模型在 workspace 根下运行 `npm create astro@latest project ...`，生成 `project/project`。
- 后续又读取 `project/package.json`、`project/src/pages/index.astro`，导致找不到文件。
- 纠偏后又在 `project/` 和 `project/project/` 之间来回操作。
- 最后尝试 flatten/delete，保留下来的页面退化为默认 Astro。

当前代码原因：

- `shell.run` 支持 `cwd`，但没有 run 级别或 project 级别的 cwd 状态。
- `shell.run` 缺省 cwd 是 `default_project_dir(ctx)`。
- `package.install` 不支持 `cwd`，也固定使用 `default_project_dir(ctx)`。
- `fs.*` 工具只接受 workspace path，和 shell 的 cwd 不是同一个概念。

修复判断：

必须引入明确的 `appRoot` / `projectRoot` 状态，所有 build/edit 阶段工具默认指向这个 root，而不是让模型在 `project`、`.`、`project/project` 之间猜。

### 2.2 交互式命令卡住

现象：

- `npm exec astro add tailwind` 输出 `Continue?` 后等待输入。
- run 的 `continue` 纠偏被排队，但当前 shell 未结束，纠偏无法立即生效。
- 人工 kill 后模型继续走错目录。

当前代码原因：

- `LocalCommandBackend` 使用 `Command::output()`，一次性等待进程结束。
- 没有流式输出观察、prompt 检测、交互式卡住 watchdog。
- 只有 timeout，缺少 "prompt-like stalled output" 的中断和反馈。

Claude Code 借鉴：

- `LocalShellTask` 有 output stall watchdog，检测 `(y/n)`、`Continue?`、`Press Enter` 等提示。
- 命中后通知模型：kill task，使用 piped input 或非交互 flag 重新运行。

修复判断：

runtime 需要在 shell 层识别交互式 prompt，并快速返回 recoverable error，避免整个 run 被长时间阻塞。

### 2.3 `package.install` 契约不一致

现象：

- permission 阶段允许 registry 缺省或 public registry 通过局部策略。
- execute 阶段报 `package.install requires registry`。

当前代码原因：

- `check_permission` 中 `registry.is_none_or(is_public_registry)` 把缺省 registry 与 public registry 混在一起处理。
- `call` 中又强制 `registry` 存在。
- schema 文案写的是 `Internal registry URL`，但本地真实 E2E 用例需要可配置 registry，且生产安全基线要求内部 registry/proxy。

修复判断：

`registry` 应该 optional，缺省使用 `RUNTIME_NPM_REGISTRY` 或内部 registry/proxy；local E2E/dev profile 可以显式 opt into public npm，并写入 audit。同时 `package.install` 应支持 `cwd`，默认使用 `appRoot`。

### 2.4 框架初始化过度依赖模型

现象：

- 模型先脚手架，再重复检查错误目录。
- Astro Tailwind 初始化走了 `astro add tailwind`，引入交互和路径副作用。
- Fumadocs/Astro 这类项目的初始化和修改不适合完全走通用 shell flow。

修复判断：

框架初始化应该提供专用工具或强 contract：

- `project.init`
- `project.detect_root`
- `project.build`
- `package.install`
- `preview.start_from_project`

模型可以继续负责页面内容和代码生成，但项目结构和 build/preview 生命周期应由 harness 收敛。

### 2.5 Promotion gate 误判

现象：

- 上一次成功作品手动 build 可通过，但 runtime promotion 因 "build log contains a terminal error" 拒绝。
- 当前代码如果 build log 缺失也默认视为 terminal error。

当前代码原因：

- promotion gate 从 `outputs/build/build.log` 读文本并扫 terminal error。
- 缺失 build log 时 `map_or(true, has_terminal_error)`。
- 没有结构化的 latest build attempt 状态。

修复判断：

promotion gate 应只看最近一次结构化 build 结果，而不是历史日志全文或缺失日志。

## 3. 修复目标

### 3.1 功能目标

- Website 用例在真实 DeepSeek/OpenAI/其他模型下稳定生成有样式 Astro site。
- Docs 用例在 Phase A.5 复用同一 project root、install、build、preview 流程；P0 只验收 Astro website。
- 模型使用通用 `fs.*` 写代码时，不再能把项目拆成 `project/project`。
- 交互式命令不会无限阻塞 run。
- `package.install` 缺省 registry 可用，并遵守内部 registry/proxy 优先的安全基线。
- promotion gate 的 build 输入只看 latest build status，不被旧错误日志污染；review/safety/browser/screenshot gate 仍保留。

### 3.2 非目标

- 不在本阶段实现完整前端产品 UI。
- 不在本阶段接入外部发布。
- 不在本阶段做完整 Docusaurus/Next.js 多模板质量保证。
- 不要求完全复刻 Claude Code 的 BashTool、权限 UI、任务系统。

## 4. 设计原则

1. **模型负责生成内容，harness 负责结构和生命周期。**
2. **工作目录必须是 runtime 状态，不是模型记忆。**
3. **工具 schema、permission、execute 三者必须一致。**
4. **所有脚手架和依赖安装都必须非交互。**
5. **promotion 只看当前候选版本的结构化证据。**
6. **失败反馈要能指导模型下一步，而不是只返回底层错误。**

## 5. 分阶段计划

### P0. 立即修复

P0 目标：让当前真实 `design.md -> Astro website` 用例稳定通过。

P0 边界必须一次性闭合以下链路：

```text
project.init
  -> appRoot locked
  -> package.install uses appRoot
  -> fs.* edits appRoot source
  -> project.build writes latest build status
  -> preview starts from appRoot and reports candidate
  -> promotion gate reads latest build status plus review/safety checks
```

如果 P0 只修其中一半，例如只改 promotion gate 但没有任何工具写 `outputs/build/latest.json`，会形成新的假失败。因此 `project.init`、`project.build`、preview appRoot、runtime policy profile、动态 shell watchdog 和 latest build status 都属于 P0 验收边界。

#### P0.1 引入 `appRoot` 状态

新增状态文件：

```text
state/project.json
```

建议结构：

```json
{
  "appRoot": "project",
  "template": "astro-website",
  "packageManager": "npm",
  "framework": "astro",
  "detectedAt": "2026-07-07T00:00:00Z"
}
```

规则：

- Build run bootstrap 时写入默认 `appRoot = "project"`。
- `project.init` 成功后直接锁定真实 root。
- P0 增加内部 helper `detect_app_root`，用于容错识别已有 `package.json`；它不是模型可调用工具。
- 如果内部 helper 发现 `project/project/package.json`，不要继续让模型猜；runtime 应返回明确 guidance 或锁定 `appRoot = "project/project"`，但不自动删除或 flatten 目录。
- 后续 `shell.run`、`package.install`、`project.build`、preview 默认使用 `appRoot`。
- Build/Edit 阶段的 source 写入必须 appRoot-aware：允许写 `appRoot/**`、`state/**`、`inputs/**`、`outputs/**` 的受控路径；如果 `fs.write` / `fs.patch` 会创建第二个 package root（例如 `project/project/package.json`），必须返回 recoverable guidance，而不是静默允许。

需要修改：

- `services/runtime/src/agent_loop.rs`
- `services/runtime/src/tools/sandbox.rs`
- 可能新增 `ProjectState` helper 到 `services/runtime/src/tools/project.rs` 或 `sandbox.rs` 内部 helper。

测试：

- `project.init` 在 workspace root 下创建 `project/package.json`。
- 如果已有 `project/project/package.json`，内部 `detect_app_root` 能返回 `project/project` 或明确报告多 root 冲突。
- `package.install` 默认 cwd 跟随 `state/project.json.appRoot`。
- Build/Edit 阶段写入 nested package root 会被阻断或返回 recoverable guidance。

#### P0.2 修复 `package.install`

修改行为：

- `registry` optional。
- 缺省 registry 来自 `RUNTIME_NPM_REGISTRY` 或内部 registry/proxy。
- 只有显式 `RuntimePolicyProfile = local-e2e` 可以 opt into `https://registry.npmjs.org/`，并且必须进入 audit；`production` / production-like profile 必须 deny public registry。
- 增加 optional `cwd`，必须在 workspace 内，缺省 `appRoot`。
- permission 和 execute 对 registry 的处理保持一致。
- 返回里包含实际 `cwd` 和 `registry`。

建议 schema：

```json
{
  "packages": ["@tailwindcss/vite", "tailwindcss"],
  "cwd": "project",
  "registry": "https://registry.internal.example/npm/",
  "timeoutMs": 120000
}
```

需要修改：

- `services/runtime/src/tools/sandbox.rs`
- `services/runtime/tests/sandbox_tools.rs`
- `services/runtime/tests/permission_engine.rs`

验收：

- 模型调用 `package.install` 只有 `packages` 时不失败。
- 内部环境缺省走配置的 registry/proxy。
- public npm registry 只在 `local-e2e` policy profile 下 ask/allow，并写 audit。
- production-like profile 指定 public npm registry 时 deny。
- 事件流不再出现 `package.install requires registry`。

#### P0.2a 明确 runtime policy profile

新增 runtime policy profile：

```text
production
local-e2e
```

规则：

- `production` 是默认值，适用于内部 sandbox、k3d production-like 测试和后续产品路径。
- `local-e2e` 只能通过显式环境变量或测试配置打开，不能由模型输入或普通 StartRun payload 设置。
- `local-e2e` 允许 public npm registry 走 ask/allow 路径，但仍必须写 audit event，包含 run id、project id、registry、packages 和触发工具。
- NetworkPolicy 层仍以 production-like deny public internet 为默认；local E2E 如需公网，需要测试 harness 显式配置，不得由 agent 动态打开。

需要修改：

- `services/runtime/src/config.rs`
- `services/runtime/src/permission.rs`
- `services/runtime/src/tools/sandbox.rs`
- `services/runtime/tests/permission_engine.rs`

#### P0.3 禁止或中断交互式命令

第一阶段可以先做两层防护。

静态防护：

- `check_command_policy` 对高风险交互命令返回 recoverable guidance。
- 命中命令包括：
  - `npm exec astro add`
  - `npx astro add`
  - `astro add`
  - 未带 `--yes` 的 `npm create astro`
  - 其他已知 scaffold/add 命令。

动态防护：

- `shell.run` 改成 spawn + streaming buffer。
- 如果输出尾部停滞并匹配 prompt pattern，kill 进程树，返回 recoverable error。
- 动态 watchdog 属于 P0 验收；只要 P0 仍允许 `shell.run`，不能只依赖静态 deny list。

Prompt patterns：

```text
Continue?
(y/n)
[Y/n]
Do you ...?
Would you ...?
Are you sure ...?
Press Enter
Overwrite?
```

需要修改：

- `services/runtime/src/tools/sandbox.rs`
- `services/runtime/src/permission.rs`
- 可新增 `services/runtime/src/tools/shell_watchdog.rs`

验收：

- `npm exec astro add tailwind` 不会挂住 60s。
- 事件流返回类似：

```text
interactive prompt detected: Continue?. Re-run with --yes or use package.install plus fs.write.
```

#### P0.4 增加 `project.init` 专用工具

新增工具：

```text
project.init
```

输入：

```json
{
  "template": "astro-website",
  "path": "project"
}
```

行为：

- 验证 `path` 是 workspace 下一级目录。
- 如果目录不存在，创建。
- P0 不使用官方 scaffold 命令；runtime 直接写最小 Astro 文件集。
- 然后通过 `package.install` 或内部 install path 安装 `astro`。
- 不允许 `project.init` 调用 `astro add`、`npm create astro` 这类会随上游版本和交互行为漂移的命令。
- `project.init` 是模板生命周期原子工具，不生成业务页面内容、信息架构或最终样式；这些仍由 agent 通过 `fs.*` 生成。
- `project.init` 必须可复现：基础依赖版本、package manager、lockfile 策略和 registry 来源必须由模板策略决定，不能依赖当天 npm resolution。

最小 Astro 文件：

```text
project/package.json
project/astro.config.mjs
project/tsconfig.json
project/src/pages/index.astro
project/public/favicon.svg
project/package-lock.json 或模板声明的等价 lockfile
```

然后把 `appRoot` 锁定为 `project`。

验收：

- 不再出现 `project/project`。
- 初始化后立即能 `npm run build`。
- 模型第一轮可以直接编辑 `project/src/pages/index.astro`。
- 连续两次 `project.init` 在相同模板版本下产生等价依赖声明和 lockfile。

#### P0.5 修复 build 状态和 promotion gate

新增结构化 build 状态：

```text
outputs/build/latest.json
```

结构：

```json
{
  "id": "build-...",
  "cwd": "project",
  "argv": ["npm", "run", "build"],
  "status": "success",
  "exitCode": 0,
  "startedAt": "...",
  "completedAt": "...",
  "logPath": "outputs/build/build-....log"
}
```

promotion gate 修改：

- 不再用整个历史 `build.log` 扫 terminal error。
- build gate 只接受 `outputs/build/latest.json.status == "success"`。
- review/safety/browser/screenshot gate 继续按既有 Phase A 规范执行。
- 如果没有 latest build，明确报 `build has not run`，而不是 `build failed`。

写入责任：

- P0 必须新增 `project.build`，由该工具负责运行 framework build 并写 `outputs/build/latest.json`。
- `shell.run npm run build` 不作为 promotion gate 的正式证据来源，除非后续显式接入 build status 写入。
- Build/Edit prompt 必须要求模型使用 `project.build`，而不是裸 `shell.run` build。

需要修改：

- `services/runtime/src/tools/sandbox.rs`
- `services/runtime/src/preview.rs`

验收：

- 历史 stderr 不影响最新成功 build。
- 缺少 build 记录时错误信息准确。

#### P0.6 让 preview 使用 `appRoot`

P0 必须修复 preview root，不能等到 P1。

可选实现：

- 改造现有 `preview.start`，默认读取 `state/project.json.appRoot`。
- 或新增最小 `preview.start_from_project`，但 P0 只要求 Astro/static dist 路径。

行为：

- 启动 preview 时使用 `appRoot` 作为 cwd。
- 写入 `state/preview.json`，包含 `cwd`、`port`、`url`、`status`、`candidateVersionId`。
- 如果 `appRoot/dist` 不存在或 build status 不是 success，返回 recoverable error。

验收：

- `project.build` 在 `project` 成功后，preview 不会从 workspace root 或 `project/project` 启动。
- `preview.candidate` 事件携带的 URL 对应 latest build 的 appRoot。

### P1. 稳定性增强

P1 目标：降低模型差异导致的失败率，让 build/edit/docs 都复用同一套工具闭环。

#### P1.1 暴露 `project.detect_root`

P0 只有内部 `detect_app_root` helper；P1 再把 root detection 暴露为模型可调用工具，用于 edit/repair/docs 等更复杂场景。

输入：

```json
{
  "preferred": "project"
}
```

输出：

```json
{
  "appRoot": "project",
  "packageJson": "project/package.json",
  "framework": "astro",
  "packageManager": "npm"
}
```

规则：

- 优先 `state/project.json.appRoot`。
- 如果缺失，扫描 workspace 下有限深度的 `package.json`。
- 如果发现多个候选，按框架和最近修改时间排序，但不要静默选择危险路径；返回 recoverable guidance 或自动锁定最可能 root。

#### P1.2 增强 `project.build`

P0 已经需要最小 `project.build` 来写 latest build status。P1 在此基础上增强多 framework、多 package manager 和更丰富诊断。

输入：

```json
{
  "cwd": "project"
}
```

行为：

- 根据 package manager 运行 build。
- 写 `outputs/build/latest.json`。
- 写 build log。
- 返回结构化状态。

这样模型不需要用 `shell.run npm run build` 作为正式 build evidence，promotion 也有统一证据。`shell.run pnpm build` 仍可作为 repair/diagnostics 命令保留，但不写 latest build status，除非后续显式接入。

#### P1.3 增强 `preview.start_from_project`

输入：

```json
{
  "cwd": "project",
  "port": 4321
}
```

P0 已经要求 preview 使用 appRoot。P1 在此基础上增强多 framework、多 package manager 和长期 preview server 管理。

行为：

- 对 Astro 使用 `npm run preview -- --host 127.0.0.1 --port <port>` 或静态 dist server。
- 写 `state/preview.json`。
- 验证 URL 可达。
- 可选触发截图。

#### P1.4 调整 Build/Edit system prompt

改成让模型使用专用工具：

```text
Use project.init when project state is missing.
Use project.detect_root before package/build/preview operations.
Use package.install for dependencies.
Use project.build, not shell.run npm run build.
Use the project-aware preview tool, not arbitrary preview commands.
Use fs.* only for source/content edits under appRoot.
```

同时删除容易诱导 `npm create astro@latest project` 的 prompt 示例。

#### P1.5 文件写入保护

针对 build run：

- 禁止删除 `appRoot` 之外的 `project` 目录树，除非工具明确处于 `project.init` 修复流程。
- `fs.delete` 对 `project`, `project/src`, `project/package.json` 等关键路径要求更高权限或专用 repair action。
- 防止模型 flatten 时误删生成物。

### P2. 质量与可观测性

P2 目标：让失败可诊断，让跨模型对比可自动化。

#### P2.1 运行健康报告

每次 run 完成或取消时写：

```text
outputs/run-health.json
```

包含：

- model
- tool failure count
- recoverable errors
- cwd changes
- appRoot
- build status
- preview status
- promotion status
- final URL

#### P2.2 自动 E2E 用例

新增真实/模拟分层测试：

- Unit tests：工具 schema、cwd、registry、promotion gate。
- Integration tests：mock model 按路径错误、registry 缺失、交互命令触发。
- Real model smoke tests：DeepSeek/OpenAI 各跑一次 design.md website。

建议用例：

1. `design.md` -> Astro website with inline CSS。
2. 用户要求 Tailwind，但禁止 `astro add` 后仍能用 `package.install + fs.write` 完成。
3. 模型误创建 `project/project`，runtime 自动 detect root。
4. 最新 build 成功但旧 build log 有 error，promotion 通过。
5. 交互式 `Continue?` 命令被 watchdog 中断。

#### P2.3 Stream 诊断规范

SSE 中增加标准事件：

```text
project.root.detected
project.root.changed
build.started
build.completed
interactive_prompt.detected
promotion.gate.checked
```

让前端和人工排查不必只读 shell stderr。

## 6. 具体代码改动清单

### 6.1 `services/runtime/src/tools/sandbox.rs`

P0 修改：

- `PackageInstallTool` registry optional，缺省内部 registry/proxy。
- `PackageInstallTool` 支持 `cwd`。
- `ShellRunTool` 加交互式命令静态拦截和动态 watchdog。
- 引入或调用 `project_state` helper。
- 新增或注册 `ProjectInitTool`。
- 新增或注册最小 `ProjectBuildTool`，由它写入 latest build status。
- `fs.write` / `fs.patch` 在 Build/Edit 阶段增加 appRoot-aware nested package root 检测。
- `preview.start` 读取 appRoot，或新增最小 `PreviewStartFromProjectTool`。

P1 修改：

- 增强 `ProjectBuildTool`，支持多 framework/package manager。
- 新增模型可调用的 `ProjectDetectRootTool`。
- 增强 `PreviewStartFromProjectTool`。
- 或将 project tools 移入 `services/runtime/src/tools/project.rs`，在 registry 里注册。

### 6.2 `services/runtime/src/permission.rs`

P0 修改：

- 对 known interactive commands 给 recoverable denial。
- 对 `npm create astro` 未带非交互 flags 的命令给 guidance。
- 更新 dependency install 检测：依赖安装统一走 `package.install` 或 `project.init`；裸 scaffold/add 命令只允许非交互且通过专用工具包装的路径。

### 6.3 `services/runtime/src/agent_loop.rs`

P0 修改：

- bootstrap 写 `state/project.json` 初始状态。
- bootstrap 写入 runtime policy profile 到 run context，不允许模型覆盖。
- Build/Edit prompt 改为使用 project tools，不再鼓励裸 scaffold。
- max turns 可以保留，但不应成为主要补救手段。

### 6.4 `services/runtime/src/preview.rs`

P0 修改：

- `PromotionGateReport` 增加 latest build fields 或改造为结构化 build gate。
- promotion gate 的 build 判断改读 latest build status，review/safety/browser/screenshot gate 保持既有规则。
- 缺失 build 状态返回 `BuildMissing`，不要归类为 `BuildFailed`。

### 6.5 `services/runtime/tests/*`

新增或调整：

- `sandbox_tools.rs`
- `permission_engine.rs`
- `agent_loop.rs`
- `model_gateway.rs`
- `http_api.rs`

## 7. 测试矩阵

| 层级 | 用例 | 预期 |
|---|---|---|
| Unit | `package.install` 缺省 registry | 使用配置的内部 registry/proxy |
| Unit | `package.install` 指定 public npm in local E2E | ask/allow 且写 audit |
| Unit | `package.install` 指定 public npm in production-like profile | deny |
| Unit | `package.install` 指定 cwd | npm 在指定 appRoot 执行 |
| Unit | 内部 `detect_app_root` 发现 `project/project/package.json` | 返回真实 appRoot 或多 root guidance |
| Unit | `project.detect_root` 读取 `state/project.json` | 返回已锁定 appRoot |
| Unit | Build/Edit `fs.write` 创建 nested package root | recoverable guidance |
| Unit | `project.init` 相同模板版本重复执行 | 依赖声明和 lockfile 等价 |
| Unit | `promotion_gate` 旧日志有 error，latest build success 且 review/safety 通过 | gate 通过 |
| Unit | 缺少 latest build | gate 返回 build missing |
| Integration | `npm exec astro add tailwind` 输出 Continue | watchdog 中断并返回 guidance |
| Integration | build run bootstrap | 写入 `state/project.json` |
| Integration | model 创建 nested project | runtime detect root，不继续错误读取 |
| Integration | `project.build` 成功 | 写入 `outputs/build/latest.json` 且 gate 可读取 |
| Integration | preview after `project.build` | 从 appRoot 启动并写 `state/preview.json` |
| E2E | `deepseek-chat` + design.md website | 生成 styled site，promotion 通过 |
| E2E | `deepseek-v4-pro` + design.md website | 即使模型路径不稳，也不破坏 workspace；失败时能明确归因 |

## 8. 验收标准

P0 完成标准：

- `cargo test --manifest-path services/runtime/Cargo.toml` 通过，或仅保留已明确重基线的策略测试。
- 使用真实 API 跑 `design.md -> website`，最终 URL 可访问且不是默认 Astro 页面。
- SSE 中不再出现：
  - `package.install requires registry`
  - 长时间卡在 `Continue?`
  - `project/project` 被模型反复误用后不可恢复
- preview 从 appRoot 启动，`state/preview.json` 记录实际 cwd。
- public npm 只在 `local-e2e` policy profile 下 ask/allow 且有 audit；production-like profile deny。
- Build/Edit 写入 nested package root 返回 recoverable guidance。
- 手动 build 成功的产物不再因历史 stderr 被 promotion 拒绝。

P1 完成标准：

- Build/Edit run 默认通过 `project.*` 工具完成 init/build/preview。
- Astro 和 Fumadocs 都能复用 appRoot 状态。
- 模型可以失败，但 runtime 不会把工作区结构越修越坏。

P2 完成标准：

- 每次失败都有 `run-health.json` 和明确 stream 事件。
- 可以对比不同模型的失败点，而不是依赖人工 tail SSE。

## 9. 风险和权衡

### 风险：专用工具降低通用 vibecoding 灵活性

处理：

- 专用工具只负责结构性生命周期：init/root/install/build/preview。
- 页面代码、样式、内容仍由模型通过 `fs.*` 生成。

### 风险：自动 root detect 选错目录

处理：

- 限制扫描深度。
- 优先 `state/project.json`。
- 多候选时返回 recoverable error，要求模型或 runtime repair 明确选择。

### 风险：watchdog 误杀慢命令

处理：

- 只有输出停滞且尾部匹配 prompt pattern 才触发。
- 普通 build/test 只走 timeout，不因慢而误杀。

### 风险：local E2E 需要 public registry，但内部环境默认禁止公网

处理：

- 支持 `RUNTIME_NPM_REGISTRY` 配置，并把内部 registry/proxy 作为 production-like 默认值。
- 只有 local E2E/dev profile 可显式 opt into public npm。
- 工具 schema 仍保持 optional registry，但 permission 必须区分内部 registry、local E2E public registry 和生产 public registry。

## 10. 推荐实施顺序

1. 修 `package.install` registry optional + cwd。
2. 加 `state/project.json` 和 `appRoot` helper。
3. 加最小 `project.init`，优先 runtime 写 Astro minimal template。
4. 加 `project.build` 和结构化 latest build。
5. 改 promotion gate 的 build 判断读取 latest build，同时保留 review/safety/browser/screenshot gate。
6. 加 shell interactive watchdog。
7. 改 Build/Edit prompt 使用 project tools。
8. 跑真实 DeepSeek/OpenAI E2E，对比 stream。

## 11. 最小 P0 Patch 边界

如果只做最小可用修复，建议一次 patch 包含：

- `PackageInstallTool` registry optional。
- `PackageInstallTool` cwd optional，默认 `state/project.json.appRoot`。
- `PackageInstallTool` 默认使用内部 registry/proxy，local E2E/dev profile 才允许 public npm。
- 新增 runtime policy profile，默认 `production`，local E2E 例外必须显式配置和 audit。
- 新增 `state/project.json` bootstrap。
- 新增 `project.init` 写 Astro minimal template。
- 新增最小 `project.build` 写 `outputs/build/latest.json`。
- 对已知交互式 Astro/NPM 命令做静态拦截，并实现动态 interactive watchdog。
- preview 从 appRoot 启动并写 `state/preview.json`。
- Build/Edit 阶段阻断 nested package root 写入。
- `Build` prompt 改用 `project.init`。
- promotion gate 的 build 判断改为 latest build success，review/safety/browser/screenshot 判断保留。

如果需要拆 patch，动态 shell watchdog 和 preview appRoot 支持仍可作为独立 patch，但必须在 P0 验收前完成，不能后移到 P1。

## 12. 最终判断

参考 Claude Code 后，最有价值的借鉴不是某个具体 Bash 命令实现，而是三个工程原则：

- cwd 是 runtime 状态，不是模型记忆。
- 长命令和交互式命令必须被 harness 观察和中断。
- 工具 schema、权限判断、执行行为必须形成一个不可矛盾的 contract。

当前 runtime 应优先把这三个原则落地。完成后，模型差异仍会存在，但不会再把同一个用例从有样式作品退化成默认 Astro 页面。
