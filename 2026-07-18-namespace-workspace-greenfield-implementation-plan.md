# Namespace Workspace 绿地实施计划

> 日期：2026-07-18<br>
> 状态：实施中（第七批对象存储门禁已通过）<br>
> 前提：所有 Kubernetes、k3d、数据库和测试环境均为全新部署，不保留旧状态<br>
> 目标：以 Kubernetes Namespace 作为唯一 Workspace 层级，移除 Organization，并让中央 Runtime 安全管理多个 Workspace

## 实施状态（2026-07-18，第二批）

已完成：

- Phase 0 的核心契约收敛与 `ProjectAccessRecord.workspaceNamespace`；
- Sandbox、Preview、Artifact 与 Publication 控制面的请求级 Namespace 路由；
- Provider Gateway 的 Namespace 租户键；
- Web Workspace 注册表、管理员注册/停用 API，以及 Project 创建的可重试注册 saga；
- Workspace Provisioner 的 Namespace、RBAC、Quota、LimitRange、NetworkPolicy、零副本模板池和开发用 mTLS/SPIFFE 凭证；
- 从空环境创建 `k3d-zerondesign-greenfield`，Provision `ws-greenfield-a` 与 `ws-greenfield-b`；
- 双向冷启动隔离门禁：真实 SandboxClaim、PVC、Pod、Service、跨 Namespace RBAC 拒绝、Runtime 到 Sandbox mTLS 和退出回收；
- 单 Runtime Pod 分别完成 Website（`ws-greenfield-a`）与 Docs（`ws-greenfield-b`）的 Brief、Build、Preview、Edit、Artifact、Release 生命周期；两者 Pod UID 不同，结束后无 Sandbox/PVC 残留；
- 两个 Published Work 已通过独立 Deployment/Service/Ingress 和 HTTPS Release identity 门禁；
- Runtime 控制面缓存已迁移到 PostgreSQL 权威源，并通过本地缓存移走、数据库 Pod 重建和重启后写入验证；
- Artifact、Source Snapshot、Validation/Acceptance Report 和 Screenshot 已迁移到 S3-compatible Object Store 权威源，并通过本地缓存移走和 MinIO Pod 重建验证；
- Runtime 默认 HTTP 集成测试 `111 passed, 0 failed, 3 ignored`。

进行中：Design Profile 创建/导入/审阅的完整管理 UI、平台管理员页面，以及生产基础设施替换和可观测性。

尚未开始：Web 产品目录的生产 PostgreSQL 迁移、托管 PostgreSQL/对象存储接入、生产证书签发器、备份/PITR 和生产监控告警。当前实现是可运行且通过权威源恢复门禁的绿地开发基线，不等于生产上线完成。

## 1. 已锁定架构

- Kubernetes Namespace 是 Workspace 的唯一身份和事实来源；
- 不存在 Organization 层级；
- Project 必须属于一个 Workspace Namespace；
- 第一阶段使用一个中央 Runtime Deployment，`replicas: 1`；
- Runtime Pod 可以并发管理多个 Workspace、Project 和 Run；
- Sandbox、Project PVC、Preview 和发布工作负载位于 Project 所属 Workspace Namespace；
- 首期 Sandbox 按任务创建和回收，不为每个 Workspace 常驻预热 Pod；
- Runtime system、Provider system 不属于任何 Workspace；
- Namespace 对象标签用于 Workspace 注册与发现；
- 中央 Runtime 不创建任意 Namespace，不拥有 cluster-admin；
- k3d 使用单 Runtime Pod + PVC；
- 首个生产环境的控制面元数据直接使用 PostgreSQL；
- 不实现固定 `anydesign-sandboxes`、`anydesign-works` 或 organizationId 的兼容路径。

## 2. Namespace 拓扑

```text
Kubernetes Cluster
├── runtime-system
│   ├── Runtime Deployment (replicas: 1)
│   ├── Runtime Service
│   └── local/k3d evidence PVC
├── provider-system
│   ├── Provider Gateway
│   └── Provider PostgreSQL / external PostgreSQL connection
├── ws-<workspace-id-a>
│   ├── Project SandboxClaims / Pods
│   ├── Project PVCs
│   ├── Preview resources
│   ├── Published Work Deployments / Services / Ingresses
│   └── Workspace policies, quota and service accounts
└── ws-<workspace-id-b>
    └── same resource classes, isolated from workspace A
```

逻辑名称 `runtime-system` 可以映射到最终实际 Namespace 名称；一旦首个环境部署后不得随意更名。

## 3. Workspace Namespace 规范

### 3.1 身份

Workspace ID 直接等于 Namespace `metadata.name`。

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: ws-01hxyz
  labels:
    zerondesign.dev/workspace: "true"
    app.kubernetes.io/managed-by: zerondesign-workspace-provisioner
```

命名要求：

- 使用 `ws-<stable-id>`；
- 符合 Kubernetes DNS label 限制；
- 不使用可变显示名称作为 Namespace 名；
- UI 单独保存 Workspace display name；
- Namespace 重命名不作为普通操作，采用新建并迁移 Project 的独立流程。

### 3.2 Workspace Provisioner

只有 Workspace Provisioner 可以创建或销毁 Workspace Namespace。它负责创建：

- Namespace 与标准 labels；
- ResourceQuota；
- LimitRange；
- 默认拒绝 NetworkPolicy；
- Runtime ServiceAccount 的 namespace-scoped RoleBinding；
- Sandbox ServiceAccount；
- Workspace channel verifier、证书和必要 Secret；
- Template SandboxTemplate；
- Preview 与 Publication 所需 NetworkPolicy；
- Ingress/TLS 配置引用。

Runtime 只验证 Namespace 已带 Workspace label 且 RBAC 可用，不自行提升权限。

首期 Workspace Provisioning 不创建常驻的 warm Sandbox Pod。Agent Sandbox `v1beta1` 要求 Claim 引用 `SandboxWarmPool`，因此 Provisioner 会创建 `replicas: 0` 的池对象作为模板间接引用；它不产生预热 Pod。收到 Run 后，Runtime 才在 Project 所属 Workspace Namespace 冷启动 Sandbox；任务完成、取消或超时后按生命周期策略清理。可通过节点镜像预拉取、k3d 镜像导入和镜像缓存降低冷启动时间，这些优化不创建租户常驻 Sandbox。

### 3.3 删除

Workspace Namespace 删除是高风险管理操作，必须：

1. 阻止新的 Run、Publish 和绑定写入；
2. 列出 Project、PVC、Release 和公开域名；
3. 完成需要保留的 Artifact、Audit 和 Source 导出；
4. 下线 Publication；
5. 删除 Runtime RoleBinding；
6. 最后删除 Namespace。

MVP 可以不提供删除 UI，但不能通过普通 Project 删除隐式删除 Namespace。

## 4. 资源放置规则

| 资源 | Namespace/存储位置 | 原因 |
| --- | --- | --- |
| Runtime Deployment | Runtime system | 中央控制面 |
| Runtime/Web PostgreSQL | 独立数据系统或托管数据库 | 产品目录、控制面事务与备份 |
| Provider Gateway | Provider system | 凭证与模型边界隔离 |
| Platform Design Profile | Runtime PostgreSQL | 跨 Workspace 可见 |
| Project Design Profile | Runtime PostgreSQL，带 workspace + project scope | 项目隔离与查询 |
| Sandbox Pod/Claim | Workspace Namespace | 租户资源隔离 |
| Project PVC | Workspace Namespace | 生命周期与配额归属 |
| Preview | Workspace Namespace | 与 Project 资源同域 |
| Published Work | Workspace Namespace | 配额、网络与清理归属 |
| Build/Artifact blob | 对象存储或 Workspace PVC | 大文件不进入控制面数据库 |
| Audit/Event evidence | PostgreSQL + 日志/对象存储 | 可查询且可长期保留 |

## 5. Project 与请求身份

ProjectAccessRecord 至少包含：

```ts
type ProjectAccessRecord = {
  projectId: string;
  ownerPrincipalId: string;
  workspaceNamespace: string;
};
```

规则：

- `workspaceNamespace` 必填；
- 不包含 organizationId；
- Runtime 从 ProjectAccessRecord 解析 Workspace；
- BFF、Run request、Query 参数中的 workspaceId 不能覆盖服务端绑定；
- K8s API 操作同时使用 namespace 和 project label；
- Project ID 保持全局唯一；
- Provider Gateway `workspaceId` 等于 namespace 名称。

## 6. Contract 调整

绿地部署直接进行破坏式 Schema 收敛，不增加兼容层：

- Shared API 移除 organizationId；
- Runtime StartRun input 不接受客户端指定 organizationId；
- Runtime ProjectAccessRecord 移除 organizationId；
- Provider Gateway TurnScope 移除 organizationId；
- ModelSelectionPolicy.scope 移除 organizationIds；
- Workspace ID 的格式约束与 Kubernetes Namespace 一致；
- Design Profile 只支持 project 或 platform scope；
- 版本号按各 Contract 的现有版本策略递增。

所有调用方、Fixture、JSON Schema 和测试在同一个变更集中更新，不维护双版本运行期分支。

## 7. Runtime Namespace 路由

当前固定 `K8S_NAMESPACE` 和 `WORKS_NAMESPACE` 要替换为请求级 Workspace 路由。

目标调用链：

```text
Authenticated request
  -> projectId
  -> ProjectAccessRecord
  -> workspaceNamespace
  -> validate Namespace workspace label
  -> namespace-scoped K8s client operation
```

必须覆盖：

- Template readiness；
- Sandbox claim 与 workspace channel；
- Project PVC；
- Preview；
- Release protection；
- Publication Deployment、Service、Ingress、NetworkPolicy；
- Garbage collection；
- k3d/e2e helper scripts。

固定 Namespace 常量只能用于 Runtime system 或 Provider system，不能用于 tenant 资源。

## 8. RBAC 与网络

### 8.1 RBAC

- Workspace Provisioner 拥有创建 Namespace 和安装 RoleBinding 的受控权限；
- Runtime ServiceAccount 通过每个 Workspace 中的 RoleBinding 获得最小权限；
- Runtime 不拥有 cluster-admin；
- Workspace 用户不直接获得 Kubernetes API 权限；
- 新 Workspace 注册失败时不得部分开放 Runtime 权限；
- RBAC 变更写入平台审计。

### 8.2 网络

每个 Workspace 默认拒绝入站和出站，只显式允许：

- Runtime 到 workspace channel；
- Sandbox 到内部 npm proxy；
- Preview/Published Work 到允许的 Ingress；
- DNS；
- 经策略批准的外部访问。

Workspace A 的 Pod、Service 和 PVC 不应被 Workspace B 直接访问。

## 9. 存储

### 9.1 k3d/local

- Runtime `replicas: 1`；
- PVC `ReadWriteOnce`；
- JSONL 可用于验证恢复逻辑；
- 每次测试使用新集群，不执行数据迁移；
- 测试结束删除整个集群。

### 9.2 production

首个生产环境直接使用 PostgreSQL 保存：

- Web Project 目录、所有权和发布任务恢复索引；
- ProjectAccess；
- Run identity 与状态；
- Design Profile、Draft、revision；
- Project Profile Binding；
- Brief、Version metadata；
- Permission、Audit、Outbox；
- Release/Publication control state。

Source、Artifact、Screenshot 和构建日志使用对象存储或专用 Artifact Store。JSONL 不作为生产控制面事实来源。

Runtime 第一阶段仍可保持一个 Pod；PostgreSQL 是数据可靠性和未来横向扩展的前置，不代表必须立即扩容 Runtime。

Web 开发环境可以继续使用 SQLite，但首个生产部署不使用 SQLite 作为 Project 目录事实来源。Web 与 Runtime 可以使用同一 PostgreSQL 集群中的独立数据库或独立 Schema，所有权边界保持不变。

## 10. k3d 全新部署流程

```text
create k3d cluster
  -> install CRDs and controllers
  -> deploy Runtime system
  -> deploy Provider system
  -> deploy Workspace Provisioner
  -> provision ws-a and ws-b
  -> create Project in each Workspace
  -> run Build/Edit/Preview/Publish
  -> verify isolation and platform Profile visibility
  -> restart Runtime and verify recovery
  -> delete cluster
```

测试脚本要求：

- 不依赖预先存在的 `anydesign-sandboxes` 或 `anydesign-works`；
- Namespace 通过 Provisioner 创建；
- 脚本使用返回的 Namespace ID，不在后续命令硬编码；
- 至少两个 Workspace 同时存在；
- 所有清理通过删除 k3d cluster 完成；
- 失败时保留诊断 bundle，但不复用失败集群作为下一次测试输入。

## 11. 实施阶段

### Phase 0：Contract 收敛

- 移除 organizationId；
- Workspace ID 使用 Namespace 格式；
- 更新 Rust、TypeScript、JSON Schema、Fixture 和测试。

### Phase 1：Workspace Provisioner

- Namespace labels；
- Quota、LimitRange、NetworkPolicy；
- RBAC；
- Sandbox templates 与证书；
- 不创建常驻 Sandbox warm pool；
- 幂等创建和失败清理。

### Phase 2：Runtime 动态 Namespace 路由

- ProjectAccess 解析；
- Sandbox/Preview/PVC；
- Publication/Release/GC；
- 不可伪造 Workspace。

### Phase 3：生产存储

- PostgreSQL schema；
- 事务边界与 revision；
- Outbox、备份与恢复；
- Artifact/Object Store 边界。

### Phase 4：k3d 绿地 E2E

- 空集群部署；
- 两 Workspace；
- 跨 Workspace 隔离；
- Runtime 重启；
- 完整生成与发布。

## 12. 测试矩阵

| 场景 | 期望 |
| --- | --- |
| 未带 Workspace label 的 Namespace | Runtime 拒绝使用 |
| 请求伪造 workspaceId | 按 ProjectAccessRecord 处理并拒绝跨域 |
| ws-a Project 创建 Sandbox | 资源只出现在 ws-a |
| ws-a principal 读取 ws-b Project | 404 |
| ws-a Pod 访问 ws-b Service | NetworkPolicy 拒绝 |
| Runtime 缺少 ws-a RoleBinding | 操作失败且不回退 cluster 权限 |
| Workspace Provisioner 重试相同请求 | 幂等返回同一 Workspace |
| Provisioning 中途失败 | 清理或标记可恢复，不留下可用半成品 |
| 两 Workspace 并发 Build | 均成功且资源不串域 |
| Runtime 重启 | 控制面状态从存储恢复 |
| 平台 Profile | ws-a、ws-b 都可见但用户不可修改 |
| 项目 Profile | 仅所属 Project 可见 |
| Publish | Deployment/Service/Ingress 位于正确 Workspace |

## 13. 验收标准

1. Namespace 是唯一 Workspace ID；
2. 所有产品 Contract 不包含 organizationId；
3. Runtime 不依赖固定 tenant Namespace 常量；
4. 中央 Runtime 一个 Pod 可以服务至少两个 Workspace；
5. Runtime 不拥有 cluster-admin；
6. Workspace Provisioning 幂等且可恢复；
7. Sandbox、PVC、Preview、Publication 全部正确落位；
8. 跨 Workspace 授权与网络测试全部拒绝；
9. k3d 从空集群完成全流程；
10. 首个生产部署使用 PostgreSQL 作为控制面事实来源；
11. Runtime 重启不丢失 Project、Run、Profile、Binding 与发布状态；
12. 不存在旧 Schema 或旧 Namespace 的兼容代码路径。

## 14. 剩余部署参数

这些是环境配置，不改变架构：

- Runtime system Namespace 最终名称；
- Workspace 默认 ResourceQuota；
- Base domain、IngressClass 与 TLS issuer；
- PostgreSQL 托管或集群内部署；
- Artifact Store 实现；

Sandbox warm pool 不再是首期部署参数。只有监控数据证明按需创建造成不可接受的用户等待后，才单独设计启用；届时至少需要定义启用范围、最小/最大空闲数、闲置缩容、清理隔离和容量成本。

## 15. 第二批绿地证据与下一门禁

当前开发集群保留在 kube context `k3d-zerondesign-greenfield`，用于继续实现；本批未删除集群。可重复执行的隔离门禁为：

```bash
bash infra/workspace-provisioner/verify-workspace-isolation.sh \
  ws-greenfield-a ws-greenfield-b
```

生命周期证据位于 `services/runtime/target/e2e-evidence/zerondesign-greenfield/`：

- `k3d-channel.json`：Namespace、RBAC、冷启动资源隔离和 mTLS/SPIFFE 证据；
- `real-provider-website.json`：fixture Provider 下 Website 绑定 `ws-greenfield-a` 的完整生命周期证据；
- `real-provider-docs.json`：fixture Provider 下 Docs 绑定 `ws-greenfield-b` 的完整生命周期证据。

文件名沿用现有 RC gate 的历史命名，但文件内 `provider.mode` 已准确记录为 `fixture`，不冒充真实 Provider 验证。

下一门禁按以下顺序推进：

1. 在同一 Runtime 实例上让 ws-a Website 与 ws-b Docs 同时 Build，验证并发不串租；
2. 在 Preview 活跃期间重启 Runtime，验证 SQLite/PVC 开发存储恢复；
3. 将 Release 发布为 Workspace 内真实 Deployment、Service、NetworkPolicy、Ingress，并验证 ws-a/ws-b 落位；
4. 再进入 PostgreSQL、对象存储和生产证书签发器实现。

## 16. 第三批 Published Work 与 URL 证据

第三门禁已经完成开发环境验收：

- Workspace Provisioner 为每个 Namespace 安装 Release Prober ServiceAccount、最小 RBAC、Namespace 读取授权和 Prober Egress；
- Publication Kubernetes backend 接受 `zerondesign.dev/workspace=true` 作为发布边界，同时保留旧 G6 测试 Namespace 兼容；
- Website `version-640` 与 Docs `version-475` 的真实生成 Artifact 被并行打包成 digest-qualified 静态运行镜像；
- Publication Controller 在 `ws-greenfield-a` 和 `ws-greenfield-b` 分别创建真实 Deployment、稳定 ClusterIP Service 和项目级 NetworkPolicy；
- 新 Controller 实例重新协调后 Deployment UID 保持不变；实际 Runtime Deployment 重启期间两个作品 URL 持续返回 200；
- 外部返回的 HTML SHA-256 与 Runtime Artifact 原文件完全一致。

开发环境 URL：

- Website：`http://website.zerondesign.localhost:18080/`
- Docs：`http://docs.zerondesign.localhost:18080/`

证据文件为 `services/runtime/target/e2e-evidence/zerondesign-greenfield/published-works.json`，可重复入口为：

```bash
bash infra/workspace-provisioner/run-published-work-e2e.sh
```

上述 URL 使用标记为 `zerondesign-e2e` 的 HTTP Ingress，只用于本地可打开的验收入口。生产发布仍要求现有 G7 HTTPS 外部 Release 身份探测；因此证据中的控制面状态诚实记录为 `workload_ready`，不伪造生产 `Published` 状态。

剩余门禁调整为：

1. 同一 Runtime 上真正并发执行两个新 Build；
2. Preview 活跃期间重启 Runtime 并恢复 Lease；
3. 在 Workspace 模型上接通 G7 TLS issuer、HTTPS 外部探测并达到控制面 `Published`；
4. PostgreSQL、对象存储和生产证书签发器。

## 17. 第四批并发 Build 与 Preview 恢复证据

前两项剩余门禁已经完成：

- `rc-website-1784370776-fixture` 在 `ws-greenfield-a`、`rc-docs-1784370776-fixture` 在 `ws-greenfield-b`；
- 两个 Build 在 `2026-07-18T10:33:45Z` 至 `10:34:33Z` 的同一窗口并发执行；
- 两个项目使用不同 Sandbox Pod UID、Preview Lease、Artifact manifest 和最终 Version；
- 独立恢复项目在 Preview Lease 活跃时触发 Runtime rollout；
- Runtime Pod UID 从 `275cdffd-0fc9-4eb2-8ca0-f995ad2a631b` 变为 `900653e5-0002-429e-9dca-c8bdb289abba`；
- 重启后原 Preview 返回 200，替换本地 port-forward 后仍返回 200；
- 证据中的 orphan count 为 0，结束后两个 Workspace 均无遗留 SandboxClaim。

并发与恢复证据：

`services/runtime/target/e2e-evidence/zerondesign-greenfield/concurrent/workspace-concurrency-recovery.json`

RC gate 新增以下显式参数，避免通过全局变量把两个 Project 错放进同一 Namespace：

```text
RUNTIME_RC_WEBSITE_WORKSPACE_NAMESPACE
RUNTIME_RC_DOCS_WORKSPACE_NAMESPACE
RUNTIME_RC_CONCURRENT_WORKSPACE_GATE=1
```

新生成 Artifact 随后被发布到原有本地 URL。Release ID 现在由真实 Artifact manifest hash 与 Runtime manifest hash 派生，不再使用固定测试 marker：

- Website：`http://website.zerondesign.localhost:18080/`
- Docs：`http://docs.zerondesign.localhost:18080/`

当前剩余主门禁只有：

1. Workspace 模型上的 G7 TLS issuer 与 HTTPS 外部 Release 身份探测；
2. PostgreSQL 控制面存储；
3. Artifact/Object Store；
4. 生产证书签发与轮换。

## 18. 第五批 Workspace HTTPS 与 Published 证据

G7 HTTPS 发布门禁已经在保留的绿地 k3d 集群完成：

- k3d LoadBalancer 将本地 `18443` 映射到 Traefik `443`；
- 使用 30 天有效期的本地测试 CA 为 `*.works.zerondesign.localhost` 签发通配证书；
- 同名 TLS Secret 分别安装在 `ws-greenfield-a` 与 `ws-greenfield-b`，Namespace 是唯一 Workspace 层级；
- Publication Controller 为两个作品创建独立 HTTPS Ingress，控制面状态均达到真实 `published`；
- 外部 HTTPS 探测同时验证根页面、`/.well-known/anydesign/release`、Release Header 和响应体中的 Release ID；
- Publication Controller 重建和实际 Runtime Deployment rollout 后，两个 HTTPS URL 均再次通过验证；
- 本地证据只保留 CA 公钥证书，CA 私钥不落盘；服务端私钥仅保存在各 Workspace 的 Kubernetes TLS Secret 中，不写入证据目录。

当前 HTTPS 作品 URL：

- Website：`https://w-757426b17958cc0298d1.works.zerondesign.localhost:18443/`
- Docs：`https://w-72fc89da0bef634cc797.works.zerondesign.localhost:18443/`

对应 Release：

- Website `version-903`：`release-84316dc896db015667343eabab5e43de`；
- Docs `version-969`：`release-9b7e8a98bb0b479fa776fdd767b2b473`。

可重复入口：

```bash
bash infra/workspace-provisioner/run-published-work-g7-e2e.sh
```

验收证据为 `services/runtime/target/e2e-evidence/zerondesign-greenfield/published-works-tls.json`，公开测试 CA 为 `services/runtime/target/e2e-evidence/zerondesign-greenfield/published-works-test-ca.crt`。因为该 CA 未自动加入操作系统信任库，浏览器直接打开 URL 可能显示证书警告；自动化验收通过显式指定该 CA 验证证书链，不会关闭 TLS 校验。

当前剩余生产化主门禁为：

1. 将控制面事实来源从开发用 SQLite/PVC 切换到 PostgreSQL；
2. 将生成 Artifact 接入持久化 Artifact/Object Store；
3. 将本地测试 CA 替换为生产证书签发器，并实现续期、轮换和失败告警。

## 19. 第六批 PostgreSQL 控制面事实来源

PostgreSQL 控制面门禁已经在保留的绿地 k3d 集群完成：

- `DATABASE_URL` 在 production profile 下必须使用 `postgres://` 或 `postgresql://`，SQLite 配置会在启动校验阶段失败；
- Runtime 增加 `runtime_control_plane_files` PostgreSQL Schema，保存路径、完整内容、SHA-256、revision 与更新时间；
- Project、Run、Profile、Binding、Lease、Publication、Release、Outbox、审计等现有领域日志和检查点继续作为本地缓存，同时每次写入后同步提交 PostgreSQL；
- Runtime 启动时若数据库已有记录，会先清理不属于数据库版本的本地控制面缓存，再按 SHA-256 校验恢复；若数据库为空，则只执行一次现有缓存导入；
- 生成 Artifact 与 Design Source blob 明确排除在数据库边界之外，继续留在 PVC，等待下一批 Object Store 实现；
- PostgreSQL 密码和连接串只保存在 Kubernetes Secret `anydesign-runtime-postgres`，验收证据不包含凭据；
- 本次迁移共持久化 `318` 个控制面文件。

权威恢复验证过程：

1. Runtime 将现有控制面缓存导入 PostgreSQL；
2. Runtime 缩容后，数据库记录对应的本地缓存文件被移动到 PVC 可恢复备份目录，不删除 Artifact；
3. Runtime 重新启动，仅依赖 PostgreSQL 恢复控制面缓存；
4. 恢复后的 `project-access.jsonl` SHA-256 与数据库记录一致：`1cf2d3ee5da3a02a1ee124977e6372833fcaed5b2b94793d5b90fe5ef9093a93`；
5. PostgreSQL Pod UID 从 `d3fa4414-4562-4373-967a-3b3d2907346a` 变为 `6941401e-7504-4dbf-bf2c-1e2193882e1d` 后，数据仍完整；
6. PostgreSQL 重启后的首次内部 ProjectAccess 写入成功，`audit-log.jsonl` 数据库 revision 从 `2` 增至 `3`；
7. 两个 Workspace 均无遗留 SandboxClaim，两个 Published Work 的 HTTPS 页面与 Release 身份继续通过验证。

可重复入口：

```bash
bash infra/workspace-provisioner/run-postgres-control-plane-e2e.sh
```

验收证据：`services/runtime/target/e2e-evidence/zerondesign-greenfield/postgres-control-plane.json`。

当前 HTTPS 作品 URL 保持不变：

- Website：`https://w-757426b17958cc0298d1.works.zerondesign.localhost:18443/`
- Docs：`https://w-72fc89da0bef634cc797.works.zerondesign.localhost:18443/`

当前剩余生产化主门禁为：

1. 将 Artifact、Source Snapshot、Validation/Acceptance Evidence 接入持久化 Object Store；
2. 将本地 PostgreSQL StatefulSet 替换为生产级 PostgreSQL 服务，补充备份、PITR、监控和连接池参数；
3. 将本地测试 CA 替换为生产证书签发器，并实现续期、轮换和失败告警。

## 20. 第七批 S3-compatible 对象存储事实来源

对象存储门禁已经在保留的 `k3d-zerondesign-greenfield` 集群完成：

- Runtime production profile 要求 `OBJECT_STORAGE_URL` 使用 `s3://`，并要求 endpoint 和凭据；HTTP endpoint 仅允许通过显式的 `OBJECT_STORAGE_ALLOW_HTTP=true` 用于本地集群；
- 本地门禁部署单副本 MinIO StatefulSet 和 4 GiB PVC，桶为 `anydesign-runtime`，前缀为 `greenfield`；生产环境可替换成任意 S3-compatible 托管服务；
- 对象边界严格限制为 `artifacts/`、`source-snapshots/`、`validation-reports/`、`acceptance-reports/`、`screenshots/`；PostgreSQL 控制面缓存、Workspace 文件和凭据均不进入对象桶；
- Runtime 首次启动在远端前缀为空时导入现有对象；远端已有对象时以远端为权威，清理不属于远端版本的本地对象缓存并恢复内容；
- 远端前缀包含独立初始化标记；即使业务对象合法删除为零，也不会被误判成首次导入并从本地残留复活；删除路径先确认远端成功，再删除本地缓存；
- Artifact stage/promote/abort/GC、Source Snapshot、Validation/Acceptance Report、普通截图和浏览器验证截图都在写入后同步到对象存储；
- 凭据仅保存在 Kubernetes Secret `anydesign-runtime-object-storage`，验收证据不包含凭据；
- Docker 离线依赖目录改为 `cargo vendor --versioned-dirs`，避免多个 reqwest 版本在 Docker 构建上下文中的目录/校验冲突；Runtime 显式选择 rustls ring provider，消除 S3 与 Kubernetes TLS 依赖同时启用多个 provider 时的启动歧义。

权威恢复验证过程：

1. Runtime 将本地五类对象缓存导入 MinIO，共 `468` 个对象：Artifact `28`、Source Snapshot `368`、Validation Report `12`、Acceptance Report `12`、Screenshot `48`；
2. Runtime 缩容后，五类本地目录被移动到 `/var/lib/anydesign-runtime/data/object-storage-e2e-cache-backup-1784378168`，保留可恢复备份；
3. Runtime 重新启动，仅依赖 MinIO 恢复全部 `468` 个对象；
4. Website Artifact tree SHA-256 在迁移前、缓存恢复后、MinIO 重建后均为 `3fdc0cc5c2b323154c1bacd7d569df918aeac3d8d039e1a8483bad1b07797114`；
5. Docs Artifact tree SHA-256 三次均为 `7ac3242550eb5c4d332571f9cc1d4d81b80db98dc9011d63bd11db2388130ad7`；
6. MinIO Pod UID 从 `2ea8f548-e4d9-438b-bb43-5877ba66d5a6` 变为 `d35f1548-0bff-48b6-86db-f4d95970c527` 后，PVC 数据和 Runtime 恢复仍完整；
7. 两个 Published Work 的 HTTPS 页面与 Release identity 继续通过验证。

可重复入口：

```bash
bash infra/workspace-provisioner/run-object-storage-e2e.sh
```

验收证据：`services/runtime/target/e2e-evidence/zerondesign-greenfield/object-storage.json`。

当前 HTTPS 作品 URL 保持不变：

- Website：`https://w-757426b17958cc0298d1.works.zerondesign.localhost:18443/`
- Docs：`https://w-72fc89da0bef634cc797.works.zerondesign.localhost:18443/`

当前剩余生产化主门禁为：

1. 将 k3d 内的 MinIO/PostgreSQL StatefulSet 替换为生产级托管服务，补充备份、PITR、版本控制、生命周期策略、监控和告警；
2. 将 Web 产品目录迁移到生产 PostgreSQL，并完成 Web/Runtime 数据所有权和事务边界的部署验证；
3. 将本地测试 CA 替换为生产证书签发器，并实现续期、轮换和失败告警。

## 21. 第八批 Web 产品目录 PostgreSQL 与事务边界

Web 产品目录 PostgreSQL 门禁已经在保留的 `k3d-zerondesign-greenfield` 集群完成：

- Web 的 Workspace、Project、Run 索引、Version 索引和 Publication Job 已统一支持异步 PostgreSQL 存储；非 production 开发环境仍可使用本地 SQLite；
- production profile 必须提供 `ZERONDESIGN_PRODUCT_DATABASE_URL`，且只接受 `postgres://` 或 `postgresql://`；缺失或使用 SQLite 时健康检查失败；
- Schema 通过 `apps/web/migrations/0001_product_catalog.sql` 显式部署并记录 `product-catalog@1`，production 禁止运行时自动迁移；
- Web 使用独立数据库 `zerondesign_web` 和 DML-only 应用角色 `zerondesign_web`，与 Runtime 的 `anydesign_runtime` 数据库明确分离所有权；
- `projects.workspace_namespace` 外键引用 `workspaces.namespace`；项目注册先在数据库事务中锁定并校验 active Workspace，再提交 `registering` 状态，之后才调用 Runtime；
- Web 容器以非 root 用户运行，readiness/liveness 都通过 `/api/health` 校验真实 PostgreSQL 和 Schema version；
- 镜像构建通过仓库 `.dockerignore` 排除宿主机 `node_modules`、`.next` 和本地 SQLite，同时保留 Runtime 离线构建所需的 `services/runtime/target/docker-vendor`。

绿地迁移与事务验证过程：

1. 当前环境不存在旧 Web SQLite 文件，因此没有伪造 legacy import；门禁从 `published-works-tls.json` 将两个现有 Published Project 引导写入新目录；
2. 首次引导后用户目录返回 Website 和 Docs 两个项目，Project 与 Workspace Namespace 对应关系正确；后续门禁重跑保留成功创建的 Saga 项目，不清空数据库来伪造绿地结果；
3. 最终重跑将 `ws-greenfield-b` 标记为 disabled 后创建项目返回 `403`，Web 项目总数保持 `4`，Runtime `project-access.jsonl` revision 保持 `4`，证明失败在 Web 事务边界内终止；
4. 重新启用 Workspace 后成功创建 Draft Project `99dd6646-36b9-413d-9da1-887a72edb829`，Web 项目总数从 `4` 增为 `5`，Runtime revision 从 `4` 增为 `5`，两侧均包含该注册；
5. PostgreSQL Pod UID 从 `8bdda7cc-81b6-4cd2-9b01-605e1ab0eaa7` 变为 `c06811ee-ca9d-4dbd-8694-635a41064a83`；Web Pod UID 从 `6867fda6-4965-498d-ae88-39ace5c97a97` 变为 `b7c13f32-f8e3-418e-a5b7-8800b99e8e2c`；
6. 重启前后有序目录 SHA-256 均为 `2171c3b60b145cd05f9bbf7528eb0c17132c8d0b9cc772a13d14e5619c10a744`，Web Pod 内不存在 `.data/product.sqlite`；
7. 两个 Published Work 的 HTTPS 页面、Release Header 和 Release identity 再次通过验证。

可重复入口：

```bash
bash infra/workspace-provisioner/run-web-product-postgres-e2e.sh
```

验收证据：`services/runtime/target/e2e-evidence/zerondesign-greenfield/web-product-catalog-postgres.json`。

当前 HTTPS 作品 URL 保持不变：

- Website：`https://w-757426b17958cc0298d1.works.zerondesign.localhost:18443/`
- Docs：`https://w-72fc89da0bef634cc797.works.zerondesign.localhost:18443/`

当前剩余生产化主门禁为：

1. 将 k3d 内 PostgreSQL/MinIO 替换为生产级托管服务，补充备份、PITR、对象版本控制、生命周期策略、监控与告警；
2. 将本地测试 CA 替换为生产证书签发器，并实现续期、轮换和失败告警；
3. 为 Web 产品目录补充迁移发布流水线、连接池容量基线和数据库不可用告警。

## 22. 第九批 cert-manager Workspace 证书与轮换

Workspace HTTPS 证书生命周期门禁已经在保留的 `k3d-zerondesign-greenfield` 集群完成：

- 安装 cert-manager controller、cainjector 和 webhook；当前 k3s 为 `v1.31.5+k3s1`，因此本地门禁固定使用最后兼容并包含安全修复的 cert-manager `v1.19.5`；
- 平台签发接口固定为 `ClusterIssuer/zerondesign-works-ca`；k3d 使用内部 CA 实现这一接口，生产必须替换为 ACME、Vault 或云厂商 CA；
- CA 根私钥只保存在 `cert-manager/zerondesign-works-root-ca` Secret，不复制到 Workspace，也不写入证据目录；
- Runtime 为每个 Published Work Ingress 添加 `cert-manager.io/cluster-issuer` 注解，并引用按作品派生的 `<work-name>-tls` Secret；
- ingress-shim 为每个作品创建同名、只包含该作品精确域名的 Certificate；每个 Certificate 独立生成 ECDSA P-256 私钥，写入作品所在 Namespace；
- 证书有效期为 `2160h`，提前 `720h` 续期，`rotationPolicy=Always`，保留最近 3 个 CertificateRequest revision；
- 不再为同一基础域名在多个 Namespace 创建重复通配符证书，避免 Traefik 在相同 SNI 覆盖范围中选择错误 Secret；
- Runtime ServiceAccount 无权读取两个 Workspace 的 TLS Secret，`ws-greenfield-a` Sandbox ServiceAccount 也无权读取 `ws-greenfield-b` TLS Secret；
- 两个 Workspace 的私钥和叶证书均不同，仍由同一个平台信任根签发。

主动轮换与恢复验证过程：

1. 使用 `cmctl renew` 同时触发两个 Workspace 的 Certificate 续期；
2. `ws-greenfield-a/work-a80d46a5cc83-tls` revision 从 `1` 增为 `2`，叶证书 SHA-256 fingerprint 从 `4b7d0417059a64457ace66d9940bf7d17dc824536604a9e22235b683d42602f6` 变为 `28ec65bb324b1f0d1652862c32909300a3e12611de5f7d835e43bfab25e355e4`；
3. `ws-greenfield-b/work-ff850395e7eb-tls` revision 从 `1` 增为 `2`，fingerprint 从 `391d01c39b3f7c17c660c509a33fd48f56ace35bc0781e9a02741e13bbbc0dbd` 变为 `2d74d2ca382024c944de0838600e99f956771467632f38ae422fd6c04a04fb05`；
4. 两次续期均同时轮换私钥；测试以 SNI 连接逐个确认 Traefik 实际返回的叶证书 fingerprint 与对应 Namespace Secret 完全一致，同时验证页面、Release Header 和 Release identity；
5. cert-manager controller Pod UID 从 `4ce29649-eaf1-4d51-9322-fc88910de3aa` 变为 `e59fc860-b9b0-4c2e-9078-e1271445cf30` 后，ClusterIssuer 和两个 Certificate 均恢复 Ready；
6. 新 CA fingerprint 为 `1e852eec48897ce666d333d0b3f3f948c345c10a54a47c113a4c9afc21520a0d`，私钥未进入证据目录。

可重复入口：

```bash
bash infra/workspace-provisioner/run-cert-manager-tls-e2e.sh
```

验收证据：`services/runtime/target/e2e-evidence/zerondesign-greenfield/cert-manager-workspace-tls.json`。`published-works-tls.json` 已同步更新为 `cert-manager-local-ca-per-work` 模式，后续 PostgreSQL、Object Store 和 Web 门禁继续使用同一个 CA 文件验证 HTTPS，不关闭 TLS 校验。Ingress 是 Certificate 的 owner，Certificate 是 TLS Secret 的 owner；删除作品 Ingress 会沿 OwnerReference 清理其证书和私钥。

当前 HTTPS 作品 URL 保持不变：

- Website：`https://w-757426b17958cc0298d1.works.zerondesign.localhost:18443/`
- Docs：`https://w-72fc89da0bef634cc797.works.zerondesign.localhost:18443/`

当前剩余生产化主门禁为：

1. 生产 Kubernetes 升级到受支持版本，并将本地 CA ClusterIssuer 替换为真实 ACME、Vault 或托管 CA；
2. 将 k3d 内 PostgreSQL/MinIO 替换为生产级托管服务，完成备份、PITR、对象版本控制、监控和告警；
3. 使用真实 AI Provider 完成 Website/Docs 生成、限流、超时、账单归属和凭据隔离门禁。

## 23. 第十批真实 AI Provider 与生成作品验证

真实 DeepSeek Provider 的最小连通性、治理接入、Website/Docs 生成和验证发布已经在保留的 `k3d-zerondesign-greenfield` 集群完成：

- `provider-system` 部署独立 Provider Gateway；Runtime 只持有 Gateway bearer，DeepSeek API Key 只存在于 Provider Gateway Secret 挂载文件中；Runtime 与 Workspace Sandbox ServiceAccount 均无权读取 Provider Secret；
- `deepseek-v4-pro` Model Resource revision 为 `4`，Selection Policy revision 为 `3`；真实最小调用返回非估算 Token usage，并确认 Provider request id 存在，但证据只记录布尔值，不持久化其原值；
- 当前本地 Gateway 使用单副本 SQLite PVC，满足 k3d 验证持久化，但不代表生产完成；生产仍需 PostgreSQL、加密密钥、至少两副本及备份/监控；
- 真实运行入口强制接收 `GENERATION_REAL_WORKSPACE_NAMESPACE`，项目访问记录和证据均保存唯一的 `ws-*` Namespace，不再存在额外 Workspace 层级；
- macOS LibreSSL 不支持 Ed25519 的环境差异已由 Runner 自动探测，并回退到可用的 OpenSSL 3；
- 实际 Provider request id 已从 JSON/NDJSON 证据中清除，只保留 `providerRequestIdPresent=true`；测试 Key 未写入仓库或证据目录。

真实生成结果：

1. Website `zenova-agent-cloud` 在首次尝试 accepted：Brief `run-1178`、Build `run-1232`、Version `version-1299`，总 Token `153926`（input `147199`、output `6727`、cached input `41600`），Artifact manifest hash 为 `c004e5f324b670d15de8bab690035f2e4a97c6ad8c63e0f67b5d88611a8fa874`；
2. Docs `agent-cloud-quickstart` 的首次诊断运行在 `preview.publish` 返回 `build.missing_dependency` 后，被旧的发布前 no-progress 计数提前终止；根因是 Runtime 只把 `build.failed` 识别为 repair transition；
3. Runtime 现在把全部 `build.*` preview failure 统一标记为 `repair_required`、激活有界 repair observation budget，并通过 `max_no_progress_turns=1` 回归测试证明修复发生在 no-progress stop 之前；
4. 修复部署后 Docs 单次重跑 accepted：Brief `run-1495`、Build `run-1557`、Version `version-1776`，总 Token `745723`（input `736757`、output `8966`、cached input `122368`），Artifact manifest hash 为 `8488279075f335ab52dc6fc75f78b0b3a9c01008b1329812d7171df0c80cc8ff`；
5. Docs 的 Next.js static export 同时包含 `docs.html` 和 `/docs/` RSC 数据目录，暴露出静态 Nginx 先命中目录并返回 403 的 pretty-route 缺陷；发布运行时现已优先解析 `.html`，同时显式支持带尾斜杠路由，修复不需要再次调用 AI；
6. 验证发布镜像 tag 现在同时包含 Project hash 与静态运行时配置 hash，避免同一 Artifact 在 Nginx 配置变化后复用陈旧镜像。

当前真实作品验证 URL：

- Website：`https://real-e948643ffd91995dd539.works.zerondesign.localhost:18443/`
- Docs：`https://real-066ab0c8d89e3606ae91.works.zerondesign.localhost:18443/docs/`

两个 URL 均通过本地 CA 的完整 HTTPS 校验、预期标题检查、`/.well-known/anydesign/release` 和 Release Header 身份核验；Certificate 由 Ingress 持有，TLS Secret 由 Certificate 持有。对应证据：

- Website suite：`services/runtime/target/e2e-evidence/zerondesign-greenfield/real-provider-runs/suite-20260718142059496-accepted/real-provider-examples-summary.json`；
- Docs suite：`services/runtime/target/e2e-evidence/zerondesign-greenfield/real-provider-runs/suite-20260718150033376-accepted/real-provider-examples-summary.json`；
- Website 发布：`services/runtime/target/e2e-evidence/zerondesign-greenfield/real-provider-runs/validation-publication.json`；
- Docs 发布：`services/runtime/target/e2e-evidence/zerondesign-greenfield/real-provider-runs/docs-validation-publication.json`。

这里的发布模式明确是 `validation`，不是产品 Release API：当前部署的 Runtime 镜像还没有 Release Packager helper，验证 Deployment 也还不是 digest-pinned。真实 Provider 稳定性审计仍为 `incomplete`，尚未达到同一门禁连续 `3/3` 成功。

当前剩余生产化主门禁为：

1. 将 Provider Gateway 切换到生产 PostgreSQL，配置独立加密密钥、至少两副本、备份、限流和告警；
2. 将 Release Packager helper 接入 Runtime 产品 Release API，使用 digest-pinned 工作负载替代验证发布脚本；
3. 完成同一真实 Website/Docs 门禁连续 `3/3` 稳定性审计，并覆盖剩余三个真实案例；
4. 测试 Key 已经出现在会话消息中，完成本轮验证后必须在 Provider 控制台撤销并轮换。
