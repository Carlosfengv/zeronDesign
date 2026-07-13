---
date: 2026-07-13
status: implemented-and-verified
type: implementation-acceptance-report
source: 2026-07-08-runtime-api-freeze.md
---

# Runtime API Freeze Phase B 实施与验收报告

## 1. 结论

`2026-07-08-runtime-api-freeze.md` 所冻结的 Phase B 消费面已经完成代码实现，
当前 worktree 的本地合同门禁、Product/BFF production build、真实 Kubernetes
Sandbox 生命周期、真实 OCI Packaging，以及 Published Runtime Blue/Green 生命周期
均已通过。

本结论表示：Phase B 可以基于冻结契约完成创建、Brief、生成、预览、编辑、权限处理、
不可变版本、一键发布、稳定 Live URL 更新、回滚和下线闭环。它不等于公网 SaaS
Production Ready；正式身份源、公共 DNS/TLS、生产 Registry/KMS 和 HA Store
仍属于部署环境门禁。当前 worktree 的真实 DeepSeek Website/Docs Build/Edit
生命周期也已在本报告完成后补充复验通过。

## 2. 需求到实现证据

| Freeze 要求 | 当前实现 | 验收证据 | 结论 |
|---|---|---|---|
| 可执行 HTTP route inventory | `services/runtime/contracts/http-routes.json` 与 Axum router 一一对应 | `contract_manifest` 5 项通过 | 已完成 |
| Start/Continue/Cancel Run | Runtime handlers、Shared Client、项目归属 BFF；工作台提供启动、继续和停止入口 | Runtime HTTP suite、Shared client test、Web build | 已完成 |
| Structured Brief GET/confirm | 项目级读写授权、幂等确认、Brief UI | Brief HTTP/agent tests、Web build | 已完成 |
| Permission allow/ask/deny | 一次性 updatedInput、schema/deny policy 重校验、Conversation correlation、BFF 权限卡 | Permission engine/HTTP tests、Shared schema test | 已完成 |
| Conversation 与 Runtime State | 项目级读取、默认过滤 debug、Edit 恢复 current version/sandbox | HTTP authorization tests、Web Edit BFF | 已完成 |
| SSE replay | `Last-Event-ID` 透传、Shared event union、终态关闭、浏览器自动重连 | SSE HTTP tests、mock BFF、Shared subscription tests | 已完成 |
| Promoted Preview | current/version metadata 和受保护 BFF proxy | Preview HTTP tests、真实 k3d Website/Docs build/edit | 已完成 |
| 不可变 Version Artifact | current 与 fixed-version 路由、hash/project identity 复核、固定 Review URL | Artifact HTTP tests、Sandbox 释放后读取 E2E | 已完成 |
| Create Release / packaging query | 幂等 application service、supervised controller、恢复查询 | Release packaging HTTP/unit tests | 已完成 |
| OCI build/SBOM/scan/sign | hash-pinned helper、isolated BuildKit、Registry、Syft、Trivy、Cosign | 当前 worktree 的 fresh repository revalidation | 已完成（本地验收） |
| Publish/update/rollback/unpublish | CAS、recoverable Operation、BFF refresh resume | Publication tests、G8 k3d lifecycle | 已完成 |
| 稳定 Live URL | 持久 hostSlug、TLS Ingress、Blue/Green 原子切换 | G8 stable-host update/rollback/fault gate | 已完成（本地 TLS） |
| Project-scoped public principal | Ed25519 JWT、project owner binding、operation scopes、BFF 短 TTL mint | Authorization HTTP tests、真实 k3d public Runtime E2E | 已完成 |
| Shared contract source of truth | API types、Zod schemas、Runtime Client 从 `@zerondesign/shared` 导出 | Shared 29/29、typecheck | 已完成 |
| Phase B Product/BFF | Next.js Product shell、SQLite project/job index、server-only Runtime access | Next production build | 已完成（单实例） |

## 3. 当前 worktree 验收结果

### 3.1 Freeze gate

```text
ANYDESIGN_SKIP_K8S_E2E=1 bash infra/phase-a/verify.sh
exit: 0

Runtime unit: 109 passed
Agent loop: 28 passed; 1 provider-backed test ignored
Astro build agent: 5 passed
HTTP suite: 95 passed; 1 provider-backed test ignored
Release packaging: 19 passed
Shared: 29 passed
Shared typecheck: passed
Phase B Web typecheck: passed
Phase B Web production build: passed
```

完整日志：`/tmp/zerondesign-freeze-final-gate.log`。

### 3.2 真实 Sandbox / Public Runtime

当前 worktree 的 dedicated k3d gate 已通过：

```text
mTLS + pod-bound JWT                         -> passed
Sandbox Claim / Workspace Channel            -> passed
Website & Docs Build -> Edit -> Artifact E2E -> passed
```

证据文件：

- `services/runtime/target/e2e-evidence/k3d-channel-0559656597df.json`
- `services/runtime/target/e2e-evidence/public-runtime-fixture-0559656597df.json`

### 3.3 真实 OCI Packaging

使用新的 repository
`localhost:5001/anydesign/work-releases-freeze-audit` 完整重跑：

```text
status: validated
attempts: 1
imageDigest: sha256:7b7a03b301a91db7d97c9dbb04651f2741f0c128db82152d2ce937e1d0d8d99b
scan: passed; critical=0; high=4; secrets=0
Cosign verified signatures: 1
GET /: 200
GET /.well-known/anydesign/healthz: 200
GET /.anydesign/runtime-manifest.json: 404
container user: 101:101
```

详细证据见 `services/runtime/evidence/work-release-packaging-g4.md`。

### 3.4 Published Runtime G8

```text
test update_rollback_restart_and_failed_switch_restore_blue_on_k3d ... ok
host=w-7c406457803bf7d149e5.g8.test
releaseA=release-6e2fc7bac9ff969db25cfc510aa854c3
releaseB=release-454e623cce1ec899c9ff9b68a550021d
```

该门禁验证同一 HTTPS Host 上的 initial publish、update、rollback、重启恢复、
EndpointSlice 故障回切和幂等重放。详细证据见
`services/runtime/evidence/blue-green-rollback-gc-g8.md`。

## 4. 本轮补齐的问题

1. Freeze gate 不再错误拒绝 `apps/web`，并把 Web typecheck/build 纳入门禁。
2. Shared `AgentRunStatus` 补齐 Runtime 的 `validating` 状态。
3. Shared Conversation kind 补齐 `permission_resolved`。
4. Structured Brief、Create Release、Packaging Query 和 fixed-version Artifact 路由进入正式合同。
5. Public Runtime 路由统一使用 project-scoped principal scopes。
6. Permission Ask/Deny 增加持久 correlation，Product UI 支持 allow/ask/deny。
7. Product/BFF 增加 Cancel Run 路由和工作台停止入口。
8. Release Packaging controller 支持启动恢复和受控并发。
9. Product publication job 可以在刷新后恢复 packaging/publication。
10. Fumadocs deterministic template manifest 更新为当前真实内容 hash。

## 5. 真实 Provider 生命周期复验

用户提供临时 DeepSeek 凭证后，凭证仅通过交互式标准输入注入测试进程，未写入
仓库、环境文件、命令行参数或证据日志。执行：

```text
RUNTIME_E2E_RUN_LOCAL_GATES=0
bash services/runtime/scripts/run-runtime-harness-provider-gates.sh
```

结果：

```text
real_provider_public_runtime_website_and_docs_lifecycle_matrix ... ok
test result: ok. 1 passed; 0 failed; 0 ignored
finished in 244.73s

Website build -> version-73  -> promoted
Website edit  -> version-131 -> promoted; expected text present
Docs build    -> version-243 -> promoted
Docs edit     -> version-302 -> promoted; expected text present
Computed Style --runtime-primary -> expected #f97316, actual #f97316
evidence summary -> ok=true, errors=[]
```

四个阶段均验证 `preview.updated` 先于 `run.completed`、Artifact 可读取、
Edit source snapshot/version 发生变化，且修改内容进入最终 Artifact。

证据目录：`.runtime-evidence/provider-20260713-152016/`：

- `provider-lifecycle.log`
- `computed-style.log`
- `evidence-summary.json`
- `run-metadata.env`

对整个证据目录执行 secret pattern 扫描，结果为
`NO_SECRET_PATTERN_FOUND`。

## 6. 生产部署仍需完成

这些不是 Phase B 冻结合同的代码缺口，但在 Public Production 前必须完成：

- 接入正式身份提供方并签发 Product session；
- 把 Product SQLite 和 Runtime file/journal store 迁移到 HA 持久化服务；
- 使用生产 Registry TLS/auth/retention 和独立 builder identity；
- 使用 KMS/Keyless Cosign identity 及正式 transparency policy；
- 配置公共 DNS、证书签发/轮换、Ingress policy 和监控告警；
- 为 CI/发布环境配置受管的 DeepSeek secret，并建立定期 provider-backed gate。
