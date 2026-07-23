# Design Profile 产品化实施计划

> 实施状态（2026-07-18，第二批）：Runtime 已收敛为 project/platform 两种 scope，并加入服务端授权、显式绑定/解绑与平台管理员 BFF；项目界面已提供可见 Profile 的浏览和绑定。Workspace 注册表、Project 注册 saga 和双 Workspace k3d 基线已完成，Runtime HTTP 全量回归通过。创建/导入/审阅/激活的完整连续 UI、平台管理员页面、平台 Profile 双 Workspace 可见性 k3d 门禁、分页与 PostgreSQL 唯一约束仍在后续批次，当前不视为全部验收完成。

> 日期：2026-07-18<br>
> 状态：实施讨论稿<br>
> 上位资源模型：`2026-07-18-resource-model-and-productization-plan.md`<br>
> 平台前置：`2026-07-18-namespace-workspace-greenfield-implementation-plan.md`<br>
> 范围：项目私有 Design Profile、平台 Design Profile、项目绑定、Build 继承、Fidelity 与 Token Sync

## 1. 目标

把 Runtime 已有的 Design Profile 能力交付为一个安全、连续、可理解的项目内产品流程：

```text
项目 Design Profile 页
  -> 创建或导入项目 Profile
  -> 查看校验结果
  -> 激活
  -> 绑定到项目
  -> Build 自动继承
  -> 查看 Fidelity
  -> Profile 更新后执行 Token Sync
```

同时为平台管理员提供平台 Profile 管理入口，使已激活的平台 Profile 对所有项目可见并可被显式绑定。

本计划在 Namespace Workspace 绿地计划完成 Contract 收敛、Workspace Provisioning、Runtime 动态 Namespace 路由和生产存储边界后实施；不在 Design Profile 功能变更中重复实现平台基础设施。

## 2. 已锁定决策

### 2.1 作用域

Design Profile 只支持两种产品作用域：

| 类型 | 创建者 | 可见范围 | 可修改者 | 可绑定范围 |
| --- | --- | --- | --- | --- |
| project | 具有项目写权限的用户 | 所属项目 | 所属项目中具有写权限的用户 | 仅所属项目 |
| platform | 平台管理员 | 所有合法项目 | 平台管理员 | 所有项目 |

MVP 不新增 workspace-scoped 或 organization-scoped Profile；Workspace 本身仍由 Kubernetes Namespace 表示。

### 2.2 绑定

- 一个项目同时只有一个当前 Design Profile 绑定；
- 项目可以绑定本项目已激活 Profile；
- 项目可以绑定任一已激活的平台 Profile；
- 项目不能绑定其他项目 Profile；
- 平台 Profile 不自动应用，必须显式绑定；
- 更换绑定不会修改历史 Run 和历史 Project Version。

### 2.3 命名

- 中文 UI 使用“作品”；
- 数据库、代码与 API 保持 `Project`；
- 本计划不迁移 `/api/projects`，不增加第二套 Project/Work ID。

### 2.4 AI Provider

本计划不向普通用户开放 Model Resource 选择，也不建设 Provider 写管理台。Provider 管理台另行规划，第一阶段只读。

### 2.5 Workspace 与 Runtime 拓扑

- Kubernetes Namespace 是唯一 Workspace 身份；
- 不引入 Organization 层级；
- Project 创建时绑定一个 Namespace，创建后不能通过普通更新直接迁移；
- 第一阶段运行一个中央 Runtime Deployment，`replicas: 1`；
- 该 Runtime Pod 可以并发服务多个 Namespace、Project 和 Run；
- Sandbox、PVC、Preview 和发布资源按 Project 所属 Namespace 创建；
- 平台 Profile 保存在中央 Runtime 的共享目录中，对所有已注册 Namespace 可见；
- 项目 Profile 必须同时通过 Namespace 成员关系和 Project 权限校验。

`replicas: 1` 不表示只能创建一个 Runtime 资源、一个作品或一个作品副本。它只表示控制面 Runtime 只有一个 Pod。Sandbox 和发布工作负载拥有各自独立的 Deployment/Pod 数量。

## 3. 非目标

以下内容不进入本次实施：

- workspace-scoped 或 organization-scoped Profile；
- 多 Profile 同时组合；
- Profile 商店或社区分享；
- 用户自定义 Template；
- 自动把平台 Profile 推送到所有项目；
- Provider 资源创建、Secret 轮换或 Policy 编辑；
- Project 到 Work 的 API 更名；
- 批量跨项目绑定；
- 删除历史 Profile revision；
- 为旧 workspace/organization Profile 或固定 Namespace 环境提供迁移兼容。

## 4. 当前基础与缺口

### 4.1 已有能力

Runtime 已有：

- Design Source 上传与内容读取；
- 从 Source 导入 Profile Draft；
- Profile 创建、读取、列表、更新、归档；
- Draft 激活；
- 版本历史和 Diff；
- Conversion Report；
- Fidelity Report；
- Project Profile 绑定；
- Build 时 Profile 解析与 Design Context 冻结；
- Profile Token Sync 三方 Diff 与冲突确认。

Shared Package 已有大部分 Schema 和 Runtime Client 方法。

Web 已有：

- Project 身份与用户所有权；
- Runtime principal header 构造；
- Design Context 诊断；
- Profile Token Sync UI；
- Build、Edit、Preview 和 Version 流程。

### 4.2 首批实施基线与剩余缺口

| 项目 | 第一批状态 |
| --- | --- |
| Runtime Design Profile CRUD、List、Get、Bind principal 授权 | 已完成核心授权 |
| 明确的 project/platform Scope Schema | 已完成 |
| Web Profile BFF | 已完成；完整创建/导入/审阅 UI 待补 |
| Runtime 文件存储单副本限制 | 仍存在；生产 PostgreSQL/对象存储待补 |
| List API 按 principal 与 project 计算可见集合 | 已完成 |
| 平台管理员与普通项目身份分离 | 已完成核心边界；管理员页面待补 |
| 按 Project 动态解析 Workspace Namespace | 已完成核心路由 |
| Sandbox 与 Work 使用 Workspace Namespace | 已完成核心路由 |
| 授权、绑定、继承和历史冻结测试 | 核心授权与绑定已覆盖；完整 k3d E2E 待补 |

## 5. 目标数据模型

### 5.1 Scope

目标 Scope 使用互斥结构：

```ts
type DesignProfileScope =
  | { projectId: string }
  | { platform: true };
```

约束：

- 必须且只能出现一种作用域；
- `projectId` 必须是非空稳定 ID；
- `platform` 必须严格等于 `true`；
- 禁止通过 `organizationId: "platform"` 等特殊值模拟平台作用域；
- 直接拒绝 workspace-scoped、organization-scoped Profile；所有环境均从新 Schema 部署，不实现 legacy 兼容分支。

### 5.2 创建者与审计

Profile 记录需要能够解释谁创建和修改了资源。建议持久化或在审计日志中记录：

- createdByPrincipalId；
- updatedByPrincipalId；
- createdByRole：project_user 或 platform_admin；
- changeReason，平台管理员写操作必填；
- scope；
- version；
- createdAt / updatedAt。

如果暂不把创建者写入 Profile Schema，必须保证 Runtime 审计记录可按 profileId 和 revision 查询。

### 5.3 Project Binding

绑定记录至少包含：

```ts
type ProjectDesignProfileBinding = {
  projectId: string;
  designProfileId: string;
  boundProfileVersion: number;
  bindingRevision: number;
  boundByPrincipalId: string;
  createdAt: string;
  updatedAt: string;
};
```

`boundProfileVersion` 用于解释绑定时选择的版本；Build Run 仍应在启动时解析当前 active revision，并冻结完整的 Profile identity 和 effective hash。

绑定更新需要乐观版本检查，避免两个浏览器窗口静默覆盖彼此的选择。

### 5.4 Workspace Identity

ProjectAccessRecord 中的 `workspaceId` 统一保存 Kubernetes Namespace 名称，`organizationId` 不再参与产品授权和资源解析。

```ts
type ProjectWorkspaceBinding = {
  projectId: string;
  workspaceNamespace: string;
};
```

约束：

- namespace 必须符合 Kubernetes DNS label 规则；
- namespace 对象必须带 `zerondesign.dev/workspace=true` 标签；Kubernetes Namespace API 本身就是 Workspace Registry；
- Runtime 从 ProjectAccessRecord 读取 namespace，不接受客户端覆盖；
- Project ID 仍应全局唯一，但所有 K8s 查询同时带 namespace；
- Namespace 重命名视为迁移操作，必须迁移 PVC、Sandbox、Preview、Release 与 RBAC；
- Provider Gateway 的 workspaceId 使用 namespace 名称；产品契约直接移除 organizationId。

## 6. 授权模型

### 6.1 权限矩阵

| 操作 | 项目 read | 项目 write | 平台管理员 |
| --- | --- | --- | --- |
| 列出本项目 Profile | 允许 | 允许 | 仅在具有项目访问上下文时允许 |
| 查看本项目 Profile | 允许 | 允许 | 仅在具有项目访问上下文时允许 |
| 查看平台 Profile | 允许 | 允许 | 允许 |
| 创建项目 Profile | 拒绝 | 允许 | 具有项目写权限时允许 |
| 修改/激活/归档项目 Profile | 拒绝 | 允许 | 不默认绕过项目权限 |
| 创建/修改/激活/归档平台 Profile | 拒绝 | 拒绝 | 允许 |
| 绑定项目 Profile | 拒绝 | 允许 | 具有项目写权限时允许 |
| 绑定平台 Profile | 拒绝 | 允许 | 具有项目写权限时允许 |
| 读取其他项目 Profile | 拒绝 | 拒绝 | 不通过普通项目 API 暴露 |

平台管理员如果需要支持诊断其他项目，使用独立的、强审计的支持接口，不让普通 Profile API 隐式拥有跨项目能力。

### 6.2 服务端规则

每个 Runtime 操作必须：

1. 从认证头解析 principal，不能信任 Body 中的 owner 或 role；
2. 根据 ProjectAccessRecord 判断 project.read 或 project.write；
3. 从 ProjectAccessRecord 读取 workspace namespace，不能信任请求中的 workspaceId；
4. 不读取或传播产品级 organizationId；
5. 根据已认证管理员令牌判断 platform_admin；
6. 从已保存 Profile 读取 scope，不能只依赖请求中的 scope；
7. List API 由服务端计算可见集合；
8. 对不存在和不可见资源统一返回 404；
9. 对可见但无写权限资源返回 403；
10. 写操作记录 principal、namespace、profileId、scope、旧 revision、新 revision 和结果。

### 6.3 可见集合

项目上下文中的 Profile 列表：

```text
visibleProfiles(projectId) =
  profiles where scope.projectId == projectId
  + active profiles where scope.platform == true
```

普通项目用户不应看到：

- 其他项目 Profile；
- archived 平台 Profile；
- 平台 Profile 的 Secret 或内部审计信息；
- 无权访问的 Profile 是否存在。

## 7. Profile 解析与冻结

Run 启动时按以下优先级解析：

1. `inputContext.designProfileId`，前提是该 Profile 对项目可见且 active；
2. Project 当前绑定的 Profile，前提是仍然可见且 active；
3. 没有绑定则不使用 Profile。

不设置隐式“第一个平台 Profile”或“最新平台 Profile”。

Run 必须冻结：

- designProfileId；
- designProfileVersion；
- schemaVersion；
- base profile hash；
- surface；
- template；
- surface/template override hashes；
- effectiveProfileHash；
- Design Source identity；
- Design Context Package artifacts。

Profile 后续更新、归档或平台下架不能重写历史 Run。

## 8. BFF API

### 8.1 项目用户 API

```text
GET    /api/projects/{projectId}/design-profiles
POST   /api/projects/{projectId}/design-profiles
POST   /api/projects/{projectId}/design-profile-sources
POST   /api/projects/{projectId}/design-profile-imports
GET    /api/projects/{projectId}/design-profiles/{profileId}
PUT    /api/projects/{projectId}/design-profiles/{profileId}
POST   /api/projects/{projectId}/design-profiles/{profileId}/activate
POST   /api/projects/{projectId}/design-profiles/{profileId}/archive
GET    /api/projects/{projectId}/design-profiles/{profileId}/versions
GET    /api/projects/{projectId}/design-profiles/{profileId}/diff
GET    /api/projects/{projectId}/design-profile-binding
PUT    /api/projects/{projectId}/design-profile-binding
DELETE /api/projects/{projectId}/design-profile-binding
```

BFF 必须先通过 Web 数据库验证当前用户拥有项目，再使用 project-scoped Runtime client。Runtime 仍需独立完成授权，BFF 校验不是安全边界的替代品。

### 8.2 平台管理员 API

```text
GET    /api/admin/design-profiles
POST   /api/admin/design-profiles
GET    /api/admin/design-profiles/{profileId}
PUT    /api/admin/design-profiles/{profileId}
POST   /api/admin/design-profiles/{profileId}/activate
POST   /api/admin/design-profiles/{profileId}/archive
```

管理员写操作要求：

- 已认证管理员 Session；
- 服务端持有的 Runtime Admin Token；
- operator identity；
- change reason；
- idempotency key；
- CSRF 防护或严格 SameSite 与 Origin 校验。

## 9. UI 流程

### 9.1 项目 Design Profile 页

页面分为三个区域：

1. 当前绑定：名称、来源、scope、版本、状态、Fidelity 摘要；
2. 本项目 Profiles：项目用户创建的 Draft、Active、Archived；
3. 平台 Profile 库：只显示 Active，明确标记“平台提供，只读”。

主要操作：

- 新建 Profile；
- 上传设计文档并导入；
- 查看与修复校验问题；
- 激活；
- 绑定、更换、解绑；
- 查看版本与 Diff；
- 查看 Fidelity；
- 启动 Token Sync。

### 9.2 必须覆盖的状态

| 状态 | UI 行为 |
| --- | --- |
| 初次加载 | 使用骨架屏，绑定区与列表区独立加载 |
| 空项目 | 说明可新建或从平台库选择，不显示空白表格 |
| 无平台 Profile | 隐藏平台库空表格，显示简短说明 |
| Draft 有阻塞问题 | 禁用激活，问题按字段路径分组 |
| Profile 已归档 | 只读，不允许新绑定 |
| 当前绑定被归档 | 显示警告，保留历史身份，要求选择新 Profile 或解绑 |
| 平台 Profile 更新 | 显示有新 revision，不自动同步 |
| 并发绑定冲突 | 刷新当前绑定并要求用户重新确认 |
| Runtime 不可用 | 保留现有项目页面，Profile 区显示可重试错误 |
| 上传超限或类型错误 | 在提交前提示限制，并展示 Runtime 返回原因 |
| Token Sync 冲突 | 逐项选择 keep current 或 apply target |

### 9.3 管理员页面

平台管理员页面与项目页面分离，包含：

- 平台 Profile 列表；
- Draft/Active/Archived 筛选；
- 创建与导入；
- 校验和激活；
- 版本历史；
- 使用该 Profile 的项目数量，只返回聚合数量，不暴露项目内容。

## 10. 持久化与部署门槛

### 10.1 全新 Kubernetes 基线

所有环境，包括本地 k3d，都从零部署，不复用 `anydesign-sandboxes`、`anydesign-works` 或旧数据库状态。

Namespace 分工：

| Namespace 类型 | 作用 | 是否 Workspace |
| --- | --- | --- |
| Runtime system | 中央 Runtime、控制面 PVC、内部 Service | 否 |
| Provider system | Provider Gateway 与其数据库 | 否 |
| Workspace Namespace | Project 的 Sandbox、PVC、Preview、发布工作负载 | 是 |

Workspace Namespace 以 Kubernetes Namespace 对象为唯一事实来源：

```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: ws-example
  labels:
    zerondesign.dev/workspace: "true"
```

Workspace Provisioner 负责一次性创建：

- Namespace 与标准 labels；
- ResourceQuota 与 LimitRange；
- 默认拒绝 NetworkPolicy；
- Runtime ServiceAccount 的 namespace-scoped RoleBinding；
- Sandbox 所需 ServiceAccount、ConfigMap、Secret 和证书；
- SandboxTemplate；首期不创建常驻 warm pool，Sandbox 按 Run 在所属 Workspace Namespace 创建并回收；
- 发布 Ingress、TLS 和域名策略所需的 Namespace 配置。

中央 Runtime 不创建任意 Namespace，也不拥有 cluster-admin。它只在带 Workspace label 且已完成 RoleBinding 的 Namespace 内工作。

### 10.2 中央 Runtime 单副本 MVP

如果第一期继续使用 Runtime 文件日志：

- 中央 Runtime Deployment 固定为 `replicas: 1`，一个 Pod 服务所有已注册 Workspace Namespace；
- Runtime 存储目录必须挂载持久卷；
- 发布说明明确不支持水平扩容；
- 增加 Profile、Draft、Source、Binding 的重启恢复测试；
- 写入失败必须返回错误，不能只更新内存；
- 定期备份 Profile JSONL 和 Design Source blob。

单副本影响：

- Runtime Pod 重启或升级时，创建/继续 Run、Profile 管理和事件流会短暂不可用；
- 无法通过滚动升级实现零停机；
- 控制面吞吐受一个进程限制，但 Sandbox 中的 Build 和发布工作负载仍是独立 Pod；
- 已完成的 Project Version、Artifact 和已发布作品不会因为 Runtime Pod 数量为 1 而只能保留一份；
- 文件日志只有一个写入者，避免多 Pod 同时追加造成状态竞争。

Workspace Namespace 要求：

- Runtime 使用 namespace-scoped Role/RoleBinding 管理每个已注册 Workspace；
- 不授予无边界 cluster-admin；
- 新增 Workspace 时显式创建 RBAC；
- 删除 Namespace 前必须完成 Project 与持久化资源保留检查。

### 10.3 多副本与生产门槛

以下条件未满足前不得把 Runtime 扩为多副本：

- Design Profile 与 Binding 使用共享事务存储；
- profileId + version 唯一；
- projectId 当前绑定唯一；
- projectId active project Profile 约束明确；
- 更新采用 expectedVersion；
- 列表支持稳定分页；
- Source metadata 与 blob 提交具备失败恢复；
- 备份、恢复和迁移经过演练。

k3d 可以使用单 Runtime Pod + PVC 文件存储验证完整流程。首个生产环境由于同样是全新部署，控制面元数据应直接使用支持事务的共享持久化存储；不要先上线 JSONL 再承担一次生产数据迁移。文件日志可以继续用于本地证据、构建日志和可再生工件，但不作为生产 Profile、Binding、ProjectAccess 和 Run identity 的唯一事实来源。

Runtime 扩成多个 Pod 与作品发布副本扩容是两个独立问题。前者要求共享控制面数据库和协调机制；后者由每个 Work Release 的 Deployment replica 策略决定。

## 11. 实施阶段

### Phase 0：Namespace Workspace、授权和 Schema

Namespace Workspace 的通用基础设施由前置计划交付。本阶段只验证依赖已满足，并实现 Design Profile 特有的 scope 与授权。

涉及：

- `packages/shared/src/schemas.ts`
- `packages/shared/src/api-types.ts`
- `services/runtime/src/types.rs`
- `services/runtime/src/authorization.rs`
- `services/runtime/src/design_profile_service/*`
- `services/runtime/src/http_api/routes/design_profiles.rs`
- `services/runtime/src/http_api/routes/design_sources.rs`
- `services/runtime/src/conversation.rs`

交付：

1. 验证 ProjectAccessRecord 可以解析可信 workspace namespace；
2. project/platform Scope Schema；
3. Profile 可见性函数；
4. 所有 Profile 路由授权；
5. 管理员与项目 principal 分离；
6. 审计字段；
7. 跨 Namespace、跨项目与平台修改拒绝测试。

### Phase 1：BFF

涉及：

- `packages/shared/src/runtime-client.ts`
- `apps/web/lib/runtime.ts`
- `apps/web/app/api/projects/[projectId]/design-profiles/**`
- `apps/web/app/api/admin/design-profiles/**`

交付：

1. 项目 Profile CRUD、Import、Activate、Archive；
2. Binding GET/PUT/DELETE；
3. 管理员平台 Profile CRUD；
4. 统一错误映射；
5. BFF contract smoke tests。

### Phase 2：项目 UI 最小闭环

涉及：

- 项目导航与 Design Profile 页面；
- Profile 列表、创建/导入、校验、激活；
- 当前绑定与平台库；
- Profile 详情与基本版本信息。

交付时不要求一次完成所有高级 Diff 和使用统计。

### Phase 3：生成继承、Fidelity 与 Sync

交付：

1. Build 自动继承项目绑定；
2. Run 冻结身份可在 Web 查看；
3. Fidelity 摘要；
4. 更新提示；
5. 复用现有 Token Sync；
6. 历史 Run 不变性测试。

### Phase 4：上线与观测

交付：

- 功能开关；
- 单副本/多副本部署门槛检查；
- 授权拒绝与失败率指标；
- Profile 创建、激活、绑定、Build 继承成功率；
- 回滚说明和数据备份验证。

### Phase 5：全新 k3d 验证

交付：

1. 删除测试脚本对固定 `anydesign-sandboxes` 和 `anydesign-works` 的假设；
2. 每次测试创建全新 k3d cluster 或唯一 Workspace Namespace；
3. 部署 Runtime system、Provider system 和 Workspace Provisioner；
4. 通过 Provisioner 创建至少两个 Workspace Namespace；
5. 验证同 Workspace 多 Project、跨 Workspace 隔离和平台 Profile 全局可见；
6. 验证 Sandbox、PVC、Preview、Publication 都落在正确 Namespace；
7. 验证 Runtime 重启恢复；
8. 测试结束删除整个 k3d cluster，不执行旧状态迁移。

## 12. 测试矩阵

### 12.1 授权

| 场景 | 期望 |
| --- | --- |
| 项目 A 用户列出项目 A | 返回 A 的 Profile + Active 平台 Profile |
| 项目 A 用户列出项目 B | 404 |
| 项目 A 用户按 ID 获取项目 B Profile | 404 |
| 项目 A 用户修改平台 Profile | 403 |
| 项目 A 用户绑定项目 B Profile | 404 |
| 项目 A 用户绑定 Active 平台 Profile | 成功 |
| 项目 A 用户绑定 Archived 平台 Profile | 409 |
| 非管理员创建 platform scope | 403 |
| 管理员创建 platform scope | 成功并写审计 |
| 请求伪造另一个 workspaceId | 忽略客户端值并按 ProjectAccessRecord 处理，跨 Namespace 请求拒绝 |
| Runtime 在未注册 Namespace 创建 Sandbox | 拒绝并记录审计 |

### 12.2 生命周期

- 创建项目 Draft；
- 导入 Source 生成 Draft；
- 阻塞问题存在时拒绝激活；
- expectedVersion 冲突时返回 409；
- 激活后可以绑定；
- 归档后不能新绑定；
- 当前绑定归档后不破坏历史 Run；
- 更换绑定递增 bindingRevision；
- 解绑后新 Build 不使用 Profile。

### 12.3 生成

- Build 冻结正确 Profile revision；
- 显式 Profile 优先于绑定；
- 不可见显式 Profile 被拒绝；
- 无绑定时不隐式选择平台 Profile；
- Profile 与 Template 不兼容时进入明确错误状态；
- Profile 更新不改变排队中或已完成 Run 的 DCP；
- Token Sync 产生新的 Edit Run 和 Version。

### 12.4 存储与恢复

- Runtime 重启后恢复 Profile、Draft、Source 与 Binding；
- Source blob 缺失时激活或 Build 失败并给出可恢复错误；
- JSONL 尾部损坏时恢复策略可预测；
- 并发更新触发 expectedVersion 冲突；
- 写磁盘失败不会返回成功；
- 备份恢复后 hash 和 revision 保持一致。

### 12.5 Namespace Workspace

- Project 创建时保存合法 Namespace；
- 无效或未注册 Namespace 被拒绝；
- Runtime 在正确 Namespace 创建 Sandbox、PVC、Preview 和发布资源；
- Namespace A 的 principal 不能读取 Namespace B 的 Project 或 Profile；
- 新增 Namespace 后最小 RBAC 生效；
- 移除 RoleBinding 后 Runtime 无法越权回退到其他 Namespace；
- Provider Gateway 收到的 workspaceId 等于 Namespace，请求和策略 Schema 不包含 organizationId。
- 两个全新 Workspace Namespace 可以同时运行 Project，资源不会落入固定共享 tenant namespace。
- k3d 测试从空集群完成 system + workspace bootstrap，不依赖预装资源。

### 12.6 Web

- 空状态；
- Loading 与重试；
- 创建、导入、激活、绑定完整流程；
- 平台 Profile 只读标识；
- 归档绑定警告；
- 并发绑定冲突；
- Token Sync 冲突决策；
- 键盘操作、焦点管理和错误信息关联。

## 13. 验收标准

功能完成必须同时满足：

1. 用户可在项目内创建或导入项目 Profile；
2. 用户只能看到本项目 Profile 和 Active 平台 Profile；
3. 平台管理员可创建平台 Profile，普通用户不可修改；
4. 项目可显式绑定项目或平台 Profile；
5. Build 自动继承绑定并冻结完整身份；
6. Profile 更新不改变历史 Run 或 Version；
7. Fidelity 与 Token Sync 从项目 UI 可达；
8. 跨项目授权矩阵全部通过；
9. 当前部署模式的重启恢复测试通过；
10. 所有写操作可审计；
11. 功能关闭后原有 Build/Edit 流程不受影响；
12. 没有新增 Project/Work 双 ID 或重复 API；
13. Project 所属 Namespace 只能由服务端 ProjectAccessRecord 解析；
14. Sandbox、PVC、Preview 和发布资源创建在正确 Workspace Namespace；
15. Runtime ServiceAccount 不依赖 cluster-admin。

## 14. 上线与回滚

### 14.1 上线

建议使用分层功能开关：

- project profile read；
- project profile write；
- platform profile visibility；
- project binding；
- Build inheritance；
- Token Sync。

先内部项目，再小范围项目，最后全量。平台 Profile 的创建权限始终只对管理员开放。

### 14.2 回滚

回滚只关闭新入口和新绑定，不删除数据：

- 已冻结的 Run 继续使用其 DCP；
- 已完成的 Version 不变；
- 已有 Binding 保留但可停止被新 Run 解析；
- Profile、Source 和 Audit 不删除；
- 恢复后继续使用原 revision 和 bindingRevision。

## 15. 需要在开工前确认

部署为纯绿地，不需要历史数据审计、Namespace 迁移或旧 Schema 兼容。开工前只需锁定三项新环境参数：

1. Runtime system 与 Provider system Namespace 的最终名称；
2. Workspace Namespace 命名规则、域名规则和 ResourceQuota 默认值；
3. 首个生产 PostgreSQL 的连接、数据库/Schema 隔离和备份策略。

Docs 与 Website 是否共用完整编辑体验、用户能否显式选择 Model Resource、Template 是否开放自定义，均不阻塞本计划的项目 Profile MVP。
