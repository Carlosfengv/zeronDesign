# Kubernetes Runtime Release Candidate 可落地收口方案

日期：2026-07-11
状态：PR0-PR6 implementation and unified real-provider audit complete; PR7 release evidence blocked
目标：关闭 Kubernetes Runtime Release Candidate 的剩余阻断项
基线：`fbe55f340d66`

关联文档：

- `2026-07-10-kubernetes-runtime-remote-workspace-remediation-plan.md`
- `2026-07-11-kubernetes-runtime-rc-gap-analysis.md`
- `2026-07-08-runtime-api-freeze.md`
- `2026-07-04-internal-ai-website-docs-generator-requirements.md`

## 1. 最终目标

### 1.1 当前执行状态

截至 2026-07-11，PR0-PR6 已实现，并在新建 k3d 集群
`k3d-zerondesign-mtls-0711-1308` 完成同一条 all-mode 命令下的 fixture、恢复、npm proxy、
DeepSeek Website/Docs Build + Edit real-audit。统一证据为：

```text
services/runtime/target/e2e-evidence/zerondesign-mtls-0711-1308/
  runtime-rc-fbe55f340d66-dirty-06cce5c81959.json
  npm-proxy.json
  recovery.json
  release-evidence.json
```

当前 Runtime candidate 为 `anydesign/runtime:fbe55f340d66-dirty-5b1c556a95d6`。真实项目的
artifact 文本、computed style、依赖安装、取消清理均通过，`terminalToolFailureCount=0`；
8 个恢复注入场景、mTLS 双向身份与证书轮换窗口、credential secret scan 也全部通过。

随后又以单条 `run-runtime-rc-gate.sh` 命令从不存在的集群开始，在
`k3d-zerondesign-fresh-201159` 完成 bootstrap、digest-pinned Runtime/Sandbox 构建、deployed
fixture、active recovery、npm proxy 和 evidence 聚合。证据位于：

```text
services/runtime/target/e2e-evidence/zerondesign-fresh-201159/
  k3d-channel.json
  runtime-rc-fbe55f340d66-dirty-7d416c89ac3e.json
  npm-proxy.json
  recovery.json
  release-evidence.json
```

该 fresh run 是 dirty fixture audit，因此 `releaseEligible=false`；它证明单命令新集群路径，
不替代前述真实 DeepSeek all-mode 证据。默认完整 image prefetch 已在此前 fresh 尝试中两次
通过；最终成功重跑为避免重复下载设置了 `PREFLIGHT_PREFETCH_IMAGES=0`。release mode 已强制
把该值覆盖为 `1`，不允许跳过 prefetch。

真实运行额外关闭了以下稳定性缺口：

- stale in-memory run snapshot 不再覆盖较新的 persisted terminal run；
- cancel 会停止 Preview 进程与 lease，并释放 ChannelManager binding；
- fixture 使用真实 npm dependency，evidence 校验 `node_modules`、lock hash 和 Verdaccio tarball；
- Artifact gate 同时校验精确文本和浏览器 computed style；
- DeepSeek transport timeout 使用明确错误语义，并对可恢复 transport failure 最多执行 5 次
  `1/2/4/8s` 指数退避；HTTP、协议和工具解析错误不重试；
- RC real-provider 单轮预算默认 600 秒，stage 预算默认 1800 秒，Brief confirmation 等待默认
  720 秒；未观察到 confirmation signal 前不得注入 `confirm`。
- registry preflight 对 inspect/pull 使用可配置的 5 次指数退避；多次失败仍以
  `infrastructure.registry_unavailable` 终止，digest drift 不参与重试放行。
- RC runner 在目标集群不存在时自动执行 k8s bootstrap；release mode 遇到同名集群立即失败，
  并在创建集群之前拒绝 dirty worktree；
- k3s 与 Sandbox Node 使用 lock digest，Traefik、metrics-server 和无用 LoadBalancer 被禁用；
  controller、npm proxy 与 fixture gateway 使用 digest ref；
- local-path、controller、npm proxy 和 fixture gateway 分别使用对象出现等待与有界 rollout
  预算，避免把新集群的正常节点拉取延迟误判为产品失败；
- 临时 Provider Secret 带 `anydesign.io/rc-gate-id` owner label，清理仍按固定 Secret 名与本次
  gate ID 双重约束执行。
- Runtime evidence 同时记录 OCI manifest digest、config digest、Pod imageID 和 reported commit；
  release validator 通过后才输出 `RC_RELEASE_ELIGIBLE`、绝对 evidence path、完整 commit SHA 和
  manifest-digest image ref。

当前证据模式仍为 `real-audit`，且仓库 dirty、镜像为 dirty candidate，因此
`releaseEligible=false` 是预期结果。PR7 必须等待 R17 approval reference 与 clean commit；
release mode 不允许通过 dirty、fixture、real-audit 或 reused image evidence。最终 release
必须在提交后重新创建全新集群并冷构建，不得把本次迭代使用的集群直接升级为 release 证据。

本方案不扩展新的生成能力，只完成以下 Release Candidate 闭环：

```text
authenticated Public Preview
  + encrypted production Workspace Channel
  + concurrent/restart/npm-proxy Kubernetes gates
  + reproducible Runtime OCI cold build
  + deployed fixture and approved-provider lifecycle
  + one machine-verifiable clean evidence bundle
  = release-ready
```

最终必须能在不存在旧 cluster、network、volume 和项目镜像的机器状态下，用一条仓库命令：

1. 完成依赖与镜像 preflight；
2. 创建专用 k3d 集群；
3. 部署 Runtime、Sandbox controller、npm proxy 和双 WarmPool；
4. 执行 Website/Docs Build + Edit；
5. 执行并发、重启、Pod replacement 和依赖安装场景；
6. 对批准的真实 provider 重复 Website/Docs Build + Edit；
7. 生成 `release-evidence@1`；
8. validator 只在所有 exit criteria 通过时返回 0。

## 2. 强制架构决策

以下决策在实现阶段不再重新讨论。

### RC-DEC-001：Public Preview 只允许 BFF 代理访问

浏览器不直接访问 Runtime `/previews/{leaseId}`。产品 BFF 是唯一浏览器入口：

```text
Browser
  -> BFF /projects/{projectId}/previews/{leaseId}/{path}
  -> Runtime /previews/{leaseId}/{path}
  -> Sandbox frozen candidate
```

BFF 为每个上游请求签发短时 Ed25519 principal token。Runtime 不信任裸
`x-user-id`、`x-project-id` 或客户端提交的 ownership header。

### RC-DEC-002：Principal token 绑定单一 project 和 operation

JWT claims 固定为：

```json
{
  "iss": "anydesign-bff",
  "aud": "anydesign-runtime-public",
  "sub": "principal-id",
  "jti": "unique-token-id",
  "iat": 0,
  "exp": 0,
  "projectId": "project-id",
  "operations": ["preview.read"]
}
```

约束：

- TTL 默认 60 秒，最大 300 秒；
- `aud`、`iss`、signature、`exp`、`projectId` 和 operation 全部必须验证；
- token 不进入 URL、SSE、conversation、audit input summary 或 evidence；
- key rotation 使用 current + previous public key；
- Preview GET 允许同一 token 在 TTL 内读取同一 lease 的多个 asset，不采用 one-shot jti；
- audit 只记录 `sub` 的不可逆 hash、project、lease 和 allow/deny reason。

### RC-DEC-003：RuntimeStore 持有 project access authority

新增 `ProjectAccessRecord`：

```rust
pub struct ProjectAccessRecord {
    pub project_id: String,
    pub owner_principal_id: String,
    pub workspace_id: Option<String>,
    pub organization_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

MVP Preview 授权只接受：

```text
claims.projectId == lease.projectId
&& claims.projectId == run.projectId
&& claims.sub == ProjectAccessRecord.ownerPrincipalId
&& claims.operations contains preview.read
```

组织管理员、workspace member 等扩展授权留给后续 Policy Engine；RC 不实现隐式继承。

### RC-DEC-004：每个 Preview asset 请求都必须鉴权

不得用带 token 的 query string，也不得把 `leaseId` 当 bearer credential。

BFF 转发 HTML、JS、CSS、图片和字体的每一个请求。Runtime HTML 重写只允许使用通过
principal 验证后的 `x-anydesign-preview-prefix`，并要求：

- prefix 必须以 `/projects/{projectId}/previews/{leaseId}` 开头；
- 禁止 scheme、host、`..`、反斜杠和 percent-encoded traversal；
- BFF 删除客户端原始同名 header 后再写入；
- Runtime 返回 `Cache-Control: private, no-store`。

Runtime Browser Worker 不使用公共 BFF 路由，也不获得匿名例外。Runtime 另启一个只绑定
`127.0.0.1` 的内部 capture proxy，和 Public Preview 复用 lease、binding、Pod UID、manifest
校验逻辑，但不注册到 Kubernetes Service。Chromium 与 Runtime 位于同一 Pod，使用该 loopback
URL 截图；生成代码运行在 Sandbox，不能访问 Runtime Pod loopback。

```text
RUNTIME_BROWSER_PROXY_BIND=127.0.0.1:8081
```

截图 metadata 只保存 `runtime://preview-captures/{projectId}/{runId}/{leaseId}`，不得保存内部
loopback URL。Public Preview 不得因为 Browser Worker 而增加 query token、cookie bypass 或
anonymous route。

### RC-DEC-005：Production Channel 使用 mTLS + JWT

职责固定为：

- mTLS：加密传输并验证 Runtime/Sandbox workload identity；
- Pod-bound JWT：绑定 project/run/binding/Pod UID，并限制具体 operation；
- NetworkPolicy：缩小可达面，不替代身份和加密。

Production 使用 `wss://`。只有显式 `local-e2e` + loopback `kubectl port-forward` 可以使用
`ws://`，并在 evidence 标记：

```text
transport.mode=debug-loopback
transport.releaseEligible=false
```

### RC-DEC-006：RC 模式拒绝 dirty worktree

```text
RUNTIME_RC_MODE=release + dirty_count > 0 -> exit 2 before build
RUNTIME_RC_MODE=audit + dirty_count > 0 -> continue, releaseEligible=false
```

不存在默认允许 dirty 的 release mode。

## 3. Workstream A：Public Principal 与 Preview Ownership

### 3.1 Runtime 配置

修改 `services/runtime/src/config.rs`，新增：

```text
PUBLIC_PRINCIPAL_AUTH_MODE=required|disabled
PUBLIC_PRINCIPAL_ISSUER=anydesign-bff
PUBLIC_PRINCIPAL_AUDIENCE=anydesign-runtime-public
PUBLIC_PRINCIPAL_PUBLIC_KEY_FILES=/keys/current.der,/keys/previous.der
PUBLIC_PRINCIPAL_MAX_TTL_SECONDS=300
```

规则：

- `Production` profile 强制 `required`；
- `disabled` 只允许 `LocalE2e`；
- 配置缺失时 Production 启动失败，不能自动降级。

### 3.2 Principal verifier

新增 `services/runtime/src/public_principal.rs`：

- 解析 `Authorization: Bearer`；
- 只接受 EdDSA/Ed25519；
- 验证 current/previous key、issuer、audience、TTL 和 clock skew；
- 返回不包含 raw token 的 `PublicPrincipal`；
- 提供稳定错误：
  - `public_auth.missing`
  - `public_auth.invalid`
  - `public_auth.expired`
  - `public_auth.wrong_audience`
  - `public_auth.project_forbidden`
  - `public_auth.operation_forbidden`

HTTP 映射：认证失败 401，授权失败 403，lease/target 不存在 404，identity drift 409。

### 3.3 Project ownership persistence

修改：

- `services/runtime/src/types.rs`
- `services/runtime/src/conversation.rs`
- `services/runtime/src/http_api.rs`

要求：

- 创建项目首个 Brief/Build run 前必须已有 ownership record；
- BFF 通过 service-authenticated internal endpoint upsert ownership；
- agent、Workspace hint 和模型 tool 不能修改 ownership；
- checkpoint/restart 后 ownership 可恢复；
- 旧 project record 读取兼容，但在 Production 访问 Preview 时按 deny-by-default 处理。

内部 endpoint：

```text
PUT /internal/projects/{projectId}/access
x-anydesign-internal: true
x-runtime-admin-token: <service secret>
```

Body：

```json
{
  "ownerPrincipalId": "principal-id",
  "workspaceId": "workspace-id",
  "organizationId": "organization-id"
}
```

### 3.4 Preview proxy authorization

修改 `services/runtime/src/http_api.rs`：

```text
verify principal
  -> load active lease
  -> load run
  -> load project access
  -> verify principal/project/operation
  -> verify binding/sandbox/Pod UID
  -> resolve ChannelManager endpoint
  -> verify candidate manifest hash
  -> stream bytes
```

禁止在 principal 校验前建立 upstream 连接。

### 3.5 BFF client contract

修改：

- `packages/shared/src/runtime-client.ts`
- `packages/shared/src/runtime-client.test.ts`
- `services/runtime/tests/mock_bff_contract.rs`

新增：

- ownership upsert client；
- principal token provider interface；
- Preview streaming proxy helper；
- `x-anydesign-preview-prefix` overwrite；
- Authorization、JWT 和 admin token redaction tests。

### 3.6 测试

`services/runtime/tests/http_api.rs` 至少增加：

1. anonymous Preview -> 401；
2. invalid/expired/wrong audience -> 401；
3. correct principal, wrong project -> 403；
4. correct project, missing operation -> 403；
5. correct principal and project -> 200；
6. HTML asset path 保持 BFF prefix；
7. stale lease -> 404；
8. Pod UID replacement -> 409；
9. audit/SSE/body 不包含 raw token；
10. Production 缺 key 配置时启动失败。

### 3.7 Exit gate

```bash
cargo test --manifest-path services/runtime/Cargo.toml --test http_api preview_
cargo test --manifest-path services/runtime/Cargo.toml --test mock_bff_contract
npm test --prefix packages/shared
```

## 4. Workstream B：Production mTLS Channel

### 4.1 Certificate contract

新增两个 identity：

```text
Runtime client SAN: spiffe://anydesign.local/ns/anydesign-runtime/sa/anydesign-runtime
Sandbox server SAN: spiffe://anydesign.local/ns/anydesign-sandboxes/sa/anydesign-sandbox
```

RC 可使用仓库 runner 创建的短期 test CA；Production 由内部 PKI/cert-manager 签发。私钥只存在
Kubernetes Secret volume，不写入 image、ConfigMap 或 evidence。

mTLS 证明调用方和服务端 workload role；同一 Sandbox role 下的具体 Pod UID 仍由短时 JWT
绑定。不得尝试在 Pod 创建前签发包含动态 Pod UID 的静态 Secret 证书。

### 4.2 Sandbox server

修改 `infra/agent-sandbox/base/workspace-channel-server.js`：

- Production 使用 `tls.createServer` + WebSocket upgrade；
- `requestCert=true`、`rejectUnauthorized=true`；
- 校验 Runtime client SAN；
- 在 TLS 成功后继续执行现有 Pod-bound JWT 校验；
- 日志只记录 cert fingerprint 前 12 位，不记录证书或 token。

新增环境变量：

```text
WORKSPACE_CHANNEL_TLS_MODE=required|debug-loopback
WORKSPACE_CHANNEL_CA_FILE=/tls/ca.crt
WORKSPACE_CHANNEL_CERT_FILE=/tls/tls.crt
WORKSPACE_CHANNEL_KEY_FILE=/tls/tls.key
WORKSPACE_CHANNEL_RUNTIME_SAN=...
```

### 4.3 Runtime client

修改：

- `services/runtime/src/channel_manager.rs`
- `services/runtime/src/tools/sandbox.rs`
- `services/runtime/src/config.rs`

新增 Runtime CA/client cert/key 配置和 `wss` transport。证书校验必须绑定预期 Sandbox SAN；
不得使用 `danger_accept_invalid_certs`。

### 4.4 Kubernetes manifests

修改：

- `infra/agent-sandbox/rbac/runtime-service-account.yaml`
- `infra/agent-sandbox/astro-website/sandbox-template.yaml`
- `infra/agent-sandbox/fumadocs-docs/sandbox-template.yaml`
- `infra/agent-sandbox/runtime/deployment.yaml`
- `infra/agent-sandbox/network/default-deny.yaml`

要求：

- Runtime/Sandbox 分别挂载最小证书 Secret；
- 新增专用 `anydesign-sandbox` ServiceAccount，SandboxTemplate 显式使用该账号；
- Sandbox server 只监听 TLS channel port；
- Preview 4321 仍仅允许 Runtime namespace；
- Runtime SA 不获得读取其他 Secret 的权限；
- test CA 由 runner 生成并直接创建 Secret，不落盘到 evidence 目录。

### 4.5 mTLS gate

必须证明：

- 无 client cert、错误 CA、错误 Runtime SAN 被 TLS 拒绝；
- 正确 mTLS 但无 JWT 被应用层拒绝；
- 正确 mTLS + wrong Pod UID JWT 被拒绝；
- 正确 mTLS + 正确 JWT 才能执行 `fs.read`；
- key/cert rotation 后 current + previous 窗口可用；
- evidence 只记录 mode、SAN hash、cert serial hash 和 expiry，不记录证书内容。

## 5. Workstream C：并发、Restart 与 npm Proxy Gate

### 5.1 复用同一 Public Runtime driver

将 `services/runtime/tests/k8s_public_runtime_e2e.rs` 中生命周期 driver 提取为可复用模块，供：

- in-process fixture；
- deployed fixture；
- deployed real provider；
- concurrency/restart scenario

共同使用。禁止为真实 provider 单独写一条弱化路径。

### 5.2 完整生命周期并发

Website 和 Docs 使用 `tokio::join!` 同时执行：

```text
Brief confirmed
  -> Build
  -> Preview
  -> Screenshot
  -> Promote
  -> Edit
  -> Rebuild
  -> Promote next version
```

必须断言：

- project/run/binding/Pod UID 全部不同；
- preview lease target、candidate manifest 和 screenshot document hash 不同；
- current version 分别只推进自己的 project；
- artifact file/manifest 不交叉；
- 一个 run 的 failure/cancel 不终止另一个 run。

### 5.3 Restart matrix

新增 `infra/agent-sandbox/run-runtime-recovery-gate.sh`：

| 场景 | 注入点 | 预期 |
|---|---|---|
| Runtime restart | active channel lease | old epoch retired，binding reacquire |
| Runtime restart | artifact staged before CAS | reconcile promote 或 abort，current 一致 |
| Runtime restart | CAS after bytes before event | outbox 幂等补发一次 |
| Sandbox Pod replacement | active preview lease | old lease stable stale error，新 lease 绑定新 Pod UID |
| Port-forward kill | active desktop channel | process reaped，下一操作 reacquire |
| Run cancel | active preview process | process、tunnel、lease 全部 stopped |

每个场景完成后执行 orphan audit：

```text
active leases belong to active bindings
active processes belong to active runs
no stale runtime epoch child process
no staged publish past GC deadline
no claim without registered owner/cleanup state
```

### 5.4 npm proxy gate

在 fixture package 中新增未预置的小依赖，固定版本和 integrity。通过 Runtime
`project.ensure_dependencies` 安装，禁止测试直接执行 `kubectl exec npm install`。

断言：

- Verdaccio access log 出现 project request；
- Sandbox 内 DNS 能解析 npm proxy；
- direct npmjs request 失败；
- package 安装、lockfile 和 build 全部在 PVC；
- 第二项目并行安装不污染第一项目；
- 上游断开时命中缓存或返回 typed infrastructure error。

## 6. Workstream D：Hermetic Runner 与 Runtime OCI

### 6.1 锁定镜像清单

新增 `infra/agent-sandbox/images.lock.json`，至少包含：

```text
k3s
k3d-tools
k3d-proxy
dockerfile frontend
Rust builder
Debian runtime base
agent-sandbox controller
Verdaccio
Node fixture gateway
local-path helper/busybox（若 runner 仍依赖）
```

每项记录 registry ref、platform 和 digest。禁止只锁 tag。

### 6.2 Preflight

新增 `infra/agent-sandbox/preflight-runtime-rc.sh`：

1. 检查 Docker daemon、buildx、k3d、kubectl、openssl、node、cargo、Chrome；
2. 检查 credential helper，公共 pull 不得因 helper 无响应无限等待；
3. 对每个镜像执行有界 registry probe；
4. prefetch 并校验 digest；
5. 在 cluster 创建后 import 必需镜像；
6. 将拉取失败分类为 `infrastructure.registry_unavailable`；
7. 禁用 k3d 未使用的 Traefik，消除无关 ImagePullBackOff；
8. 任何 timeout 都必须有明确非零退出码和诊断摘要。

### 6.3 Runtime Dockerfile

修改 `services/runtime/Dockerfile`：

- Dockerfile frontend 使用 lock file 中的 digest；
- runner 在 build 前保证 frontend 已进入 BuildKit 可用 cache/registry；
- Rust/Debian base 使用 digest；
- apt 包源和 package version 进入 build metadata；
- image labels 包含 repository commit、dirty、lock hash 和 build timestamp；
- cold build 不能依赖宿主 Cargo cache 才成功。

### 6.4 RC mode

修改 `infra/agent-sandbox/run-runtime-rc-gate.sh`：

```text
preflight
  -> reject dirty in release mode
  -> create or assert empty dedicated cluster
  -> build/import Runtime image
  -> verify Pod image ref/config digest/reported commit
  -> run fixture/recovery/concurrency/npm gates
  -> optionally run approved provider
  -> aggregate and validate evidence
```

`RUNTIME_RC_REUSE_IMAGE` 只允许 audit/debug，设置后 `releaseEligible=false`。

## 7. Workstream E：统一 Release Evidence

### 7.1 Schema

新增：

- `services/runtime/evidence/release-evidence.schema.json`
- `services/runtime/scripts/aggregate-release-evidence.mjs`
- `services/runtime/scripts/validate-release-evidence.mjs`

顶层结构：

```json
{
  "schemaVersion": "release-evidence@1",
  "releaseEligible": true,
  "repository": {},
  "cluster": {},
  "images": {},
  "transport": {},
  "auth": {},
  "provider": {},
  "projects": [],
  "recoveryScenarios": [],
  "networkChecks": {},
  "secretScan": {},
  "result": "pass"
}
```

### 7.2 必填字段

```text
repository.commit / dirty / lockHash
cluster.name / kubeContext / createdAt / nodeUid
images.runtime.ref / configDigest / reportedCommit
images.sandbox.ref / configDigest
images.controller / npmProxy / dockerfileFrontend digests
transport.mode / mtlsVerified / runtimeSanHash / sandboxSanHash
auth.principalMode / projectOwnershipVerified / channelJwtVerified
provider.mode / model / approvalReference
project.kind / projectId / buildRunId / editRunId
bindingId / podUid / buildId / candidateManifestHash
sourceSnapshotUri / previewLeaseId / screenshotId / nonblankPixelRatio
version before/after CAS / artifactManifestHash / artifact URL
preview.updated eventId / run.completed eventId / sequenceValid
sandboxReleasedAt / artifactHttpStatusAfterRelease
recovery scenario / injectionPoint / result / orphanCount
network directRegistryDenied / npmProxyInstallPassed
secretScan.patternSet / filesScanned / matches
```

### 7.3 Validator hard failures

以下任一条件返回非零：

- `releaseEligible != true`；
- repository dirty；
- commit、image digest、reported commit 不一致；
- Website 或 Docs 缺 Build/Edit；
- provider 未批准或 approval reference 缺失；
- principal/mTLS/npm/recovery/concurrency 任一 gate 缺失；
- screenshot 空白或 hash 不一致；
- event sequence 错误；
- Sandbox release 后 Artifact 非 200；
- orphan count 非 0；
- secret scan matches 非空。

### 7.4 Secret policy

扫描范围包括 summary、原始 gate JSON、runner log 和 Kubernetes sanitized dump。禁止写入：

- API key、Authorization、JWT；
- private key、client key、完整 certificate；
- provider request body；
- 内部附件正文；
- environment dump。

## 8. Workstream F：Deployed Fixture 与真实 Provider

### 8.1 Deployed fixture

Runtime 必须以 Kubernetes Deployment 运行。driver 只能经 Runtime Service/port-forward 调用
Public HTTP/SSE API，不能直接访问测试内 `RuntimeStore`。

为了查询状态，使用公开 API 或专用 read-only evidence endpoint；不得让 test 通过进程内 store
绕过部署边界。

### 8.2 Provider Secret

真实 provider credential 通过临时 Kubernetes Secret 注入 Runtime Deployment：

- runner 从环境读取，不接受 CLI argument；
- `kubectl create secret --from-literal` 的命令和输出不得进入 log；
- Secret 有本次 gate 的 owner label；
- gate 结束立即删除；
- evidence 只写 `credentialPresent=true`，不写 hash 或长度。

### 8.3 R17 approval gate

新增必填环境变量：

```text
RUNTIME_PROVIDER_APPROVAL_ID=<internal approval reference>
```

release mode 缺失时直接失败。approval 至少覆盖：

- provider route；
- 允许的数据分类；
- retention/log policy；
- prompt、附件、design source 和生成代码处理范围；
- incident/audit owner。

### 8.4 真实 provider 场景

Website 与 Docs 分别执行：

```text
Brief -> Confirm -> Build -> Preview -> Promote
Edit -> Rebuild -> Computed style/content assertion -> Promote
Release Sandbox -> Artifact 200
```

Provider 限流、DNS 或 5xx 只能标记 `infrastructure failure`，不能写 pass。模型产生 terminal
tool contract violation 则标记 product failure。

## 9. 分阶段 PR 计划

### PR 0：冻结 RC schema 与红灯测试

内容：

- 增加 principal、mTLS、concurrency、restart、npm 和 evidence validator 的 failing tests；
- 增加 `release-evidence@1` schema；
- RC mode dirty fail；
- 不改变现有 LocalE2e 行为。

验收：测试能够准确红在当前缺口，现有绿色套件不回归。

### PR 1：Public principal + project ownership

内容：Workstream A 全部。

回滚：Production Preview deny-all；不得回退到匿名 lease URL。

### PR 2：mTLS Workspace Channel

内容：Workstream B 全部。

回滚：阻止新的 Production run；LocalE2e debug-loopback 不受影响。

### PR 3：并发、restart、npm gates

内容：Workstream C 全部。

回滚：仅关闭新 gate，不回滚 RuntimeStore additive schema。

### PR 4：Hermetic runner 与 OCI cold build

内容：Workstream D 全部。

回滚：允许 audit runner 使用旧路径，但 `releaseEligible=false`。

### PR 5：统一 evidence 与 deployed fixture

内容：Workstream E + F 的 fixture 部分。

验收：clean cluster deployed fixture summary 通过 validator。

### PR 6：Approved real-provider gate

内容：Workstream F 的 provider 部分。

验收：Website/Docs Build+Edit、computed style、secret scan 和 cleanup 全部通过。

### PR 7：最终 RC evidence

内容：不新增功能，只在 clean candidate commit 上运行 release command、保存 sanitized summary、
更新原方案状态。

## 10. Commit 建议

```text
test(runtime): freeze RC auth and evidence contracts
feat(runtime): authorize preview access by project principal
feat(runtime): secure workspace channels with mutual TLS
test(k8s): add concurrent recovery and npm proxy gates
build(k8s): make RC image provisioning reproducible
test(runtime): aggregate and validate release evidence
test(provider): run deployed Website and Docs lifecycle gate
docs(runtime): record clean Release Candidate evidence
```

每个 commit 必须可独立编译；schema migration 只允许 additive。

## 11. 每个 PR 的强制检查

```bash
cargo fmt --manifest-path services/runtime/Cargo.toml --check
cargo test --manifest-path services/runtime/Cargo.toml --all-targets
npm test --prefix packages/shared
npm run typecheck --prefix packages/shared
bash services/runtime/scripts/check-remote-workspace-fs-boundary.sh
bash -n infra/agent-sandbox/*.sh
git diff --check
```

涉及 Kubernetes 的 PR 还必须在随机新 cluster name 上执行，不能复用开发者默认集群。

## 12. 最终 Release 命令

目标命令：

```bash
RUNTIME_RC_MODE=release \
ANYDESIGN_E2E_CLUSTER="zerondesign-rc-$(git rev-parse --short=12 HEAD)" \
RUNTIME_PROVIDER_APPROVAL_ID="<approval-reference>" \
DEEPSEEK_API_KEY="${DEEPSEEK_API_KEY}" \
bash infra/agent-sandbox/run-runtime-rc-gate.sh
```

runner 必须拒绝：

- 同名 cluster 已存在；
- dirty worktree；
- 缺失 approval；
- 镜像 digest 不匹配；
- 任何 required gate 缺失；
- evidence secret scan 命中。

成功输出只能是：

```text
RC_RELEASE_ELIGIBLE=true
RC_EVIDENCE_SUMMARY=<absolute path>
RC_COMMIT=<full git sha>
RC_RUNTIME_IMAGE=<digest-pinned ref>
RC_SANDBOX_IMAGE=<digest-pinned ref>
```

## 13. 完成定义

只有同时满足以下条件，原修复方案才能改为 `release-ready`：

1. PR 0-PR 6 全部合并且主分支绿色；
2. Public Preview anonymous/cross-project access 被拒绝；
3. Production mTLS + JWT gate 通过；
4. Website/Docs 完整生命周期并发且不串线；
5. Runtime/Pod/port-forward restart recovery 无 orphan；
6. Sandbox 只能经 npm proxy 成功安装依赖；
7. clean cluster Runtime OCI cold build 和 deployed fixture 通过；
8. R17 approval reference 有效；
9. deployed real-provider Website/Docs Build+Edit 通过；
10. `release-evidence@1` validator 返回 0；
11. repository dirty=false；
12. secret scan matches=0。

任何一项缺失，状态保持 `RC blocked`。
