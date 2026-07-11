# Kubernetes Runtime Release Candidate 差距分析

日期：2026-07-11
状态：Unified fixture + real-provider RC audit passed; final release blocked
对照文档：`2026-07-10-kubernetes-runtime-remote-workspace-remediation-plan.md`
审计基线：`fbe55f340d66`
当前验证集群：`k3d-zerondesign-mtls-0711-1308`

## 1. 结论

### 1.1 2026-07-11 实施更新

本文件后续第 2-8 节保留首次 fresh-cluster 审计快照，用于说明缺口来源；以下实施更新是当前
判定，优先于旧快照。

已在全新集群 `k3d-zerondesign-mtls-0711-1308` 完成：

- Public Principal：production required、project owner 持久化、Candidate Preview 匿名 401、
  跨项目 403、owner 200，token 在每次预览请求前即时签发；
- Workspace Channel：双向 TLS、SPIFFE URI SAN、Pod-bound JWT 及 no-cert/wrong-CA/
  wrong-JWT/wrong-Pod/wrong-SAN 负向矩阵；
- deployed fixture：Website/Docs 均完成 Build + Edit、非空截图、CAS Promote、Sandbox release，
  release 后 Artifact 200；
- 并发、Runtime recovery、Warm Pod replacement、npm proxy 与 direct npmjs deny gate；
- digest 锁定 preflight、无宿主 Cargo cache 的 Runtime OCI cold build、Pod image/config digest/
  reported commit parity；
- `release-evidence@1` Schema、聚合器、strict validator 和 credential secret scan；
- 单命令 audit 结束后 `SandboxClaim=0`，失败路径按本次 gate ID 回收 claim 和临时 Secret。

在此基础上，又从不存在的集群名 `k3d-zerondesign-fresh-201159` 开始，仅调用一次
`run-runtime-rc-gate.sh`，完成 k3d bootstrap、Channel/Public Runtime gate、Runtime cold build、
deployed fixture、恢复、npm proxy 与统一 evidence。该 fresh fixture audit 的证据目录为：

`services/runtime/target/e2e-evidence/zerondesign-fresh-201159/`

这份证据证明单命令新集群路径；真实 Provider 结论仍来自下述同一 all-mode DeepSeek audit，
两类证据不混写。

真实 DeepSeek `deepseek-v4-pro` 已验证 API HTTP 200，并在同一条 Kubernetes all-mode audit
中依次完成 Website/Docs Build + Edit。两条路径均包含非空截图、CAS、release 后 Artifact 200、
Sandbox release、事件顺序检查、真实依赖证据、Artifact 文本与 computed style 断言，以及
`terminalToolFailureCount=0`。统一审计证据位于：

- `services/runtime/target/e2e-evidence/zerondesign-mtls-0711-1308/real-provider-website.json`；
- `services/runtime/target/e2e-evidence/zerondesign-mtls-0711-1308/real-provider-docs.json`；
- `services/runtime/target/e2e-evidence/zerondesign-mtls-0711-1308/release-evidence.json`。

真实运行额外发现并修复：

1. `process.exec` 的 WebSocket transport 固定 30 秒，现改为 command timeout + 5 秒；
2. Verdaccio 在 Astro 冷缓存 restore 时 OOM，现使用 digest 镜像、1 GiB limit 和 768 MB heap；
3. 长 streaming provider turn 缺少总 wall-clock timeout，现由 `tokio::time::timeout` 对完整
   send + stream parse 设置上限；RC real-provider audit 使用 600 秒；
4. 短时 principal token 不再在任务开始时签发，而在 Preview 请求前即时签发；
5. evidence 分开记录 recoverable 与 terminal tool failure，只有 terminal 非零阻止 release；
6. `process.exec` 超时只终止直接子进程，现为一次性命令创建独立进程组，并在返回前对整个
   进程组执行 TERM/KILL；真实 Docs 重跑未再出现并发残留的 `npm install`；
7. Workspace archive 使用 JavaScript `localeCompare`，与 Rust UTF-8 字节序不一致，导致含
   `404.html` 和 `_next/*` 的 Docs manifest mismatch；现统一使用 UTF-8 byte order 并增加回归测试；
8. k3d fixture gate 仍校验旧 `runId/versionId` 字段，现同步到 `buildRunId/editRunId` 和 CAS
   版本字段，完整脚本最终返回 0。
9. stale in-memory `Running` snapshot 会覆盖较新的 persisted terminal run，现按 snapshot
   时间合并，并增加重启回归测试；
10. cancel 只改 run status，未停止 Preview 与 Channel lease，现同步停止进程、lease 并释放
    binding，取消后旧 Preview 必须返回 404；
11. npm gate 原先只验证连通性，现 fixture 安装真实 package，并记录 Pod UID、`node_modules`、
    lockfile SHA-256 与 Verdaccio tarball request count；
12. mTLS gate 增加 current/previous CA rotation window，evidence 只记录 SAN、serial hash 与 expiry；
13. Artifact gate 不再只看 HTTP 200，现使用 Chromium 校验精确文本与 computed style；
14. DeepSeek transport failure 现区分 timeout/transport/decode，并仅对 transport failure 执行
    最多 5 次 `1/2/4/8s` 指数退避；
15. RC harness 的 120 秒 Brief 等待、600 秒 stage 与 600 秒 provider turn 预算冲突，现默认
    使用 720 秒 Brief confirmation、600 秒 provider turn、1800 秒 real stage，并禁止在未观察到
    confirmation signal 时发送 `confirm`。
16. image-lock preflight 遇到 CloudFront 短时 EOF 会立即失败，现对 registry inspect/pull 使用
    可配置的 5 次指数退避；重跑已通过，digest drift 仍立即阻断。
17. RC gate 原先要求集群和 channel evidence 预先存在，最终 release 命令不能从空状态执行；
    现由 gate 在集群不存在时自动调用 k8s bootstrap，并把证据写入同一 cluster 目录；
18. release mode 的 dirty 检查原先位于 cluster bootstrap 之后，现前移到任何集群创建之前；
    实测 dirty release 返回 2，目标集群不存在；
19. 新集群中 controller 裸 tag + 120 秒 rollout、npm proxy 180 秒 rollout 会因节点侧 registry
    延迟产生假阴性；现 controller 强制 lock digest，controller/npm/fixture gateway 使用 600 秒
    有界 rollout，local-path 先等待对象出现再等待 Ready；
20. k3d bootstrap 现使用 lock 中的 k3s/Node digest，禁用 Traefik、metrics-server 和无用
    LoadBalancer；fixture gateway 改为 Node digest，Provider Secret 增加 gate owner label；
21. release mode 强制 `PREFLIGHT_PREFETCH_IMAGES=1`，调用者只能在 audit mode 关闭重复 prefetch。
22. Runtime image 原先只有 config digest，且 release 成功分支没有方案要求的机器可读输出；
    现从 OCI `index.json` 提取 manifest digest，Schema/validator 强制校验，并仅在 strict validator
    通过后输出 `RC_RELEASE_ELIGIBLE`、绝对 evidence path、完整 commit SHA 和 digest ref。

补充兼容性结论：`deepseek-v4-pro` 的普通 streaming tools 已在同一 all-mode audit 中完成
Website 与 Docs lifecycle。当前 DeepSeek route 仍不启用 `MODEL_STRICT_TOOLS`；参数预算由
Runtime parser、typed recovery 和 provider turn wall-clock timeout 强制执行。此前模糊的
`error decoding response body` 已归一为明确的 timeout 或 transport error，不再作为 JSON
协议错误记录。

当前只剩两个最终 release 阻断项：

| 阻断项 | 当前状态 | 放行条件 |
|---|---|---|
| R17 provider approval | 未提供 | 提供覆盖 route、数据分类、retention、附件策略和 owner 的 approval reference |
| clean candidate | 当前工作树包含本次实现与用户认可的项目名修订 | 提交后在全新空集群使用 release mode 冷构建，strict validator 返回 0 |

因此当前可以声明 **fixture + real-provider RC audit passed**，不能声明 `release-ready`。现有
DeepSeek 证据模式是 `real-audit`，在 R17 approval reference 写入前不得改标为 `approved-real`。

当前实现已经具备可用的远端 Workspace、Candidate Preview、Artifact Promotion 和
Website/Docs fixture 生命周期，但还不能声明 Kubernetes Runtime release-ready。

本次从不存在 cluster、Docker network 和 image volume 的状态创建全新 k3d 集群。以下
两层 gate 已通过：

- Workspace Channel：双 claim、Pod-bound JWT、granular scope、jti 防重放、process lease、
  binary export 和并行 Workspace 隔离；
- Public Runtime fixture：Website 与 Docs 的 Build -> Preview -> Screenshot -> CAS Promote，
  Sandbox release 后 Artifact 仍返回 200，且 `preview.updated` 先于 `run.completed`。

Release Candidate 仍有 4 个功能或安全阻断项、4 个 release-evidence 阻断项，以及 3 个
clean-cluster runner 可复现性问题。最关键的缺口不是生成能力，而是 Public Preview 身份、
生产传输身份、部署态真实 provider 和可重复的最终证据。

## 2. 首次 Fresh-cluster 验证结果（历史快照）

### 2.1 环境真实性

创建前检查：

```text
k3d cluster: absent
Docker network k3d-zerondesign-rc-audit-20260711: absent
同名 image volume: absent
```

创建后状态：

```text
cluster: zerondesign-rc-audit-20260711
kubeContext: k3d-zerondesign-rc-audit-20260711
server: 1/1 Ready
k3s: v1.31.5+k3s1
agent-sandbox controller: v0.5.0, 1/1 Ready
Website WarmPool: 1 Ready
Docs WarmPool: 1 Ready
npm proxy: 1/1 Ready
```

当前 Sandbox 镜像来自审计 checkout，但工作树包含用户对原方案文档的 1 个修改文件，故
evidence 正确标记为 dirty：

```text
repository.commit=fbe55f340d66
repository.dirtyFiles=1
sandbox.imageRef=anydesign/astro-website-sandbox:fbe55f340d66-dirty-77348e307e6c
sandbox.imageId=sha256:55ea3a3707e40b7a378f1431fc934a92e3da10509b26cd6185941c6cecace74a
```

这组结果可用于实现审计，不可作为最终 clean RC evidence。

### 2.2 已通过证据

- `services/runtime/target/e2e-evidence/zerondesign-rc-audit-20260711/k3d-channel-fbe55f340d66.json`
- `services/runtime/target/e2e-evidence/zerondesign-rc-audit-20260711/public-runtime-fixture-fbe55f340d66.json`
- `check-remote-workspace-fs-boundary.sh`：通过；
- `cargo fmt --check`：通过；
- `git diff --check`：通过。

Website 与 Docs evidence 均包含独立 `podUid`、`buildId`、candidate/artifact manifest hash、
source snapshot URI、preview lease、真实 1440x900 PNG、CAS 前后版本、事件顺序和 release 后
Artifact 200。两份页面的 document/PNG hash 不同。

### 2.3 部署态 gate 结果

`run-runtime-rc-gate.sh` 未完成。失败发生在 Runtime OCI 编译之前：

```text
#2 resolve image config for docker-image://docker.io/docker/dockerfile:1.7
```

在全新 Docker 环境中，BuildKit 必须远端解析 Dockerfile frontend；即使宿主已预拉
`docker/dockerfile:1.7`，docker driver 仍会访问 registry。当前 runner 没有 frontend
离线导入或 registry preflight，因此部署态 Runtime image、`/version` parity 和 deployed
Website/Docs HTTP gate 本轮没有产生证据。

## 3. Release Exit Criteria 对照

| # | Exit criterion | 状态 | 当前证据或缺口 |
|---|---|---|---|
| 1 | Website/Docs Public Runtime Build 在 k3d 完成 | 部分 | Sandbox 在新 k3d；Public Runtime 由测试进程启动，不是 Kubernetes Deployment |
| 2 | repository、Runtime/Sandbox image ref 与 imageID 一致 | 部分 | Sandbox parity 通过且 dirty 被记录；Runtime OCI 未构建/部署 |
| 3 | Channel 未认证、过期、错误 Pod UID、越权 operation 被拒绝 | 已实现 | JWT 单测、Node server 测试和 fresh-cluster channel gate 通过 |
| 4 | jti 防重放，日志/SSE 无 token/API key | 已实现 | replay test 与 evidence secret scan 通过 |
| 5 | Build/Edit/Repair 不读取宿主 Sandbox 文件 | 部分 | source scan 与 Build k3d 通过；Edit/Repair 没有 deployed k3d lifecycle evidence |
| 6 | RuntimeStore 权威字段不被 Workspace hint 覆盖 | 已实现 | authority 回归测试通过 |
| 7 | build/publish 不依赖人工 kubectl exec | 已实现 | fresh-cluster gate 全程由 Public Runtime/tool driver 完成 |
| 8 | Candidate Preview 只走 Runtime proxy，固定 4321 | 已实现 | preview lease、proxy URL、process lease 与 screenshot evidence 通过 |
| 9 | Preview proxy 校验 principal、project ownership、lease target、Pod UID | 未实现 | 当前 handler 只校验 lease、binding、sandbox name、Pod UID 和 manifest hash；无 Public API principal |
| 10 | stage -> gates -> CAS，重试无重复 version/event | 已实现 | ArtifactPublishRecord、CAS 和幂等测试覆盖 |
| 11 | current/publish/outbox 原子提交并覆盖 crash recovery | 已实现 | promotion WAL/outbox crash-point 回归通过 |
| 12 | Runtime-owned immutable source snapshot，文本/二进制无损恢复 | 已实现 | immutable snapshot 与 binary archive tests 通过 |
| 13 | Sandbox release 后 Artifact 可访问 | 已实现 | Website/Docs 均返回 200 |
| 14 | 两个并行项目不串 channel、preview、artifact | 部分 | 双 claim Workspace 并行隔离通过；完整 Website/Docs lifecycle 仍串行执行 |
| 15 | 无公网 egress，依赖只经内部 npm proxy | 部分 | NetworkPolicy 和 proxy deployment 存在；gate 没有在 Sandbox 内真实安装依赖 |
| 16 | Runtime/port-forward/Pod 重启后 reconciliation 无孤儿 | 部分 | Channel/Artifact 单测存在；没有 fresh-cluster restart/Pod replacement gate，也没有完整 preview process reconcile evidence |
| 17 | 同一确定性错误不会无限消耗 turn | 已实现 | retry fingerprint、三次上限和 partial 回归覆盖 |
| 18 | preview.updated 早于成功 run.completed | 已实现 | Website/Docs fresh-cluster evidence `sequenceValid=true` |
| 19 | 批准 provider 的 Website/Docs evidence 且无 credential | 未完成 | DeepSeek Build/Edit 只在测试内 Runtime 通过；未在 Deployment 执行，R17 批准状态未提供 |

汇总：11 项已实现，6 项部分完成，2 项未完成。任何“部分”或“未完成”都阻止文档状态改为
`release-ready`。

## 4. 必须补齐的实现

### P0. Public Preview 身份与 ownership

当前 `/previews/{leaseId}` 是未认证 GET。`leaseId` 实际成为 bearer capability，违反文档中
“leaseId 不替代用户鉴权”的硬约束。

必须实现：

1. 定义 Runtime 信任的 Public API principal，不接受客户端可伪造的裸 identity header；
2. principal 由 BFF/identity gateway 通过可验证 service credential 或签名 token 注入；
3. RuntimeStore 持久化 project ownership/scope；
4. proxy 同时校验 principal -> project、project -> run、run -> lease、lease -> binding/Pod UID；
5. HTML 重写后的 asset 请求沿用同一认证方式；
6. 增加 anonymous、wrong project、expired principal、stale lease、Pod replacement 测试。

验收：未认证和跨项目请求返回 401/403；不得用 404 掩盖审计判定；授权请求仍只访问 lease
绑定的 frozen candidate。

### P0. Production channel transport identity

当前 Workspace Channel 为应用层 Ed25519 JWT + `ws://`，NetworkPolicy 只提供网络可达性限制。
原方案要求 production in-cluster 采用 mTLS、service mesh workload identity 或等价机制。

必须实现并固定一种生产拓扑：

- service mesh STRICT mTLS + Runtime/Sandbox service account authorization；或
- Runtime 与 Sandbox 双向 TLS，证书绑定 workload identity 并支持轮换。

k3d 的 loopback port-forward 可以继续允许明文，但 evidence 必须标记 `transport=debug-loopback`，
不能替代 production transport gate。

### P1. 完整并发与 restart gate

需要新增同一 Public Runtime API driver 下的并发测试：Website 与 Docs 同时 Build/Preview/
Promote，并断言 channel lease、candidate URL、screenshot、version 和 artifact manifest 均不串线。

随后在 active lease/publish 存在时依次执行：

1. 重启 Runtime Deployment；
2. 替换一个 Sandbox Pod；
3. 中断并恢复 port-forward；
4. 验证旧 lease stable error、新 lease reacquire、staged publish reconcile 和无孤儿进程。

### P1. npm proxy 真实安装

现有 fixture 用 `node build.cjs`，不会触发 package restore。需要在 Sandbox default-deny
NetworkPolicy 下安装一个未预置的小依赖，并同时证明：

- 请求实际到达 `anydesign-npm-proxy:4873`；
- Sandbox 直接访问 `registry.npmjs.org` 失败；
- lockfile、package manager state 和 build cwd 仍位于 PVC；
- proxy 缓存/上游失败返回 typed recoverable error。

## 5. 必须补齐的 Release Evidence

### P0. Deployed Runtime + approved provider

需要让同一 Kubernetes Deployment 在两种 provider mode 下执行同一 driver：

- fixture：PR/RC 基础 gate；
- approved real provider：Website/Docs Build + Edit，包含 computed-style 和 artifact 检查。

DeepSeek 只有在 R17 数据合规批准后才能作为 release evidence。批准记录必须引用 provider
route、数据分类、日志/retention 与附件策略；API key 不得写入文档、Pod spec、日志或 evidence。

### P0. 统一 evidence-summary

当前 channel、Public Runtime fixture 和 deployed Runtime 分别写不同 JSON；
`run-runtime-rc-gate.sh` 的 deployed evidence 只包含 Runtime version/image 与项目 run URL，
不满足 P4 必填字段。

需要统一 `release-evidence@1` validator，强制交叉校验：

- repository commit/dirty；
- Runtime 与 Sandbox image ref/config digest/reported commit；
- project/run/binding/Pod UID/build/manifest/source snapshot；
- preview/screenshot/CAS/event sequence；
- sandbox release 与 artifact status；
- provider mode/model；
- cluster/context 与 gate timestamps。

缺失字段、dirty candidate、image mismatch、事件逆序、secret scan 命中均必须返回非零。

### P1. Clean candidate 强制失败

当前 RC runner 遇到 dirty worktree 只改变 tag，不会阻止 gate 被误作最终证据。应改为：

```text
RC mode: dirty_count != 0 -> fail before image build
Debug mode: 可显式 ALLOW_DIRTY=1，但 evidence.releaseEligible=false
```

本次 evidence 的 `dirtyFiles=1` 已正确记录，因此只能作为审计证据。

## 6. Clean-cluster Runner 问题

本次从零验证复现了以下 runner 问题：

1. Docker `credsStore: desktop` 无响应时，公共镜像 pull 会卡在 credential helper；
2. controller 首次 pull 超过 `install-controller.sh` 的 120 秒 rollout timeout；
3. Runtime Dockerfile frontend 必须远端解析，部署 gate 无离线闭环；
4. runner 没有预检 buildx/frontend/controller/Verdaccio/k3s/k3d-proxy 镜像和 digest；
5. k3d 默认 Traefik 在 `ImagePullBackOff` 时，项目 gate 仍可通过，产生不必要红色噪声。

建议：为 RC runner 增加锁定的 image manifest、preflight/prefetch/import、registry timeout 分类，
并在创建 k3d 时禁用未使用的 Traefik。基础镜像供应失败应标记 infrastructure failure，不能
标记 Runtime regression，也不能伪装成功。

## 7. 收口顺序

1. Public principal + project ownership + preview proxy auth；
2. production mTLS/workload identity 决策与 gate；
3. full lifecycle concurrency + Runtime/Pod restart reconciliation；
4. Sandbox 内 npm proxy 真实安装；
5. hermetic image preflight 与 Runtime OCI cold build；
6. deployed fixture Website/Docs；
7. R17 批准后 deployed real-provider Website/Docs Build+Edit；
8. clean commit 上生成统一 evidence-summary，secret scan 通过；
9. 只有 validator 全绿后，原方案 front matter 才能改为 `release-ready`。

## 8. 当前判定

```text
Remote Workspace implementation: usable for fixture lifecycle
Kubernetes fixture gate: pass on a newly created cluster
Deployed Runtime RC gate: blocked before image build by frontend supply
Production security closure: blocked
Real-provider release evidence: blocked
Clean RC evidence: blocked
Overall: RC blocked, not release-ready
```
