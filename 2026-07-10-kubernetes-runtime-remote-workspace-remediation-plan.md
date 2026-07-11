---
date: 2026-07-10
status: deployed-fixture-gate-implemented-release-blocked
type: remediation-plan
topic: kubernetes-runtime-remote-workspace
review_resolution: 2026-07-10
last_reviewed: 2026-07-11
evidence:
  - ./.runtime-evidence/k3d-zenova-sales-20260710-171024
  - ./services/runtime/target/e2e-evidence/k3d-channel-abc9119d3e34.json
  - ./services/runtime/target/e2e-evidence/public-runtime-fixture-abc9119d3e34.json
  - ./services/runtime/target/e2e-evidence/runtime-rc-abc9119d3e34-dirty-336319a30aca.json
  - ./.runtime-evidence/provider-deepseek-v4-pro-rc-combined/evidence-summary.json
related:
  - ./2026-07-04-internal-ai-website-docs-generator-requirements.md
  - ./2026-07-04-internal-ai-website-docs-generator-architecture.md
  - ./2026-07-04-rust-runtime-spec.md
  - ./2026-07-08-runtime-api-freeze.md
  - ./2026-07-08-project-lifecycle-generation-edit-build-plan.md
  - ./2026-07-09-runtime-harness-edit-loop-fix-plan.md
  - ./2026-07-10-design-profile-fidelity-remediation-plan.md
  - ./infra/agent-sandbox/README.md
---

# Kubernetes Runtime 远端 Workspace 全链路修复方案

## 1. 文档结论

截至 2026-07-11，本方案 P0-P3 和 P4 fixture gate 已完成实现。Runtime 的
Build -> Preview -> Promote 已经通过 Public Runtime API 在专用 k3d 中分别完成 Website
和 Docs 生命周期，不再通过宿主机 Workspace fallback 或手工 `kubectl exec` 补链路。

当前结论仍不是 release-ready。Runtime OCI/Kubernetes 部署证据已经补齐，Release Candidate
还缺以下独立条件：

- Preview proxy 尚未接入产品 Public API 的 authenticated principal 与 project ownership；
- `deepseek-v4-pro` 的 Website/Docs Build+Edit Harness gate 已通过，但当前证据来自测试内启动的
  Public API Runtime，不是 Kubernetes Deployment；R17 数据合规批准及部署版 real-provider
  gate 仍需单独完成；
- 最终候选证据必须在 clean worktree/候选 commit 上重跑；当前 OCI evidence 明确标记
  `repositoryDirty=true`，不能冒充最终发布制品。

2026-07-11 实施证据：

| Gate | 结果 | 关键证据 |
| --- | --- | --- |
| 全量 Runtime 回归 | 通过 | `cargo test --all-targets`；真实 provider 和重型 npm smoke 按条件 ignored |
| Remote FS boundary | 通过 | `check-remote-workspace-fs-boundary.sh` |
| k3d channel gate | 通过 | 双 claim 隔离、JWT、process lease、binary archive export |
| Website fixture | 通过 | 真实 Sandbox、Runtime proxy、1440×900 PNG、CAS promotion、release 后 artifact 200 |
| Docs fixture | 通过 | 独立 Fumadocs WarmPool；document/PNG hash 与 Website 不同 |
| Deployed Runtime fixture | 通过 | 集群内 Runtime + internal gateway HTTP；`/version`、image ref、Pod imageID 一致 |
| DeepSeek V4 Pro Harness | 通过 | Website/Docs Build+Edit 四阶段、artifact、snapshot 变化和 computed style |
| Evidence integrity | 通过 | 必填字段、事件顺序、镜像 config digest、secret-like scan |

以下内容保留为原始问题诊断和设计约束。

原始问题不是 DeepSeek 无法生成作品，也不是 agent-sandbox、PVC 或 Workspace Channel
不可用。修复前的真实 k3d 测试已经证明：

- Runtime 可以创建 `SandboxClaim`，并绑定独立 Sandbox 和 PVC；
- Brief Agent 可以通过真实 DeepSeek API 完成内容整理和确认；
- Build Agent 可以在 PVC 中完成 `project.init` 和页面源码生成；
- 同一份源码在 Sandbox Pod 内执行 `npm install` 和 `astro build` 可以成功；
- Pod 内启动的 Astro Preview 可以通过浏览器访问，桌面和移动端布局均可用。

Runtime API 失败的根因是 Harness 把三种不同的路径错误地表示成同一个
`std::path::PathBuf`：

```text
模型看到的逻辑路径          /workspace/project/src/pages/index.astro
Runtime 宿主机路径          /Users/.../.runtime-evidence/.../workspace/project/...
Sandbox Pod 内物理路径      /workspace/project/src/pages/index.astro
```

当 `style.update_tokens`、`project.ensure_dependencies`、`project.build` 和
`preview.publish` 对远端文件调用宿主机的 `canonicalize()`、`exists()`、`is_dir()`
或 `fs::*` 时，Runtime 会得到 `CannotResolve`，即使对应文件已经存在于 PVC。

因此，修复不能继续采用“遇到一个 `CannotResolve` 就替换一个调用点”的方式。
必须完成以下架构收敛：

```text
逻辑路径校验归 Runtime Path Policy
文件存在性和内容操作归 WorkspaceBackend
命令和依赖安装归 SandboxCommandBackend
候选预览归 PreviewBackend
正式版本导出和服务归 ArtifactPublisher
本地 k3d 连接归 SandboxChannelManager
```

目标不是让 Runtime 宿主机假装拥有 PVC 文件，而是让所有工具明确知道资源由谁
持有、在哪里执行、如何发布以及如何在重启后恢复。

本方案已经关闭架构评审中的关键分叉，MVP 固定采用：

```text
Workspace Channel authentication: Runtime-signed short-lived JWT
Candidate Preview port:          4321 for every template
Candidate Preview access:        Runtime /previews/{leaseId}/{*path} proxy
Desktop transport:               binding-scoped kubectl port-forward
In-cluster transport:            Sandbox Service DNS
Dependency registry:             in-cluster npm proxy Service
Artifact transaction:            stage -> gates -> CAS promote
Recovery:                        persisted lease/publish state + startup reconcile
```

实现阶段不得再把 mTLS/JWT、tunnel/proxy、public registry/internal proxy 或
single-phase/two-phase publish 作为临场选择。JWT 是 Desktop 与 in-cluster 两种拓扑都
必须实现的应用层身份契约；Production in-cluster channel 还必须使用 mTLS 或具备等价
workload identity 的 service mesh，只有 loopback k3d port-forward 允许明文 `ws://`。

### 1.1 Review 结论与处置

本次评审结论为：**方向正确，但只有在以下阻断项被写成硬契约后才可按本文实施；任何
单项完成都不能被解释为 Kubernetes Runtime 已稳定可用。**

| Review 发现 | 风险 | 文档处置 |
|---|---|---|
| 目标架构与当前实现状态混在一起 | 局部单测通过后过早宣布可用 | 状态改为 `implementation-in-progress`，第 3 节证据只代表历史基线，第 13 节是唯一 release 判定 |
| P0 同时依赖 P3 的完整 Channel Manager | 阶段顺序不可执行 | P0 只交付 binding-scoped resolver；持久化 tunnel、epoch reconcile 和并发隔离留在 P3 |
| Artifact CAS 与 SSE 发事件没有原子恢复约束 | current 已更新但客户端收不到 `preview.updated` | RuntimeStore current pointer + transactional outbox；重启后幂等补发 |
| JWT 有 `jti` 但未定义防重放 | 60 秒内 token 可被重复建连 | token 只允许一次成功 upgrade，服务端维护 TTL replay cache；每帧继续校验 connection capability |
| Preview Proxy 只写“鉴权”而未定义 principal | lease URL 可能成为永久 bearer URL | 复用 Public Runtime API principal，并校验 project/run ownership；leaseId 仅是 capability nonce，不替代用户鉴权 |
| k3d gate 未证明镜像来自当前 checkout | 可能在旧 zerondesign 镜像/旧集群上得到绿色结果 | 镜像必须带 Git SHA，导入目标 k3d，并校验 Pod `imageID`、Runtime commit 和测试 evidence commit 一致 |
| 两阶段发布缺少 crash-point 判定 | staged 孤儿、重复版本或 current 指针漂移 | 增加持久化 publish record、状态机、abort/GC 和 crash matrix |
| 真实 provider 与 PR gate 边界不清 | CI 不稳定或 release 缺少真实证据 | PR 使用确定性 provider；release candidate 必须执行 Website + Docs 真实 provider gate |
| source snapshot 只描述 URI，未定义恢复语义 | Edit 恢复时二进制资产丢失 | snapshot 使用 Runtime-owned immutable manifest，恢复通过 binary WorkspaceBackend 写回同一 binding |

### 1.2 范围与非目标

本文负责 Runtime、Sandbox、Workspace Channel、Preview、Artifact 和 k3d Harness 的远端
Workspace 生命周期闭环。以下事项不属于本轮：

- 不重做前端作品详情页，只保证其依赖的 SSE、candidate 和 current Artifact 契约稳定；
- 不新增第三种模板；Website/Astro 与 Docs/Fumadocs 是本轮完整覆盖面；
- 不把长期正式访问建立在 Pod、port-forward 或 PreviewLease 上；
- 不以真实 provider 的模型质量替代 Harness 的确定性一致性、安全和恢复测试；
- 不在本轮解决多 region object storage 复制，但 ArtifactPublisher 接口不得阻塞后续替换。

## 2. 产品目标与 Harness 不变量

本方案直接服务产品需求 R11-R15、R17 和 R21-R22：每个作品拥有独立 Sandbox，
生成过程可恢复，正式预览只在通过 Build/Review/Safety gate 后更新，失败时保留上一个
promoted 版本。

修复完成后必须满足以下不变量：

1. Build、Edit、Repair 的源码、依赖和构建输出始终属于同一个 Sandbox/PVC。
2. Runtime 不得使用宿主机 `fs::*` 判断远端文件是否存在。
3. `project.build` 必须在声明的 `appRoot` 中执行，并产生结构化 build evidence。
4. Candidate Preview 和 staged Artifact 必须读取同一个只读 candidate output snapshot，
   并绑定相同 manifest hash、本次 run、Sandbox、build evidence 和 source snapshot。
5. Promoted Artifact 必须在 Sandbox 停止或释放后仍可访问。
6. 成功路径中 `run.completed` 必须晚于 `preview.updated`。
7. 同一 `errorKind + tool + path` 最多自动重试 3 次，之后进入 `partial` 或
   `blocked`，不能让模型无限重复同一个失败调用。
8. Production Sandbox 默认不能访问公网 npm；依赖必须来自内部 registry/proxy。
9. Desktop + k3d 和 in-cluster Runtime 必须共享同一工具语义，只允许连接方式不同。
10. `currentVersionId` 只以 RuntimeStore 为权威；ArtifactStore 只保存 immutable bytes，
    不单独维护可与 RuntimeStore 漂移的 current 文件或软链接。
11. current CAS、`preview.updated` outbox 写入和 run terminal 状态必须属于同一 RuntimeStore
    事务；SSE 投递失败只能导致幂等补发，不能导致版本回退或重复 promotion。
12. Build、Review、Publish 或恢复失败时，产品端继续展示上一个 promoted Artifact；candidate
    错误不能把作品详情页切成空白或失效的 Pod URL。

## 3. 真实测试证据

### 3.1 环境

以下是 `2026-07-10T09:10:24Z` evidence bundle 记录的**故障复现基线**，不是当前工作树的
release evidence。该 bundle 未记录 Git commit、Runtime image digest 和 Sandbox image
digest，因此只能证明故障形态，不能证明当前 checkout 与集群镜像一致。

| 项目 | 结果 |
|---|---|
| k3d cluster | `zerondesign`, server `1/1` Ready |
| agent-sandbox controller | `v0.5.0`, deployment `1/1` Ready |
| SandboxWarmPool | `anydesign-astro-website-pool`, `1` Ready |
| Workspace PVC | `5Gi`, `Bound` |
| 仓库原生 K8s E2E | `1 passed` |
| Runtime sandbox tools | `82 passed`, `1 ignored` |

后续任何新 evidence bundle 必须额外记录：

```text
repositoryCommit
repositoryDirty
runtimeImageRef + runtimeImageDigest
sandboxImageRef + sandboxImageDigest
k3dClusterName + kubeContext
runtimeConfigHash
testDriverHash
providerMode（fixture | real，不记录 secret）
startedAt / completedAt
```

### 3.2 真实任务

输入：

- `/Users/carlos/Desktop/2026-Q2-Zenova-销售季度演讲稿.md`
- ElevenLabs Warm Editorial DesignProfile
- `MODEL_PROVIDER=deepseek`
- `DEEPSEEK_MODEL=deepseek-chat`
- `SANDBOX_BACKEND_MODE=kubernetes`

关键运行记录：

| 阶段 | 结果 |
|---|---|
| Brief | `run-5`, completed |
| Build | `run-40`, 创建真实 SandboxClaim |
| `project.init` | completed |
| 源码生成 | Pod 内生成约 23 KiB Astro/CSS 源码 |
| `style.update_tokens` | 宿主机 `CannotResolve(tokenFile)` |
| `project.ensure_dependencies` | 宿主机 `CannotResolve(appRoot)` |
| `preview.publish` | 宿主机 `CannotResolve(appRoot)` |
| Pod 内手工 `npm install` | 278 packages installed |
| Pod 内手工 `npm run build` | 1 static page built successfully |
| Pod Preview | HTTP 200，桌面和移动端可用 |

证据目录：

```text
.runtime-evidence/k3d-zenova-sales-20260710-171024
```

### 3.3 证据说明

这组结果把故障范围收敛到了 Harness：

- 模型链路可用：真实模型完成 Brief 和源码生成；
- Sandbox 控制面可用：Claim、WarmPool、Pod、PVC 均正常；
- Workspace Channel 可用：bootstrap、读写、目录遍历均发生在 Pod；
- 生成源码可构建：同一 PVC 内手工 production build 成功；
- Runtime 生命周期不可用：正式工具仍读取宿主文件系统，无法自动完成 build、preview
  和 promotion。

真实 DeepSeek 只证明 provider-backed Harness 可以执行本用例，不等于满足产品 R17 的
生产数据驻留和内部受控处理要求。生产环境必须使用批准的内部模型网关或经过数据合规
评审的 provider route；真实 provider evidence 不能替代数据安全验收，测试证据也不得
保存 API key、Authorization header 或完整敏感附件副本。

因此，之前“Phase A Runtime acceptance green”只能证明本地 Phase A Contract 和 K8s
Workspace Channel smoke 为绿色，不能证明 Public Runtime API 在 Kubernetes backend 下
完成了真实作品生命周期。修复完成前，不应把 Kubernetes Runtime 标记为 release-ready。

## 4. 根因分析

### 4.1 路径身份混淆

当前 `ToolContext.workspace_root` 同时承担：

- 权限边界；
- Runtime 宿主目录；
- Workspace Channel 路径映射基准；
- Sandbox 内工作目录。

在 `PhaseAContract + LocalWorkspaceBackend` 下，这些路径碰巧指向同一文件系统，所以测试
全部通过。在 `Kubernetes + SandboxChannelWorkspaceBackend` 下，只有逻辑路径相同，物理
文件系统并不相同。

典型错误包括：

- `check_existing_path()` 调用宿主机 `canonicalize()`；
- `Path::exists()` / `Path::is_dir()` 判断远端 build output；
- `fs::read_to_string(state/project.json)` 读取远端 lifecycle state；
- 宿主机启动 `python -m http.server` 直接服务 Pod 内 `dist`；
- staged write/read tracking 仍直接写宿主目录。

### 4.2 Backend 抽象只覆盖了部分操作

现有 `WorkspaceBackend` 已覆盖 read、write、list、stat、remove 和 copy，但工具内部仍有
大量本地文件调用。抽象存在，不代表调用边界已经完成迁移。

另外，Workspace Channel 当前以 UTF-8 text 为主，不能完整导出图片、字体、压缩包等
二进制 Artifact。即使把 HTML 同步到宿主机，也不能把这种文本同步方案当成正式发布
协议。

### 4.3 Preview 所有权不明确

当前 `PreviewStartTool` 默认在 Runtime 宿主机启动静态服务器，却把 `appRoot` 和 `dist`
视为 Sandbox 路径。Kubernetes 模式下应明确分为两类资源：

- Candidate Preview：在 Sandbox 内运行，但只服务本次 build 的只读 candidate snapshot；
- Promoted Artifact：从成功 build output 导出为不可变版本，由 Runtime/Object Storage
  服务。

这两个资源不能继续共用一个“宿主机目录 + localhost port”的隐式实现。

### 4.4 Desktop 连接是全局覆盖，不是 binding lease

`SANDBOX_CHANNEL_HOST_OVERRIDE` 和 `SANDBOX_CHANNEL_PORT_OVERRIDE` 是进程级全局值。
真实运行中必须先猜 WarmPool Pod，再手动建立 port-forward。Claim 切换到另一 Pod、并行
Project 或 Pod 重建后，该端口会指向错误 Workspace。

连接必须绑定 `sandboxBindingId`，而不是绑定 Runtime 进程。

### 4.5 网络策略与 local-e2e profile 未形成闭环

Runtime `local-e2e` 允许 public npm registry，但默认 Sandbox NetworkPolicy 只允许 DNS，
依赖安装仍无法访问 registry。本次测试必须临时增加 egress policy 才能完成安装。

标准 Kubernetes NetworkPolicy 不能按外部 registry FQDN 做稳定 allowlist，因此本方案不
再设计 public-registry egress overlay。Local E2E 在 `anydesign-runtime` namespace 部署
`anydesign-npm-proxy:4873`，Sandbox 只按 namespace/pod selector 访问该 Service；proxy
负责上游联网和缓存。Production 替换为公司内部 registry/proxy，但保持同一个 Sandbox
NetworkPolicy 和 `RUNTIME_NPM_REGISTRY` 语义。

### 4.6 错误分类导致无效重试

`CannotResolve` 在部分高层工具中以 `recoverable=false` 返回，但 Agent 仍会改用其他工具
反复尝试同一逻辑操作。缺少稳定 `errorKind`、retry fingerprint 和统一修复预算，会把
确定性的 Harness 缺陷变成模型循环。

## 5. 目标架构

```text
Public Runtime API
  -> Run / Project lifecycle state
  -> SandboxBinding
       -> SandboxChannelManager
            desktop: binding-scoped kubectl port-forward
            cluster: service DNS / mTLS
       -> WorkspaceBackend
            logical path policy
            stat/read/write/list/copy/archive
       -> SandboxCommandBackend
            install/build/preview process
       -> PreviewBackend
            start/status/stop/health
       -> ArtifactPublisher
            export build output
            immutable version storage
            /artifacts/{projectId}/{versionId}
  -> Review / Safety / Fidelity gates
  -> preview.updated
  -> run.completed
```

### 5.1 两类预览

#### Candidate Preview

- 在 Sandbox Pod 内启动；
- 只用于当前 run 的 Build/Review；
- 服务 `project.build` 生成的 `outputs/candidates/{buildId}` 只读副本，不直接服务可变
  `project/dist` 或 `project/out`；
- URL 由 Runtime Preview Proxy 提供，不把 Pod IP 直接暴露给产品端；
- 与 `sandboxBindingId + runId + buildId + candidateManifestHash` 绑定；
- Sandbox 重建后允许失效，不作为正式作品地址。

#### Promoted Artifact

- 只从成功 `project.build` 的静态输出导出；
- 导出时记录 source fingerprint、build id、文件清单和 SHA-256；
- 写入 Runtime storage 或内部 object storage；
- `/artifacts/{projectId}/current` 只解析到 promoted version；
- Sandbox、Preview 进程或 port-forward 停止后仍可访问。

这是推荐方案。不要把长期正式访问建立在 Pod preview port-forward 上。

## 6. 核心契约修改

### 6.1 显式逻辑路径

新增逻辑路径类型，禁止在工具边界上传递未标记的裸 `PathBuf`：

```rust
pub struct WorkspacePath(String);

impl WorkspacePath {
    pub fn parse(input: &str) -> Result<Self, WorkspacePathError>;
    pub fn join(&self, relative: &str) -> Result<Self, WorkspacePathError>;
    pub fn as_virtual_path(&self) -> &str;
}
```

规则：

- 对模型只暴露 `/workspace/...` 或 workspace-relative path；
- 拒绝 `..`、NUL、secret path 和 workspace 外绝对路径；
- Path Policy 只做词法边界和敏感路径判断；
- 是否存在、真实类型和 symlink 解析由实际 WorkspaceBackend 完成；
- 只有 `LocalWorkspaceBackend` 可以把逻辑路径解析为宿主机 `PathBuf`。

### 6.2 控制面与 Workspace 状态权威

远端读取 `state/project.json` 不代表把它提升为控制面 source of truth。权威矩阵固定为：

| 字段 | 权威来源 | Workspace 文件用途 |
|---|---|---|
| `sandboxBindingId` / Sandbox UID / Pod UID | RuntimeStore | 不允许 agent 写入或覆盖 |
| `currentVersionId` / expected current version | RuntimeStore | 只读展示 hint |
| `sourceSnapshotUri` / source fingerprint | RuntimeStore + build evidence | Workspace 保存本次 build 副本 |
| mutable run lock / owner epoch | RuntimeStore | 不写入 agent 可变文件 |
| latest successful build evidence | RuntimeStore | `outputs/build/latest.json` 是可诊断镜像 |
| promoted preview/artifact state | RuntimeStore | Workspace 不能触发 promotion |
| DesignProfile/policy/registry snapshot | immutable run snapshot | Workspace 只保存模型上下文副本 |
| `appRoot` / template / framework / package manager | `project.init` 结果写入 RuntimeStore | `state/project.json` 是 agent hint 和诊断副本 |

执行规则：

- `project.init` 成功后由专用工具同时写 Workspace hint 和 RuntimeStore project state；
- Build/Edit/Repair 开始时从 RuntimeStore 生成 immutable run snapshot，后续 cwd、template、
  registry 和 policy 都从该 snapshot 读取；
- `state/project.json` 与 RuntimeStore 不一致时返回 `project.state_conflict`，不得静默采用
  agent 文件；
- 通用 `fs.write/fs.patch/fs.delete` 禁止修改 `state/project.json`、build evidence、lease、
  current version 和 promotion state，只允许对应的 runtime-owned tool 写入；
- Runtime restart 后先恢复 RuntimeStore state，再验证 Workspace hint，不从 hint 反向重建
  binding、current pointer 或权限状态。

### 6.3 扩展 WorkspaceBackend

目标接口至少包括：

```rust
#[async_trait]
pub trait WorkspaceBackend {
    async fn stat(&self, ctx: &ToolContext, path: &WorkspacePath)
        -> Result<WorkspaceMetadata>;
    async fn read_bytes(&self, ctx: &ToolContext, path: &WorkspacePath)
        -> Result<Vec<u8>>;
    async fn write_bytes(&self, ctx: &ToolContext, path: &WorkspacePath, bytes: &[u8])
        -> Result<()>;
    async fn mkdir_all(&self, ctx: &ToolContext, path: &WorkspacePath)
        -> Result<()>;
    async fn list(&self, ctx: &ToolContext, path: &WorkspacePath)
        -> Result<Vec<WorkspaceEntry>>;
    async fn remove(&self, ctx: &ToolContext, path: &WorkspacePath, recursive: bool)
        -> Result<()>;
    async fn copy_tree(&self, ctx: &ToolContext, from: &WorkspacePath, to: &WorkspacePath,
        excludes: &[String]) -> Result<()>;
    async fn export_tree(&self, ctx: &ToolContext, path: &WorkspacePath,
        excludes: &[String], target: WorkspaceExportTarget)
        -> Result<WorkspaceExportReceipt>;
}
```

`read_to_string` 和 `write_string` 可以作为 bytes API 上的 UTF-8 helper 保留。

Workspace Channel response 必须返回结构化错误：

```json
{
  "ok": false,
  "error": {
    "code": "ENOENT",
    "kind": "not_found",
    "message": "path does not exist"
  }
}
```

不得依赖解析 Node 错误字符串判断 `NotFound`。

### 6.4 Workspace Channel 身份认证

Workspace Channel 的鉴权使用 Runtime 签发的短期 Ed25519 JWT，避免给所有 WarmPool Pod
分发可伪造 token 的共享对称密钥。

密钥与身份：

- Runtime 持有 signing private key，来自 Runtime Secret；
- Sandbox 只挂载 verification public key；
- SandboxTemplate 通过 Downward API 注入 `POD_NAME` 和 `POD_UID`；
- Runtime 在拿到 Ready binding、确认 Sandbox name 和 Pod UID 后才能签发 token；
- token TTL 最大 60 秒，只用于建立一次 Channel connection，不写入 checkpoint/evidence。

JWT claims：

```json
{
  "iss": "anydesign-runtime",
  "aud": "anydesign-workspace-channel",
  "exp": 1783670000,
  "jti": "channel-token-uuid",
  "sandboxBindingId": "binding-123",
  "sandboxName": "anydesign-astro-website-pool-abcde",
  "podUid": "kubernetes-pod-uid",
  "projectId": "project-123",
  "runId": "run-123",
  "operations": [
    "fs.read",
    "fs.write",
    "process.exec",
    "process.manage",
    "archive.export"
  ]
}
```

Server 在 WebSocket upgrade 前必须验证 signature、issuer、audience、expiry、sandboxName、
podUid 和 operation scope；不满足时返回 401/403，不能先升级再返回 tool error。

Capability 和防重放规则：

- operation 必须按具体动作授权，`fs`、`process`、`*` 这类宽泛 scope 不能进入 production；
- 每次连接签发新的 `jti`，Sandbox 在首次成功 upgrade 时原子消费，并在 `exp` 前保留
  replay-cache 记录；同一 `jti` 再次 upgrade 返回 401；
- 已建立连接只能执行 token 中声明的 operation，服务端逐帧校验 action -> operation 映射；
- token 过期后不强制中断正在执行的单个有界请求，但不能启动新 action；最长连接生命期为
  5 分钟，到期由 Runtime 使用新 token 重连；
- replay cache 只保存 `SHA-256(jti)`、expiry 和 Pod UID，不保存原 token；Sandbox 重启后旧
  Pod UID token 因 identity mismatch 自动失效；
- signing key 支持 `kid` 和双公钥轮换窗口，private key 只存在于 Runtime Secret。

传输要求：

- Desktop k3d：只连接 Runtime 管理的 `127.0.0.1` port-forward，并携带 JWT；
- Production：使用 `wss://`，mTLS/service-mesh identity 与 JWT 必须同时通过；
- NetworkPolicy 只是纵深防御，不能替代应用层鉴权；
- JWT、Authorization header、private key 和完整 claim 不进入 SSE、模型上下文或 audit；audit
  只记录 `jtiHashPrefix`、`kid`、binding、Pod UID、operation 和 allow/deny；
- Channel auth failure 使用 `workspace.channel_unauthorized`，禁止模型自动重试，先由
  Runtime 重建 binding/lease。

### 6.5 PreviewBackend

```rust
#[async_trait]
pub trait PreviewBackend {
    async fn start(&self, ctx: &ToolContext, request: PreviewStartRequest)
        -> Result<PreviewLease>;
    async fn status(&self, ctx: &ToolContext, lease_id: &str)
        -> Result<PreviewStatus>;
    async fn stop(&self, ctx: &ToolContext, lease_id: &str)
        -> Result<()>;
}
```

`PreviewLease` 必须记录：

- `leaseId`
- `sandboxBindingId`
- `runId`
- `buildId`
- `sandboxPort`
- Runtime proxy URL
- process handle 或 pid metadata
- `startedAt` / `expiresAt`

MVP Preview transport 固定为：

1. 所有模板 build 后都使用镜像内 runtime-owned
   `/opt/anydesign/bootstrap/static-preview-server.js` 服务 candidate snapshot，并统一监听
   Sandbox `4321`；不调用项目依赖中的 Astro/Next/`serve` preview command；
2. Workspace Channel 增加 runtime-owned `process.start/status/stop`，使用 process lease
   而不是让 `process.exec` 永久等待；
3. SandboxTemplate Service 显式暴露 `workspace:3001` 和 `preview:4321`；
4. Runtime 提供唯一浏览器入口 `/previews/{leaseId}/{*path}`；
5. Desktop Runtime 为该 PreviewLease 创建 `pod/<podName> localPort:4321` port-forward；
6. In-cluster Runtime 使用 `http://<sandbox-service>.<namespace>.svc:4321`；
7. Runtime proxy 校验 Project/Run 权限和 lease target 后转发 HTTP/WebSocket；
8. `browser.open/screenshot/inspect` 只能访问 Runtime proxy URL，不能访问模型提供的
   localhost、Pod IP、Service DNS 或任意内部 URL。

`project.build` 成功后必须先把静态输出复制到
`outputs/candidates/{buildId}`、计算 deterministic manifest 并将 run 转入 `validating`。
从该状态开始拒绝 agent mutation tools，PreviewBackend 和 ArtifactPublisher 都只能读取这
个 candidate snapshot；两者 manifest hash 不一致时返回 `artifact.candidate_mismatch`，
禁止进入 gate 或 CAS promotion。

截图由 Runtime-owned browser worker 对 proxy URL 执行并保存真实 bitmap 与 viewport
metadata；模型提交的 `blank`、URL 或 screenshot status 不能作为 review evidence。
Local backend 可以继续在宿主机启动 preview，但必须实现同一 PreviewLease 和 proxy route
契约，避免两种 backend 对上层暴露不同语义。

Deterministic candidate manifest 固定规则：

- 路径使用 UTF-8、NFC、workspace-relative POSIX separator，并按 byte order 稳定排序；
- 拒绝绝对路径、`..`、NUL、symlink、device、hardlink 和大小写折叠后冲突的两个路径；
- entry 至少包含 `path/type/size/sha256`，目录不参与 bytes hash；
- manifest 不包含 mtime、uid/gid、宿主绝对路径或随机字段；
- `candidateManifestHash = SHA-256(canonical JSON bytes)`；Preview、Stage、Gate、Promote
  每一步都必须携带并比对该值，不能重新解释可变 `dist/out`。

Preview Proxy 的请求身份固定复用 Public Runtime API 的 authenticated principal：

- principal 必须拥有 `projectId`，并且 lease 的 `projectId/runId` 与请求资源一致；
- `leaseId` 是 256-bit capability nonce，用于不可枚举和路由，不替代 principal 鉴权；
- local E2E 可在显式 `AUTH_MODE=test` 下使用 fixture principal，production 禁止 anonymous；
- HTML 中的绝对资源路径只能重写到同一 lease prefix，redirect `Location` 也必须同源；
- proxy 删除上游 cookie、认证 header 和内部 hop-by-hop header，并添加
  `X-Content-Type-Options: nosniff`、限制性 CSP 与 `Referrer-Policy: no-referrer`；
- static preview server 禁止目录列表、dotfile、symlink escape、任意 root 参数和写操作。

### 6.6 ArtifactPublisher

```rust
#[async_trait]
pub trait ArtifactPublisher {
    async fn stage(
        &self,
        ctx: &ToolContext,
        build: &BuildEvidence,
        candidate_output: &WorkspacePath,
        idempotency_key: &str,
    ) -> Result<StagedArtifact>;
    async fn promote(
        &self,
        staged: &StagedArtifact,
        expected_current_version_id: Option<&str>,
    ) -> Result<PublishedArtifact>;
    async fn abort(&self, staged_artifact_id: &str) -> Result<()>;
}
```

Artifact transaction 固定为两阶段：

```text
project.build success
  -> ArtifactPublisher.stage(idempotencyKey = projectId/runId/buildId)
       immutable bytes + manifest
       state = staged
       not addressable by /artifacts/{projectId}/current
  -> candidate preview + screenshot
  -> Review / Safety / DesignProfile Fidelity gates
  -> ArtifactPublisher.promote(expectedCurrentVersionId)
       compare-and-swap current pointer
       state = promoted
  -> preview.updated
  -> run.completed
```

RuntimeStore 与 ArtifactStore 的职责固定为：

| 数据 | 权威存储 | 原子性要求 |
|---|---|---|
| immutable artifact bytes + manifest | ArtifactStore | 同一 `versionId` 内容不可覆盖 |
| `ArtifactPublishRecord` | RuntimeStore | 状态迁移使用 revision/CAS |
| `currentVersionId` | RuntimeStore | 与 `preview.updated` outbox、run 状态同一事务 |
| `/artifacts/{projectId}/current` | Runtime HTTP | 每次从 RuntimeStore 解析 version，再读 immutable ArtifactStore |
| source snapshot manifest + bytes | Runtime-owned SnapshotStore | immutable、按 project/build 隔离 |

禁止 ArtifactStore 额外维护 `current` 文件、symlink 或 mutable object pointer。否则
RuntimeStore CAS 成功与对象存储 current 更新失败之间会产生双权威。

Stage 过程必须：

1. 验证 build success 且 candidate output/manifest 属于本次 build；
2. 通过 WorkspaceBackend 导出 tar/archive，不能由宿主机遍历 PVC；
3. 防止 archive path traversal、symlink escape 和超限文件；
4. 计算 manifest 和整体 hash；
5. 原子写入 immutable version storage；
6. 使用 `projectId/runId/buildId` 幂等键，相同 bytes 返回同一个 staged artifact，不重复写；
7. staged version 只能供内部 gate 读取，不能更新 current pointer。

Promote 过程必须：

- 验证 staged artifact、PreviewLease、candidate manifest、build evidence、source
  fingerprint 和 run 一致；
- 以 `expectedCurrentVersionId` 执行 compare-and-swap，防止并发 run 覆盖新版本；
- CAS 成功后才提交 promoted state 和 current pointer；
- current pointer 提交成功后才发 `preview.updated`；
- 重复 promote 同一个 idempotency key 返回原结果；
- gate 失败调用 `abort` 或把 staged artifact 标记为 `garbage_collectable`，不改变 current；
- staged/publishing 状态写入 RuntimeStore，供 Runtime restart reconciliation 使用。

`ArtifactPublishRecord` 至少包含：

```text
publishId / idempotencyKey
projectId / runId / buildId / versionId
sandboxBindingId / podUid
candidateManifestHash / artifactManifestHash / sourceSnapshotUri
expectedCurrentVersionId
state / revision
stagingHandle / immutableArtifactUri
createdAt / updatedAt / lastError / gcAfter
```

current CAS 成功时，RuntimeStore 必须在同一事务内：

1. 把 publish record 从 `promoting` 更新为 `promoted`；
2. 更新 Project 的 `currentVersionId`；
3. 插入唯一键为 `preview.updated:{projectId}:{versionId}` 的 outbox event；
4. 把 run 更新到允许完成的 `promotion_committed` 状态。

SSE dispatcher 只从 outbox 投递事件；投递后记录 ack/delivery metadata。消费者仍按 event ID
去重，因此语义是 at-least-once delivery + idempotent consumption，而不是不可证明的
exactly-once。`run.completed(success)` 只能在 `preview.updated` 已进入 outbox 后产生，并保持
SSE 排序；Runtime crash 后按 outbox sequence 补发。

状态机：

```text
staging -> staged -> validating -> promoting -> promoted
                      |              |
                      v              v
                    failed       reconcile_required
                      |
                      v
              garbage_collectable
```

传输约束：

- MVP 使用 Workspace Channel binary frames 向 Runtime-owned staging sink 流式传输；
- `WorkspaceExportReceipt` 只包含 manifest、size、hash 和 staging handle，不持有完整 bytes；
- 禁止把完整 tar 转成 base64 后塞入单个 JSON Workspace Channel message；
- producer 和 consumer 必须有 backpressure、checksum 和中断清理；
- MVP 默认配额建议为 archive 256 MiB、单文件 64 MiB、20,000 files，最终值由运行环境
  quota 确认并做成配置；
- 超限返回 `artifact.limit_exceeded`，不能截断后继续 promotion；
- 临时 archive 使用 run-scoped 随机路径，publish/cancel 后必须清理。

Source snapshot 不使用 `file:///workspace/...`：

- build 完成时把源码和二进制资源写入
  `runtime://source-snapshots/{projectId}/{buildId}` 对应的 immutable manifest；
- snapshot manifest 记录相对路径、size、mode、SHA-256，拒绝 symlink 和 workspace escape；
- Edit/Repair 恢复时由 Runtime 读取 SnapshotStore，并通过当前 binding 的
  `WorkspaceBackend.write_bytes` 写回，不把 Runtime host path 暴露给 tool/model；
- 恢复后重新计算 source fingerprint，不一致返回 `workspace.snapshot_integrity_failed`；
- snapshot retention 至少覆盖所有仍可回滚的 promoted version，GC 不能只按 run terminal
  删除。

### 6.6.1 Crash-point 决策表

| Crash point | 重启后的判定 | 必须结果 |
|---|---|---|
| staging bytes 未完成 | staging handle 无完整 manifest | abort 临时对象，current 不变 |
| bytes 完成、record 仍 `staging` | manifest/hash 可验证 | 幂等转 `staged`，否则 abort |
| gate 失败前后 | record 为 `staged/validating` 且有 blocking finding | 标记 `garbage_collectable`，current 不变 |
| immutable version 已写、CAS 未执行 | record 为 `promoting`，current 仍 expected | 重放 CAS；冲突则保留 orphan 等待 GC |
| CAS 已成功、进程未发 SSE | current 指向 version，outbox 未 ack | 补发同一 event ID，不重复 version |
| SSE 已发、run 未完成 | outbox 已 ack，run 非 terminal | 幂等完成 run |
| expected current 已被其他 run 更新 | CAS conflict | 当前 run 返回 `artifact.current_version_conflict`，不得覆盖新版本 |

Candidate Preview Proxy 的安全约束：

- 入口必须经过 Project/Run 权限检查；
- proxy target 只能来自有效 `PreviewLease`，不能接受用户提交的任意 URL；
- target 必须与 lease 的 SandboxBinding 和允许端口一致，防止 SSRF；
- 不向浏览器暴露 Pod IP、Service account token、内部 DNS 或原始 preview cookie；
- lease 过期、run terminal、binding 变化后立即拒绝访问。

### 6.7 SandboxChannelManager

Desktop + k3d 需要 Runtime 管理连接，而不是依赖人工端口转发：

```rust
#[async_trait]
pub trait SandboxChannelManager {
    async fn acquire(&self, binding: &SandboxBinding) -> Result<ChannelLease>;
    async fn release(&self, lease_id: &str) -> Result<()>;
    async fn reconcile(&self, runtime_epoch: &str) -> Result<ReconcileReport>;
}
```

约束：

- 一个 lease 只绑定一个 `sandboxBindingId`；
- Runtime 发现 Sandbox Pod 变化时重建 lease；
- 并行项目不能共享全局 channel port；
- Runtime shutdown、run terminal 和 claim release 都必须清理子进程；
- Production in-cluster 实现直接使用 Service DNS，不启动 `kubectl`。

RuntimeStore 持久化 `ChannelLeaseRecord`、`PreviewLeaseRecord` 和
`ArtifactPublishRecord`。每条 lease 至少包含：

```text
leaseId
ownerRuntimeEpoch
sandboxBindingId
sandboxUid
podUid
projectId
runId
transport = port_forward | service_dns
localPort / serviceEndpoint
childPid + childStartTime（仅 desktop）
state = acquiring | ready | stale | releasing | released | failed
createdAt / heartbeatAt / expiresAt
```

Runtime 每次启动生成新的 `runtimeEpoch`，在接受 mutable run 前执行 reconcile：

1. 标记旧 epoch 的 desktop port-forward lease 为 stale；
2. 只在 pid 和 childStartTime 同时匹配时终止 orphan child，避免误杀复用 PID；
3. 查询 Kubernetes binding、Sandbox UID 和 Pod UID；
4. binding 仍有效时按新 Pod 重建 ChannelLease；binding 已失效时把 run 转入 recovery；
5. 通过 `process.status` 查询 PreviewLease，停止不属于 active run/build 的 preview；
6. 对 staged/promoting Artifact 执行幂等完成、回滚或标记 GC；
7. reconcile report 写入 audit，完成前 `/runs` 的 mutable phase 返回 `503 recovering`。

lease acquire/release、preview start/stop 和 artifact stage/promote 都必须可幂等重放。仅依赖
`Drop`、signal handler 或正常 terminal callback 的 cleanup 不计为恢复实现。

### 6.8 资源所有权与公开路由

| 资源 | Owner | 生命周期 | 对产品公开的路由 |
|---|---|---|---|
| source workspace / dependencies | Sandbox PVC | binding/run | 不公开 |
| candidate snapshot | Sandbox PVC，只读 | build/run | 不直接公开 |
| Preview process | PreviewBackend + Sandbox | PreviewLease | 不直接公开 Pod/Service URL |
| PreviewLease | RuntimeStore | validating/run | `/previews/{leaseId}/{*path}` |
| staged Artifact | ArtifactPublisher | publish transaction | 不公开 |
| promoted Artifact bytes | ArtifactStore | version retention | `/artifacts/{projectId}/{versionId}/{*path}` |
| current pointer | RuntimeStore | project | `/artifacts/{projectId}/current/{*path}` |
| source snapshot | SnapshotStore | promoted version retention | 不公开；Edit/Repair 内部使用 |
| Channel/port-forward process | SandboxChannelManager | binding/runtime epoch | 不公开 |

现有 Runtime 的 `GET /preview/{projectId}/current` 继续作为 metadata resolver，返回
`versionId/status/previewUrl/screenshotId`，其中 `previewUrl` 必须指向
`/artifacts/{projectId}/current`。早期产品架构文档中“该路由直接转发 Sandbox preview”的
描述被本方案取代；产品端收藏/iframe 使用返回的稳定 Artifact URL。不得按 `Accept` header
让同一路由有时返回 JSON、有时返回 HTML。

实际正式内容统一由 `/artifacts/{projectId}/current/{*path}` 提供。精确历史版本补充
`/artifacts/{projectId}/{versionId}/{*path}`；Candidate 与 promoted 两套路由、权限和
生命周期不得互相 fallback。该路由迁移需同步更新产品架构和 Runtime API freeze 文档，
不能只改服务端。

## 7. 工具迁移清单

以下工具必须完成 backend-aware 改造：

| 工具/模块 | 当前问题 | 修复方式 |
|---|---|---|
| `ToolContext` / `ToolExecutor` | `workspace_root: PathBuf` 混合逻辑和宿主路径 | 注入 `WorkspaceScope + WorkspaceBackend`，HostPath 只存在于 Local backend |
| Permission Engine | `canonicalize/exists` 依赖宿主文件 | 只做 WorkspacePath 词法策略，远端存在性由 backend `stat` |
| AgentLoop bootstrap/context/health | context 和 health 仍直接写宿主目录 | 全部通过 runtime-owned workspace tools/backend |
| `fs.*` | 部分 permission/read tracking 仍落宿主机 | Path Policy + WorkspaceBackend |
| `project.init` | 远端 parent 和 token file 曾使用本地假设 | `mkdir_all/stat/read/write` 全走 backend |
| `project.inspect` | project state、lockfile 探测使用宿主 `fs` | async backend state reader |
| `style.update_tokens` | `tokenFile` 调用本地 `canonicalize` | backend `stat + read + write` |
| `project.ensure_dependencies` | cwd 调用本地 `check_existing_path` | 词法校验后由 backend `stat`，命令在 Pod 执行 |
| `package.install` | local-e2e registry 与 NetworkPolicy 不一致 | profile-aware egress + audit |
| `project.build` | cwd、dist 检测、project state 存在本地调用 | backend state/stat + command backend |
| source fingerprint | 必须完整遍历远端源码 | backend list/read bytes，稳定排序和 hash |
| source snapshot | 不能生成宿主机 file URI | backend copy/archive + immutable snapshot URI |
| `preview.start` | 宿主机服务远端 `dist` | PreviewBackend 在 Sandbox 启动 |
| `browser.*` | localhost 指向 Runtime 而非 Sandbox | Runtime preview proxy URL |
| `preview.publish` | 混合 build、host preview、promotion | build -> candidate lease -> artifact publish -> gate |
| runtime-state API | 本地 fallback 可能掩盖远端缺失 | 明确 state source 和 freshness |
| artifact HTTP routes | 从 workspace `dist/out` 直接读宿主文件 | 只读取 promoted ArtifactStore/current pointer |
| oversized tool results/staged writes | 直接写 `workspace_root/outputs` | 按所有权拆为 WorkspaceBackend 或 RuntimeStorage |

硬规则：Kubernetes backend 的工具路径中不得出现以下调用，除非操作对象明确是
Runtime-owned storage：

```text
std::fs::canonicalize
Path::exists
Path::is_dir
std::fs::read_to_string
std::fs::write
```

P0 增加 source scan gate：扫描 `agent_loop.rs`、`permission.rs`、`tools/runtime.rs`、
`tools/sandbox.rs` 和 `http_api.rs`。任何 Kubernetes execution path 新增上述调用必须失败；
允许项必须位于显式 `RuntimeStorage`/`LocalWorkspaceBackend` 模块，并带 allowlist reason。

## 8. 分阶段实施计划

阶段按顺序推进；后一阶段可以开发，但不能在前一阶段 exit gate 未通过时合并到 release
分支。每个阶段都必须保留上一个 promoted Artifact 的读取路径。

| 阶段 | 主责 | Entry gate | Exit gate | 回滚边界 |
|---|---|---|---|---|
| P0 Remote Workspace | Runtime Core + Sandbox Infra | 现有 Local contract tests green | authenticated Channel 上 init/style/install/build green | feature flag 回到 Local backend；不回滚 RuntimeStore schema |
| P1 Candidate Preview | Runtime Core | P0 build evidence + frozen candidate | authenticated proxy + screenshot + cleanup green | 关闭 remote preview，不影响 current Artifact |
| P2 Artifact Publish | Runtime Core + Storage | P1 candidate manifest 可验证 | stage/gate/CAS/outbox/recovery green | 停止新 promotion，继续读取旧 immutable version |
| P3 Connection/Network | Sandbox Infra + Security | P0-P2 单项目链路可用 | epoch reconcile、npm proxy、双项目隔离 green | 禁用自动 tunnel acquire，保留诊断 override；production 不放宽 egress |
| P4 Release Gate | Harness QA | P0-P3 全部 exit | Website + Docs public API evidence 完整 | release blocked，不降级为手工 `kubectl exec` |

跨阶段 schema 只允许 additive migration。RuntimeStore 新字段必须能读取旧 record，并通过
startup migration/reconcile 补齐；不得依赖删除本地 storage 才能升级。

### 8.1 当前实现快照

以下只用于安排后续工作，来自 `2026-07-10` 的未提交工作树检查，**不是 exit gate**：

| 阶段 | 已进入工作树 | 尚未关闭的阻断项 | 状态 |
|---|---|---|---|
| P0 | Channel JWT/Pod identity、RuntimeStore project state、remote backend read/write、candidate manifest/freeze | granular scope + replay cache、完整 `WorkspacePath` 类型、retry budget、source scan gate、全量测试 | In progress |
| P1 | runtime-owned static server、固定 4321、PreviewLease persistence、Runtime proxy | process lease、binding-scoped preview tunnel、真实 browser bitmap worker、stale Pod reconcile、完整 proxy auth | In progress |
| P2 | File ArtifactPublisher、immutable version route、manifest check、current CAS、Runtime source snapshot URI | persisted publish state、abort/GC、transactional outbox、binary streaming/backpressure、crash recovery | In progress |
| P3 | 尚无可作为 exit evidence 的完整实现 | ChannelManager epoch reconcile、npm proxy、NetworkPolicy、双项目隔离 | Not started |
| P4 | 旧 smoke/evidence 可用于复现 | 当前 checkout 镜像部署、Website + Docs Public API release evidence | Blocked by P0-P3 |

每次合并对应 commit 后更新本表；只有第 13 节全部满足，front matter 才能改成
`release-ready`。

### P0. 关闭远端路径语义缺口

目标：让 `project.init -> style.update_tokens -> ensure_dependencies -> project.build`
完整在 Workspace Channel 上执行。

工作项：

- Runtime Ed25519 signing key、Sandbox verification key、Downward API Pod identity 和
  short-lived Channel JWT 生效；
- granular operation scope、one-shot `jti` replay cache 和 key rotation 生效；
- 实现最小 binding-scoped Channel resolver：每次操作从 RuntimeStore binding 解析当前
  Sandbox/Pod UID，动态建立连接；P0 不要求持久化 child process 或 startup epoch reconcile；
- 引入 `WorkspacePath` 和 backend-aware `stat`；
- 移除上述工具中的宿主机存在性检查；
- Workspace Channel 支持结构化 error code 和递归 parent creation；
- project state、package manager、lockfile 和 dist 检测改为 async backend 读取；
- `project.build` 原子创建 `outputs/candidates/{buildId}` 和 deterministic manifest，成功后
  run 进入 `validating` 并冻结 agent mutation tools；
- `ToolContext` 不再把远端 workspace 表示为可供任意模块使用的宿主 `PathBuf`；
- `AgentLoop` context/health、Permission Engine、default cwd、staged writes 和 runtime-state
  fallback 一起迁移，不能只修四个高层工具；
- RuntimeStore/run snapshot 权威规则和 `state/project.json` 写保护生效；
- 同一 deterministic error 增加 retry fingerprint 和 3 次上限。

验收：

- 无 token、过期 token、错误 audience、错误 Pod UID 和越权 operation 均在 WebSocket
  upgrade 前被拒绝；
- 同一 `jti` 第二次 upgrade 被拒绝，token/claim 不进入 SSE、模型上下文或日志；
- Host workspace 下不存在 `project/`，Pod PVC 中存在时，所有工具仍能成功；
- `style.update_tokens` 修改的是 Pod 内 token CSS；
- `project.ensure_dependencies` 和 `project.build` 的 cwd 均为 `/workspace/project`；
- Runtime 产生成功 build evidence，不允许手工 `kubectl exec npm run build` 作为替代。
- candidate snapshot 与 build evidence manifest hash 一致，冻结后 mutation tool 被拒绝；
- P0 source scan gate 通过，Kubernetes execution path 没有未登记的 host `fs::*` 调用。

P0 回滚时允许关闭 Kubernetes backend 的新 run，但不得让已创建的 Kubernetes run 静默
切换到宿主 Local workspace；它们必须进入 `blocked` 并保留 recovery metadata。

### P1. Candidate Preview 归 Sandbox

目标：让 `preview.start/status/stop` 在 Kubernetes backend 下拥有明确执行位置。

工作项：

- 新增 `PreviewBackend`；
- Workspace Channel 新增带 lease 的 `process.start/status/stop`；
- 使用 runtime-owned static preview server 服务 candidate snapshot，并固定监听 `4321`；
- Sandbox Service 和 NetworkPolicy 显式允许 Runtime -> preview `4321`；
- 为 preview process 增加 health check、timeout、log 和 cleanup；
- Runtime 实现 `/previews/{leaseId}/{*path}` 鉴权 proxy；
- Desktop proxy target 使用 binding-scoped port-forward；
- Production proxy target 使用 cluster Service DNS + workload identity；
- Browser worker 只接收 Runtime proxy URL 并产生真实 bitmap evidence。

验收：

- Candidate URL 可访问；
- Preview lease 与 run/build/binding 一致；
- Astro 与 Fumadocs 都使用同一个 `4321` port contract；
- 未授权用户、过期 lease 和伪造 target 均不能通过 Preview Proxy；
- Pod 重建后旧 lease 失效并返回稳定 `preview.lease_stale`；
- run cancel/terminal 后进程和 tunnel 被清理。

P1 不允许 promotion 依赖 Candidate Preview 持续存活。关闭 P1 feature flag 后，现有
promoted Artifact 和 `/preview/{projectId}/current` metadata 必须继续可读。

### P2. Promoted Artifact 脱离 Sandbox

目标：正式作品不依赖长生命周期 Pod。

工作项：

- Workspace Channel 增加 binary export stream；
- 新增 `ArtifactPublisher.stage/promote/abort` 和 artifact manifest；
- 从 Website `dist/` 或 Docs `out/` 生成 candidate snapshot，并只发布该 snapshot；
- `/artifacts/{projectId}/current` 解析 promoted immutable version；
- source snapshot 从 `file:///workspace/...` 迁移到 Runtime-owned URI；
- stage 使用 `projectId/runId/buildId` 幂等键；
- gates 只读取 staged candidate，promotion 使用 expected current version 做 CAS；
- startup reconciliation 处理 staging/promoting 中断记录。

验收：

- 停止 Preview、删除 SandboxClaim 后正式 Artifact 仍返回 HTTP 200；
- 图片、字体和其他二进制资产 hash 正确且可访问；
- 失败 publish 不改变 current version；
- 同一 build 重试 stage/promote 不产生重复 version 或重复 `preview.updated`；
- Runtime 在 stage 或 CAS 中途崩溃后可以幂等完成或回滚；
- `preview.updated` 只在原子 publish 和 gate 通过后发出。

P2 的 artifact-only reconciliation 只处理 publish record 和 outbox，不依赖 P3 的 tunnel
epoch。P3 完成后由统一 startup recovery gate 编排 artifact、channel 和 preview reconcile。

### P3. k3d 连接与网络闭环

目标：本地真实 E2E 不再需要人工猜 Pod、手动 port-forward 或临时改策略。

工作项：

- 实现持久化、epoch-aware 的 `SandboxChannelManager`；
- 将进程级 host/port override 降级为调试逃生口；
- 在 `anydesign-runtime` namespace 部署 `anydesign-npm-proxy` Service；
- Sandbox NetworkPolicy 只允许到 npm proxy Pod selector 的 TCP `4873`；
- local-e2e 与 production 都只向 Sandbox 下发内部 registry URL，不按公网 FQDN 配置
  标准 NetworkPolicy；
- 测试结束自动释放 claim、preview、channel tunnel 和 staged artifact；
- 支持两个项目并行 claim，证明 channel 不串 Workspace。
- 删除 release/E2E 对进程级 `SANDBOX_CHANNEL_HOST_OVERRIDE` 和固定本地端口的依赖；该
  override 只允许在显式 debug profile 使用，并在 evidence 标为 non-release。

验收：

- 一条仓库命令完成 cluster setup、image import、Runtime 启动和真实任务；
- 不需要人工读取 WarmPool Pod 名；
- Sandbox 无公网 egress 也能通过 npm proxy 安装依赖；
- 两个并发项目的文件、preview 和 artifact 不串线；
- 默认 production policy 下公网 registry 仍被拒绝。

k3d runner 必须新建或显式选择专用集群，不能复用无法证明来源的 `k3d-zerondesign` 环境。镜像
使用 `${component}:${gitSha}`，执行 `k3d image import` 后等待 rollout，并断言 Pod spec image、
container `imageID` 和 evidence 中记录的 digest 一致；任一不一致立即失败，不能继续跑任务。

### P4. Release Gate

目标：把真实 Kubernetes 生命周期变成必须通过的发布证据。

新增 gate：

```text
Brief confirm
  -> Build run
  -> SandboxClaim Ready
  -> authenticated ChannelLease Ready
  -> project.init
  -> DesignProfile context read
  -> dependencies restored
  -> project.build success
  -> artifact staged with build idempotency key
  -> candidate preview accessible
  -> screenshot nonblank
  -> Review / Safety / Fidelity gates pass
  -> artifact promoted with current-version CAS
  -> preview.updated
  -> run.completed
  -> artifact remains accessible after sandbox release
```

Website 和 Docs 必须分别执行；真实 provider gate 可以显式启用，默认 PR gate 使用确定性
provider fixture，但两者必须复用同一个 Public Runtime API driver。

Gate 分层固定为：

- PR required：fixture provider，Website + Docs，全生命周期、auth、crash、concurrency；
- nightly：真实 provider smoke，可因 provider 限流标记 infrastructure failure，但不能伪装成功；
- release candidate required：真实批准 provider 的 Website + Docs，全部 exit criteria 和
  evidence integrity 通过；
- manual debug：允许 override/手工 tunnel，但其结果永远不能作为 release evidence。

每次 P4 必须产出机器可校验的 `evidence-summary.json`，至少交叉验证：

```text
repository.commit / repository.dirty
cluster.name / kubeContext
runtime.imageRef / runtime.imageDigest / runtime.reportedCommit
sandbox.imageRef / sandbox.imageDigest
projectId / runId / sandboxBindingId / podUid
buildId / candidateManifestHash / sourceSnapshotUri
previewLeaseId / screenshotId / nonblankPixelRatio
versionId / artifactManifestHash / artifactUrl
preview.updated.eventId / run.completed.eventId / event sequence
currentVersionId before/after CAS
sandboxReleasedAt / artifactHttpStatusAfterRelease
provider.mode / provider.model（不记录 credential）
```

runner 对缺失、空字符串、跨接口不一致或时间顺序错误一律失败。evidence 打包前运行 secret
scan；原始 API key、Authorization、JWT、完整内部附件和 provider request body 不得进入 bundle。

## 9. 文件级改造建议

### Runtime

- `services/runtime/src/tools/runtime.rs`
  - `ToolContext` 拆分 WorkspaceScope、RuntimeStorage 和 immutable run snapshot。
- `services/runtime/src/permission.rs`
  - 远端路径只做词法策略，不调用宿主 canonicalize/stat。
- `services/runtime/src/agent_loop.rs`
  - context/health 通过 backend，retry fingerprint，启动恢复门禁。
- `services/runtime/src/tools/sandbox.rs`
  - 先完成 P0；随后拆出 workspace、project、preview 和 artifact 职责。
- `services/runtime/src/sandbox_adapter.rs`
  - binding-aware channel discovery 和 lease metadata。
- `services/runtime/src/http_api.rs`
  - authenticated preview proxy、ArtifactStore serving、current CAS 和 runtime-state source 标记。
- `services/runtime/src/config.rs`
  - JWT signing key、local/production channel manager、preview port 和 registry proxy 配置。
- 建议新增：
  - `services/runtime/src/workspace.rs`
  - `services/runtime/src/preview_backend.rs`
  - `services/runtime/src/artifact_publisher.rs`
  - `services/runtime/src/sandbox_channel_manager.rs`

### Sandbox Image

- `infra/agent-sandbox/base/workspace-channel-server.js`
  - JWT verification、structured errors、binary stream、process lease、mkdir 和 symlink guard。
- 建议新增 `infra/agent-sandbox/base/static-preview-server.js`
  - 只读服务指定 candidate root，支持 index/HTML fallback、content type、缓存头和路径逃逸
    防护；不执行项目代码、不接受任意 root。
- `infra/agent-sandbox/astro-website/sandbox-template.yaml`
  - Downward API Pod identity、verification public key、candidate preview `4321` port contract。
- `infra/agent-sandbox/network/default-deny.yaml`
  - 保持 default deny，增加 Runtime -> `4321` ingress 和 Sandbox -> npm proxy `4873` egress。
- 建议新增：
  - `infra/agent-sandbox/npm-proxy/deployment.yaml`
  - `infra/agent-sandbox/npm-proxy/service.yaml`
  - `infra/agent-sandbox/base/workspace-channel-verification-key.yaml` 的部署模板；真实 key
    由环境 Secret/Config 管理生成，不提交 private key。

### Tests / Harness

- `services/runtime/tests/sandbox_tools.rs`
  - host path absent + remote path present 的工具矩阵。
- `services/runtime/tests/k8s_sandbox_e2e.rs`
  - auth deny/allow、binary file、nested mkdir、stream export 和 process lifecycle。
- 建议新增：
  - `services/runtime/tests/k8s_runtime_lifecycle_e2e.rs`
  - `services/runtime/scripts/run-k3d-runtime-lifecycle-e2e.sh`

P0 source scan 必须覆盖上述全部 Runtime 文件；只修改 `sandbox.rs` 不视为 P0 完成。

## 10. 测试矩阵

| 层级 | 场景 | 必须断言 |
|---|---|---|
| Unit | WorkspacePath parser | traversal/secret/external path 拒绝 |
| Unit | error mapping | ENOENT/EACCES/EEXIST 保留语义 |
| Unit | Channel JWT | expired/wrong audience/wrong Pod UID/wrong operation 被拒绝 |
| Unit | Channel JWT replay | 同一 `jti` 第二次 upgrade 被拒绝，日志无原 token |
| Unit | state authority | agent hint 不能覆盖 binding/current/registry/build evidence |
| Unit | publish transaction | current CAS、publish record、outbox 同事务提交或全部回滚 |
| Backend contract | Local 与 Channel 同一 fixture | read/write/stat/mkdir/copy/export 行为一致 |
| Tool integration | host path 缺失、remote path 存在 | init/style/install/build 全通过 |
| Candidate freeze | build 后继续调用 mutation tool | typed deny，snapshot/manifest 不变 |
| K8s smoke | claim + PVC + authenticated channel | text/binary/process/export 通过 |
| Preview E2E | Pod candidate preview | 4321 health、proxy auth、SSRF deny、cleanup 通过 |
| Artifact E2E | stage/gate/CAS 后删除 claim | artifact 仍 HTTP 200 |
| Artifact mismatch | PreviewLease 与 staged manifest hash 不同 | gate/promotion 被拒绝 |
| Artifact crash | stage/CAS 各 crash point | restart 后幂等完成或回滚，无重复 event |
| SSE recovery | CAS 后 dispatcher 前 crash | 相同 event ID 补发，顺序仍早于 `run.completed` |
| Snapshot restore | 文本 + 图片 + 字体 | binary hash 不变，恢复后 source fingerprint 一致 |
| Concurrency | 两个 project 同时 build | binding、PVC、port、artifact 不串线 |
| Security | production network | direct public npm denied，npm proxy Service allowed |
| Recovery | Runtime/port-forward/Pod 重启 | epoch reconcile、stale lease 重建、orphan process 清理 |
| Real provider | Website + DesignProfile | Public API 成功完成并 promotion |
| Real provider | Docs + DesignProfile | Docs navigation/artifact/edit 闭环 |
| Image parity | 当前 checkout 部署到 k3d | Git SHA、Pod image ref、container imageID、evidence digest 一致 |

本地核心命令：

```bash
cargo fmt --manifest-path services/runtime/Cargo.toml -- --check
cargo test --manifest-path services/runtime/Cargo.toml
bash infra/agent-sandbox/run-k8s-e2e.sh
bash services/runtime/scripts/run-k3d-runtime-lifecycle-e2e.sh
```

## 11. 事件与错误契约

新增或稳定以下 `errorKind`：

```text
workspace.not_found
workspace.permission_denied
workspace.path_outside_root
workspace.channel_unavailable
workspace.channel_unauthorized
workspace.channel_replay
workspace.export_failed
workspace.snapshot_integrity_failed
sandbox.binding_stale
sandbox.channel_lease_failed
project.state_conflict
project.candidate_frozen
dependency.registry_unreachable
preview.process_failed
preview.health_timeout
preview.lease_stale
preview.proxy_unauthorized
artifact.export_failed
artifact.integrity_failed
artifact.candidate_mismatch
artifact.limit_exceeded
artifact.publish_failed
artifact.current_version_conflict
artifact.reconcile_required
event.outbox_delivery_failed
runtime.recovering
```

每次错误必须附带：

- `runId`
- `projectId`
- `sandboxBindingId`
- `buildId` / `versionId`（存在时）
- `tool`
- 逻辑 workspace path，不泄露宿主绝对路径
- `recoverable`
- `suggestedAction`
- retry fingerprint
- `runtimeEpoch` 和 correlation/trace ID（仅内部 debug/audit，用户摘要不暴露基础设施细节）

用户侧只显示可操作摘要；底层 stderr、Pod、PVC 和 tunnel 信息保留在 debug evidence/audit。

## 12. Commit 计划

建议按可独立回滚的顺序提交：

1. `fix(sandbox): align workspace channel filesystem semantics`
   - structured error、nested mkdir、现有回归测试。
2. `feat(sandbox): authenticate workspace channel leases`
   - Ed25519 JWT、granular operation、one-shot `jti`、Pod identity、upgrade deny tests、
     key rotation 和 secret redaction。
3. `refactor(runtime): separate logical workspace paths and state authority`
   - WorkspacePath、backend stat、ToolContext/Permission/AgentLoop/API 迁移、RuntimeStore
     authority、P0 source scan。
4. `feat(runtime): proxy sandbox candidate previews`
   - process lease、fixed 4321 contract、PreviewBackend、principal/ownership proxy、real screenshot。
5. `feat(runtime): stage and promote immutable artifacts`
   - binary export、manifest、source snapshot、ArtifactPublisher two-phase transaction、
     publish record、current CAS、transactional SSE outbox 和 artifact-only reconcile。
6. `feat(runtime): reconcile channel leases and registry access`
   - epoch-aware SandboxChannelManager、startup reconcile、npm proxy Service、NetworkPolicy。
7. `test(e2e): gate the public runtime lifecycle on k3d`
   - Git SHA/image digest parity、Website/Docs、sandbox release 后 artifact 可用、
     auth/concurrency/crash recovery/npm proxy。
8. `docs(runtime): document kubernetes lifecycle operations`
   - README、故障手册、release gate 说明。

不得把 P0-P4 压成一个大提交；Workspace、Preview、Artifact 和 Tunnel 是不同故障域。

## 13. Release Exit Criteria

以下条件全部满足后，才能重新声明 Kubernetes Runtime 稳定可用：

- Public Runtime API 的 Website 和 Docs Build 均在 k3d 中完成；
- evidence 的 repository commit、Runtime/Sandbox image ref 与运行中 container imageID 完全一致，
  且工作树 dirty 状态被显式记录；
- 未认证、过期、错误 Pod UID 或越权 operation 的 Workspace Channel 请求全部被拒绝；
- 同一 JWT `jti` 不能重复建连，Channel/Preview 日志和 SSE 中不包含 token 或 API key；
- Build/Edit/Repair 不再通过宿主机读取 Sandbox 文件；
- RuntimeStore/run snapshot 权威字段不能被 Workspace hint 覆盖；
- `project.build` 和 `preview.publish` 不需要人工 `kubectl exec`；
- Candidate Preview 全部通过 authenticated Runtime Proxy，固定 Sandbox port 为 `4321`；
- Preview Proxy 同时通过 Public API principal、project ownership、lease target 和 Pod UID 校验；
- Artifact 按 stage -> gates -> CAS promote 执行，重试不产生重复 version/event；
- current pointer、publish record 和 `preview.updated` outbox 原子提交，所有 crash-point
  recovery tests 通过；
- source snapshot 使用 Runtime-owned immutable URI，文本与二进制资源可无损恢复；
- Sandbox release 后 promoted Artifact 仍可访问；
- 两个并行项目不会共享错误的 port-forward 或 Workspace；
- production NetworkPolicy 无公网放宽，Sandbox 只访问内部 npm proxy；
- Runtime、port-forward 或 Pod 重启后 reconciliation gate 通过，无 orphan lease/process；
- 同一确定性错误不会无限消耗模型 turn；
- SSE 事件满足 `preview.updated` 先于成功 `run.completed`；
- 真实批准 provider 的 Website/Docs evidence 被保存且不包含 API key；DeepSeek 只有在
  provider route 通过 R17 数据合规评审后才能作为 release evidence。

## 14. 实施结果与下一步

2026-07-11 已按以下顺序完成实现和 fixture 验收：

```text
1. 修复并跑绿当前 P0-P2 全量 Rust/Node 回归，清除共享临时目录导致的并行测试串扰
2. 完成 ArtifactPublishRecord + abort/GC + transactional outbox + crash recovery tests
3. 完成 granular JWT scope、jti replay cache、process lease 和真实 browser evidence
4. 实现 epoch-aware ChannelManager、npm proxy 与 NetworkPolicy
5. 用当前 Git SHA 构建并导入专用 k3d，执行 Website + Docs Public API gate
```

实际 gate 使用专用 `k3d-zerondesign-e2e`，自动创建/选择集群、安装锁定 controller、构建
当前工作树内容指纹镜像、执行 `k3d image import`，并校验 WarmPool Pod spec image 与
container config `imageID`。旧 `k3d-zerondesign`、固定 Pod 名、手工 tunnel、宿主机 Workspace
fallback 和直接 Pod preview URL 均未用于通过证据。

下一步只处理 Release Candidate 收口，不再扩展 Harness 功能：

1. 已完成 Runtime OCI image 与 Kubernetes Deployment，并把 `repository.commit`、Runtime
   image ref/imageID/reported commit 纳入同一 evidence；
2. 接入产品身份层，使 Preview proxy 在 capability lease/Pod UID 之外验证 Public API
   principal 与 project ownership；
3. R17 批准 provider route 后，以同一 Public Runtime API driver 执行 Website + Docs，保存
   不含 credential 的 real-provider evidence；
4. 在 clean worktree/候选 commit 上重跑 gate，随后才可把本文状态改为 `release-ready`。
