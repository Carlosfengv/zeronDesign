"use client";

import {
  AgentEventSchema,
  type BriefResponse,
  type ConversationItem,
  type DeploymentStateResponse,
  type PublicationOperationResponse,
  type ProfileTokenSyncOperationResponse,
  type ReleasePackagingResponse,
  type WorkRelease,
} from "@zerondesign/shared";
import { FormEvent, useCallback, useEffect, useRef, useState } from "react";

type Project = {
  id: string;
  name: string;
  kind: "website" | "docs";
  status: string;
  latestRunId?: string;
};

type Preview = { versionId: string; previewUrl: string };
type VersionBookmark = {
  versionId: string;
  status: "candidate" | "promoted" | "failed";
  current: boolean;
  reviewUrl: string;
};
type PublicationSnapshot = {
  deployment: DeploymentStateResponse | null;
  releases: WorkRelease[];
  activeJob: {
    projectId: string;
    versionId?: string;
    releaseId?: string;
    packagingId?: string;
    operationId?: string;
    action: "publish" | "rollback" | "unpublish";
    phase: "packaging" | "publication";
    status: string;
  } | null;
};
type FidelityAssertionSummary = {
  ruleId: string;
  recipeId: string | null;
  priority: string;
  kind: string;
  route: string;
  viewport: number | null;
  selector: string | null;
  property: string | null;
  actualSummary: string | null;
  expectedSummary: string | null;
  comparator: string | null;
  passed: boolean;
  reason: string | null;
};
type FidelitySummary = {
  status: "passed" | "failed";
  checkedAt: string | null;
  outputVersionId: string | null;
  requiredFailedRuleIds: string[];
  assertions: FidelityAssertionSummary[];
  repairContext: { targets: string[]; instructions: string[] };
};
type DesignContextSnapshot = {
  manifest: {
    package: {
      contentHash?: string | null;
      effectiveProfileHash: string;
      designProfileId: string;
      designProfileVersion: number;
      surface: "website" | "docs";
      template: string;
      effectiveCompatibilityMode?: string | null;
      verificationPolicyId: string;
      warnings: string[];
    };
    artifacts: Array<{ path: string; kind: string; sha256: string; requiredBeforeMutation: boolean }>;
  };
  diagnostics: {
    gate: "ready" | "read_required";
    readFiles: string[];
    missingRequiredReads: string[];
    materialization: { hash?: string | null; ready: boolean };
    styleContract: { verified: boolean | null };
    fidelity: FidelitySummary | null;
  };
  syncTarget: {
    designProfileId: string;
    designProfileVersion: number;
    effectiveProfileHash: string;
  } | null;
};

export function ProjectShell() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [selected, setSelected] = useState<Project | null>(null);
  const [prompt, setPrompt] = useState("");
  const [message, setMessage] = useState("准备创建你的第一个项目");
  const [conversation, setConversation] = useState<ConversationItem[]>([]);
  const [brief, setBrief] = useState<BriefResponse | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [versions, setVersions] = useState<VersionBookmark[]>([]);
  const [activeRunId, setActiveRunId] = useState<string | null>(null);
  const [publication, setPublication] = useState<PublicationSnapshot | null>(null);
  const [designContext, setDesignContext] = useState<DesignContextSnapshot | null>(null);
  const [profileSync, setProfileSync] = useState<ProfileTokenSyncOperationResponse | null>(null);
  const [conflictDecisions, setConflictDecisions] = useState<Record<string, "keep_current" | "apply_target">>({});
  const [publishing, setPublishing] = useState(false);
  const eventSource = useRef<EventSource | null>(null);
  const resumedPublicationJob = useRef<string | null>(null);

  const loadConversation = useCallback(async (project: Project) => {
    const response = await fetch(`/api/projects/${project.id}/conversation`, { cache: "no-store" });
    if (!response.ok) return;
    const payload = (await response.json()) as { items: ConversationItem[] };
    setConversation(payload.items);
    const briefId = [...payload.items].reverse().map(briefIdFromItem).find(Boolean);
    if (briefId) {
      const briefResponse = await fetch(`/api/briefs/${briefId}?projectId=${encodeURIComponent(project.id)}`, { cache: "no-store" });
      if (briefResponse.ok) setBrief(await briefResponse.json());
    }
  }, []);

  const loadPreview = useCallback(async (project: Project) => {
    const response = await fetch(`/api/projects/${project.id}/preview`, { cache: "no-store" });
    if (response.ok) setPreview(await response.json());
  }, []);

  const loadVersions = useCallback(async (project: Project) => {
    const response = await fetch(`/api/projects/${project.id}/versions`, { cache: "no-store" });
    if (!response.ok) return;
    const payload = (await response.json()) as { versions: VersionBookmark[] };
    setVersions(payload.versions);
  }, []);

  const loadPublication = useCallback(async (project: Project) => {
    const response = await fetch(`/api/projects/${project.id}/publication`, { cache: "no-store" });
    if (!response.ok) return;
    setPublication(await response.json());
  }, []);

  const loadDesignContext = useCallback(async (project: Project, runId: string) => {
    const response = await fetch(
      `/api/projects/${project.id}/runs/${encodeURIComponent(runId)}/design-context`,
      { cache: "no-store" },
    );
    if (response.status === 404) {
      setDesignContext(null);
      setProfileSync(null);
      return;
    }
    if (!response.ok) return;
    setDesignContext(await response.json());
    setProfileSync(null);
    setConflictDecisions({});
  }, []);

  const connectEvents = useCallback((project: Project, runId: string) => {
    eventSource.current?.close();
    setActiveRunId(runId);
    const source = new EventSource(`/api/projects/${project.id}/runs/${runId}/events`);
    eventSource.current = source;
    source.onmessage = (raw) => {
      const parsed = AgentEventSchema.safeParse(JSON.parse(raw.data));
      if (!parsed.success) return setMessage("收到无法识别的 Runtime 事件");
      const event = parsed.data;
      if (event.type === "agent.message") setMessage(event.text);
      if (event.type === "state.changed") setMessage(event.state);
      if (event.type === "preview.rebuilding") setMessage("Preview 正在重新构建…");
      if (event.type === "permission.requested") {
        setMessage(`等待授权：${event.tool}`);
      }
      if (event.type === "permission.denied") {
        setMessage(`权限已拒绝：${event.tool}`);
        setActiveRunId(null);
      }
      if (event.type === "preview.updated") {
        setMessage(`Preview 已更新：${event.versionId}`);
        void loadPreview(project);
        void loadVersions(project);
      }
      if (event.type === "run.completed") {
        setMessage(event.summary || `Run ${event.status}`);
        setActiveRunId(null);
        source.close();
      }
      void loadConversation(project);
    };
    source.onerror = () => setMessage("事件流已断开，正在自动重连…");
  }, [loadConversation, loadPreview, loadVersions]);

  useEffect(() => {
    void fetch("/api/projects")
      .then(async (response) => {
        if (!response.ok) throw new Error((await response.json()).error);
        return response.json() as Promise<{ projects: Project[] }>;
      })
      .then((payload) => {
        setProjects(payload.projects);
        setSelected(payload.projects[0] ?? null);
      })
      .catch((error: Error) => setMessage(error.message));
    return () => eventSource.current?.close();
  }, []);

  useEffect(() => {
    eventSource.current?.close();
    setConversation([]);
    setBrief(null);
    setPreview(null);
    setVersions([]);
    setPublication(null);
    setDesignContext(null);
    setProfileSync(null);
    setConflictDecisions({});
    if (!selected) return;
    void loadConversation(selected);
    void loadPreview(selected);
    void loadVersions(selected);
    void loadPublication(selected);
    if (selected.latestRunId) connectEvents(selected, selected.latestRunId);
    if (selected.latestRunId) void loadDesignContext(selected, selected.latestRunId);
  }, [connectEvents, loadConversation, loadDesignContext, loadPreview, loadPublication, loadVersions, selected]);

  useEffect(() => {
    if (!selected || !activeRunId) return;
    void loadDesignContext(selected, activeRunId);
  }, [activeRunId, loadDesignContext, selected]);

  useEffect(() => {
    const job = publication?.activeJob;
    if (!selected || !job || publishing) return;
    const identity = `${selected.id}:${job.phase}:${job.packagingId ?? job.operationId ?? job.status}`;
    if (resumedPublicationJob.current === identity) return;
    resumedPublicationJob.current = identity;
    setPublishing(true);
    void resumePublicationJob(selected, job)
      .catch((error) => setMessage(error instanceof Error ? error.message : "恢复发布任务失败"))
      .finally(() => setPublishing(false));
  }, [publication, publishing, selected]);

  async function create(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const response = await fetch("/api/projects", {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ name: data.get("name"), kind: data.get("kind") }),
    });
    const payload = await response.json();
    if (!response.ok) return setMessage(payload.error);
    const project = payload.project as Project;
    setProjects((current) => [project, ...current]);
    setSelected(project);
    setMessage("项目已创建，可以描述你想生成的内容");
    event.currentTarget.reset();
  }

  async function startBrief(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!selected) return;
    setMessage("正在分析需求并生成 Brief…");
    const response = await fetch(`/api/projects/${selected.id}/brief-runs`, {
      method: "POST", headers: { "content-type": "application/json" },
      body: JSON.stringify({ prompt }),
    });
    const payload = await response.json();
    if (!response.ok) return setMessage(payload.error);
    const project = { ...selected, latestRunId: payload.runId };
    setSelected(project);
    setProjects((current) => current.map((item) => item.id === project.id ? project : item));
    setMessage(`Brief Run 已启动：${payload.runId}`);
  }

  async function confirmBrief() {
    if (!brief) return;
    setMessage("正在确认 Brief…");
    const response = await fetch(`/api/briefs/${brief.briefId}/confirm?projectId=${encodeURIComponent(selected?.id ?? "")}`, { method: "POST" });
    const payload = await response.json();
    if (!response.ok) return setMessage(payload.error);
    setBrief(payload);
    setMessage("Brief 已确认，可以进入 Build");
    if (selected) void loadConversation(selected);
  }

  async function startBuild() {
    if (!brief || !selected) return;
    setMessage("正在启动 Build Run…");
    const response = await fetch(`/api/projects/${selected.id}/build-runs`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ briefId: brief.briefId }),
    });
    const payload = await response.json();
    if (!response.ok) return setMessage(payload.error);
    const project = { ...selected, latestRunId: payload.runId };
    setSelected(project);
    setProjects((current) => current.map((item) => item.id === project.id ? project : item));
    setMessage(`Build Run 已启动：${payload.runId}`);
  }

  async function startEdit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!selected || !preview) return;
    setMessage("正在恢复当前版本并启动 Edit Run…");
    const response = await fetch(`/api/projects/${selected.id}/edit-runs`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ message: prompt }),
    });
    const payload = await response.json();
    if (!response.ok) return setMessage(payload.error);
    const project = { ...selected, latestRunId: payload.runId };
    setSelected(project);
    setProjects((current) => current.map((item) => item.id === project.id ? project : item));
    setPrompt("");
    setMessage(`Edit Run 已启动：${payload.runId}`);
  }

  async function planProfileSync() {
    if (!selected || !designContext?.syncTarget || !designContext.manifest.package.contentHash) return;
    try {
      setMessage("正在读取实际 token 快照并生成 Profile Sync 三方 diff…");
      const operation = await fetchJson<ProfileTokenSyncOperationResponse>(
        `/api/projects/${selected.id}/runs/${encodeURIComponent(activeRunId ?? selected.latestRunId ?? "")}/profile-sync`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ idempotencyKey: crypto.randomUUID() }),
        },
      );
      setProfileSync(operation);
      setConflictDecisions({});
      const conflicts = operation.items.filter((item) => item.state === "conflict").length;
      setMessage(conflicts ? `发现 ${conflicts} 个 token 冲突，请逐项选择后确认` : "三方 diff 已就绪，可确认同步");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "无法创建 Profile Sync 计划");
    }
  }

  async function confirmProfileSync() {
    if (!selected || !profileSync) return;
    const conflicts = profileSync.items.filter((item) => item.state === "conflict");
    if (conflicts.some((item) => !conflictDecisions[item.token])) {
      setMessage("请为每个冲突 token 选择保留当前值或应用 Profile 值");
      return;
    }
    try {
      setMessage("正在确认 Profile Sync；Runtime 会在 child Run 的模型启动前写入 token…");
      const sourceRunId = activeRunId ?? selected.latestRunId;
      if (!sourceRunId) throw new Error("缺少可同步的源 Run");
      const operation = await fetchJson<ProfileTokenSyncOperationResponse>(
        `/api/projects/${selected.id}/runs/${encodeURIComponent(sourceRunId)}/profile-sync/${encodeURIComponent(profileSync.operationId)}/confirm`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            planHash: profileSync.planHash,
            conflictDecisions,
            idempotencyKey: crypto.randomUUID(),
          }),
        },
      );
      setProfileSync(operation);
      if (!operation.childRunId) {
        setMessage(`Profile Sync 状态：${operation.status}`);
        return;
      }
      const project = { ...selected, latestRunId: operation.childRunId };
      setSelected(project);
      setProjects((current) => current.map((item) => item.id === project.id ? project : item));
      connectEvents(project, operation.childRunId);
      setMessage("Profile Sync 已应用，已创建后续 Edit Run");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "确认 Profile Sync 失败");
    }
  }

  async function cancelActiveRun() {
    if (!selected || !activeRunId) return;
    if (!window.confirm("停止当前生成或编辑？已完成的记录会保留，当前成功版本不会被替换。")) return;
    const runId = activeRunId;
    setMessage("正在停止当前 Run…");
    try {
      const result = await fetchJson<{ runId: string; status: "cancelled" }>(
        `/api/projects/${selected.id}/runs/${encodeURIComponent(runId)}/cancel`,
        { method: "POST" },
      );
      if (result.runId !== runId || result.status !== "cancelled") {
        throw new Error("Runtime 返回了不一致的取消结果");
      }
      if (eventSource.current) {
        eventSource.current.onerror = null;
        eventSource.current.close();
        eventSource.current = null;
      }
      setActiveRunId(null);
      await Promise.all([
        loadConversation(selected),
        loadPreview(selected),
        loadVersions(selected),
      ]);
      setMessage("Run 已停止；上一成功 Preview 和已完成记录已保留");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "停止 Run 失败");
    }
  }

  async function publishCurrent() {
    if (!selected || !preview) return;
    setPublishing(true);
    try {
      setMessage("正在创建不可变 Release…");
      const created = await fetchJson<ReleasePackagingResponse>(
        `/api/projects/${selected.id}/releases`,
        { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ versionId: preview.versionId }) },
      );
      const packaged = await waitForPackaging(selected.id, created, setMessage);
      setMessage("Release 已验证，正在切换发布流量…");
      const started = await fetchJson<PublicationOperationResponse>(
        `/api/projects/${selected.id}/publication`,
        { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ action: "publish", releaseId: packaged.release.id }) },
      );
      await waitForPublication(selected.id, started, setMessage);
      await loadPublication(selected);
      setMessage("发布完成，持久化链接已更新");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "发布失败");
    } finally {
      setPublishing(false);
    }
  }

  async function mutatePublication(action: "rollback" | "unpublish", releaseId?: string) {
    if (!selected) return;
    setPublishing(true);
    try {
      const started = await fetchJson<PublicationOperationResponse>(
        `/api/projects/${selected.id}/publication`,
        { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ action, ...(releaseId ? { releaseId } : {}) }) },
      );
      await waitForPublication(selected.id, started, setMessage);
      await loadPublication(selected);
      setMessage(action === "rollback" ? "已回滚到历史 Release" : "作品已取消发布");
    } catch (error) {
      setMessage(error instanceof Error ? error.message : "发布操作失败");
    } finally {
      setPublishing(false);
    }
  }

  async function resumePublicationJob(
    project: Project,
    job: NonNullable<PublicationSnapshot["activeJob"]>,
  ) {
    if (job.phase === "packaging" && job.packagingId) {
      setMessage("正在恢复 Release 打包任务…");
      const current = await fetchJson<ReleasePackagingResponse>(
        `/api/projects/${project.id}/releases/${encodeURIComponent(job.packagingId)}`,
        { cache: "no-store" },
      );
      const packaged = await waitForPackaging(project.id, current, setMessage);
      const started = await fetchJson<PublicationOperationResponse>(
        `/api/projects/${project.id}/publication`,
        { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ action: "publish", releaseId: packaged.release.id }) },
      );
      await waitForPublication(project.id, started, setMessage);
    } else if (job.phase === "publication" && job.operationId) {
      setMessage("正在恢复发布操作…");
      const current = await fetchJson<PublicationOperationResponse>(
        `/api/projects/${project.id}/publication/operations/${encodeURIComponent(job.operationId)}`,
        { cache: "no-store" },
      );
      await waitForPublication(project.id, current, setMessage);
    } else if (job.phase === "publication") {
      setMessage("正在恢复尚未确认的发布请求…");
      const current = await fetchJson<PublicationOperationResponse>(
        `/api/projects/${project.id}/publication`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            action: job.action,
            ...(job.releaseId ? { releaseId: job.releaseId } : {}),
          }),
        },
      );
      await waitForPublication(project.id, current, setMessage);
    }
    await loadPublication(project);
    setMessage("发布任务已恢复并完成");
  }

  const currentReleaseId = publication?.deployment?.runtime.currentReleaseId ?? undefined;
  const rollbackRelease = publication?.releases.find(
    (release) => release.status === "validated" && release.id !== currentReleaseId,
  );
  const resolvedPermissionIds = new Set(
    conversation
      .filter((item) => item.kind === "permission_resolved" || item.kind === "permission_denied")
      .map(permissionIdFromItem)
      .filter((permissionId): permissionId is string => Boolean(permissionId)),
  );

  async function permissionResolved(status: string) {
    if (selected) await loadConversation(selected);
    if (status === "blocked") setActiveRunId(null);
    setMessage(status === "running" ? "权限已批准，Run 正在继续" : `权限决策已记录：${status}`);
  }

  return (
    <main className="shell">
      <aside className="sidebar">
        <div className="brand">zeronDesign <span>alpha</span></div>
        <form className="new-project" onSubmit={create}>
          <input name="name" aria-label="项目名称" placeholder="项目名称" required />
          <select name="kind" aria-label="项目类型" defaultValue="website"><option value="website">Website</option><option value="docs">Docs</option></select>
          <button type="submit">新建项目</button>
        </form>
        <nav aria-label="项目列表">{projects.map((project) => (
          <button
            className={selected?.id === project.id ? "project active" : "project"}
            data-project-id={project.id}
            key={project.id}
            onClick={() => setSelected(project)}
          >
            <strong>{project.name}</strong><small>{project.kind} · {project.status}</small>
          </button>
        ))}</nav>
      </aside>
      <section className="workspace">
        <header><div><small>WORKSPACE</small><h1>{selected?.name ?? "新项目"}</h1></div><div className="status" data-testid="runtime-status">{message}</div></header>
        <div className="panes">
          <section className="chat">
            <div className="messages">
              {conversation.length === 0 && <div className="empty-copy"><span>01</span><h2>描述你想创建的内容</h2><p>Runtime 会先生成可确认的结构化 Brief。</p></div>}
              {conversation.map((item) => item.kind === "permission_requested" && !resolvedPermissionIds.has(permissionIdFromItem(item) ?? "") && selected
                ? <PermissionCard item={item} key={item.id} projectId={selected.id} onResolved={permissionResolved} />
                : <article className={`message ${item.role ?? "system"}`} key={item.id}><small>{item.role ?? item.kind}</small><p>{item.text}</p></article>)}
              {brief && <article className="brief-card"><small>STRUCTURED BRIEF · {brief.status}</small><h3>{brief.brief.projectType} / {brief.brief.recommendedTemplate}</h3><p>{brief.brief.visualDirection}</p><p>受众：{brief.brief.audience}</p>{brief.status === "draft" && <button disabled={Boolean(activeRunId && activeRunId !== brief.runId)} onClick={confirmBrief}>确认 Brief</button>}{brief.status === "confirmed" && <button disabled={Boolean(activeRunId)} onClick={startBuild}>开始 Build</button>}</article>}
              {selected && designContext && <DesignContextCard
                context={designContext}
                operation={profileSync}
                conflictDecisions={conflictDecisions}
                disabled={Boolean(activeRunId)}
                onPlan={planProfileSync}
                onDecision={(token, decision) => setConflictDecisions((current) => ({ ...current, [token]: decision }))}
                onConfirm={confirmProfileSync}
              />}
            </div>
            <form onSubmit={preview ? startEdit : startBrief}><textarea value={prompt} onChange={(event) => setPrompt(event.target.value)} placeholder={preview ? "描述要修改的文字、布局或视觉样式…" : "例如：为一个面向开发者的 API 平台创建简洁、专业的文档站…"} required disabled={!selected || Boolean(activeRunId)} /><div className="run-actions"><button type="submit" disabled={!selected || Boolean(activeRunId)}>{preview ? "应用修改" : "生成 Brief"}</button>{activeRunId && <button className="stop-run" type="button" onClick={cancelActiveRun}>停止当前 Run</button>}</div></form>
          </section>
          <section className="preview">
            <div className="preview-bar"><span>Preview</span><div className="publication-actions">{publication?.deployment?.publicUrl && <a href={publication.deployment.publicUrl} target="_blank" rel="noreferrer">访问线上作品</a>}<button disabled={!preview || publishing || Boolean(activeRunId)} onClick={publishCurrent}>{currentReleaseId ? "发布更新" : "发布"}</button>{rollbackRelease && <button disabled={publishing} onClick={() => mutatePublication("rollback", rollbackRelease.id)}>回滚</button>}{currentReleaseId && <button disabled={publishing} onClick={() => mutatePublication("unpublish")}>取消发布</button>}</div><div className="version-links">{versions.slice(0, 5).map((version) => <a className={version.current ? "current" : ""} href={version.reviewUrl} key={version.versionId} target="_blank" rel="noreferrer">{version.versionId}</a>)}<span>{preview?.versionId ?? "等待首个 promoted version"}</span></div></div>
            {preview ? <iframe key={preview.versionId} title="项目 Preview" src={preview.previewUrl} /> : <div className="canvas"><div><b>实时预览区</b><p>Brief 确认并完成 Build 后，这里将加载受保护的 Runtime Preview。</p></div></div>}
          </section>
        </div>
      </section>
    </main>
  );
}

function DesignContextCard({
  context,
  operation,
  conflictDecisions,
  disabled,
  onPlan,
  onDecision,
  onConfirm,
}: {
  context: DesignContextSnapshot;
  operation: ProfileTokenSyncOperationResponse | null;
  conflictDecisions: Record<string, "keep_current" | "apply_target">;
  disabled: boolean;
  onPlan: () => Promise<void>;
  onDecision: (token: string, decision: "keep_current" | "apply_target") => void;
  onConfirm: () => Promise<void>;
}) {
  const source = context.manifest.package;
  const target = context.syncTarget;
  const targetChanged = Boolean(target && (
    target.designProfileId !== source.designProfileId
    || target.designProfileVersion !== source.designProfileVersion
    || target.effectiveProfileHash !== source.effectiveProfileHash
  ));
  const conflicts = operation?.items.filter((item) => item.state === "conflict") ?? [];

  return (
    <article className="design-context-card" data-testid="design-context-card">
      <small>DESIGN CONTEXT · {context.diagnostics.gate === "ready" ? "READY" : "READ REQUIRED"}</small>
      <h3>{source.designProfileId}@{source.designProfileVersion} · {source.template}</h3>
      <dl>
        <div><dt>DCP</dt><dd>{shortHash(source.contentHash)} · {context.diagnostics.materialization.ready ? "materialized" : "not materialized"}</dd></div>
        <div><dt>Style contract</dt><dd>{context.diagnostics.styleContract.verified === true ? "verified" : context.diagnostics.styleContract.verified === false ? "failed" : "pending"}</dd></div>
        <div><dt>Required reads</dt><dd>{context.diagnostics.readFiles.length} read / {context.diagnostics.missingRequiredReads.length} missing</dd></div>
      </dl>
      {context.diagnostics.missingRequiredReads.length > 0 && <p className="context-warning">缺少：{context.diagnostics.missingRequiredReads.join("、")}</p>}
      {source.warnings.map((warning) => <p className="context-warning" key={warning}>{warning}</p>)}
      <FidelityOutcome fidelity={context.diagnostics.fidelity} />
      {targetChanged && !operation && <button data-testid="profile-sync-plan-trigger" disabled={disabled} onClick={() => void onPlan()}>查看最新 Profile 的三方 diff</button>}
      {target && !targetChanged && <p className="context-note" data-testid="profile-sync-aligned">项目当前绑定的 Profile 已与此 Run 快照一致。</p>}
      {operation && <div className="profile-sync-plan" data-testid="profile-sync-plan">
        <small>PROFILE SYNC · {operation.status}</small>
        <p>{operation.items.filter((item) => item.state === "apply_target").length} 项将应用；{conflicts.length} 项需要决议；{operation.items.filter((item) => item.state === "not_managed").length} 项不受管理。</p>
        {conflicts.map((item) => <fieldset key={item.token}>
          <legend>{item.token}</legend>
          <label><input data-testid="profile-sync-keep-current" type="radio" name={item.token} checked={conflictDecisions[item.token] === "keep_current"} onChange={() => onDecision(item.token, "keep_current")} /> 保留当前值</label>
          <label><input data-testid="profile-sync-apply-target" type="radio" name={item.token} checked={conflictDecisions[item.token] === "apply_target"} onChange={() => onDecision(item.token, "apply_target")} /> 应用 Profile 值</label>
        </fieldset>)}
        {operation.status === "planned" && <button data-testid="profile-sync-confirm" disabled={disabled} onClick={() => void onConfirm()}>确认并创建后续 Edit Run</button>}
        {operation.childRunId && <p className="context-note">child Run：{operation.childRunId}</p>}
      </div>}
    </article>
  );
}

function FidelityOutcome({ fidelity }: { fidelity: FidelitySummary | null }) {
  if (!fidelity) {
    return <section className="fidelity-outcome empty" data-testid="fidelity-outcome" data-status="not-run">
      <small>FIDELITY · NOT RUN</small>
      <p>此 Run 尚无 preview.publish 验证结果。</p>
    </section>;
  }
  const failures = fidelity.assertions.filter((assertion) => !assertion.passed);
  return <section className={`fidelity-outcome ${fidelity.status}`} data-testid="fidelity-outcome" data-status={fidelity.status}>
    <div className="fidelity-heading">
      <small>FIDELITY · {fidelity.status.toUpperCase()}</small>
      <span>{fidelity.requiredFailedRuleIds.length} required failed</span>
    </div>
    {(fidelity.outputVersionId || fidelity.checkedAt) && <p className="fidelity-meta">
      {fidelity.outputVersionId ? `Version ${fidelity.outputVersionId}` : "No promoted version"}
      {fidelity.checkedAt ? ` · ${fidelity.checkedAt}` : ""}
    </p>}
    {fidelity.requiredFailedRuleIds.length > 0 && <div className="fidelity-rule-links" aria-label="Required fidelity failures">
      {fidelity.requiredFailedRuleIds.map((ruleId) => <a href={`#${fidelityRuleAnchor(ruleId)}`} key={ruleId}>{ruleId}</a>)}
    </div>}
    {failures.map((assertion) => <article className="fidelity-finding" id={fidelityRuleAnchor(assertion.ruleId)} key={assertion.ruleId}>
      <h4><a href={`#${fidelityRuleAnchor(assertion.ruleId)}`}>{assertion.ruleId}</a></h4>
      <p>{assertion.recipeId ? `Recipe ${assertion.recipeId} · ` : ""}{assertion.priority} · {assertion.kind}</p>
      <dl>
        {assertion.selector && <div><dt>Element</dt><dd><code>{assertion.selector}</code></dd></div>}
        {assertion.viewport !== null && <div><dt>Viewport</dt><dd>{assertion.viewport}px</dd></div>}
        {assertion.property && <div><dt>Property</dt><dd><code>{assertion.property}</code></dd></div>}
        {assertion.actualSummary && <div><dt>Current</dt><dd>{assertion.actualSummary}</dd></div>}
        {assertion.expectedSummary && <div><dt>Target</dt><dd>{assertion.expectedSummary}</dd></div>}
      </dl>
      {assertion.reason && <p className="fidelity-reason">{assertion.reason}</p>}
    </article>)}
    {fidelity.status === "passed" && <p className="context-note">本次 Runtime fidelity 验证没有 required failure。</p>}
    {(fidelity.repairContext.targets.length > 0 || fidelity.repairContext.instructions.length > 0) && <div className="fidelity-repair">
      <strong>Repair context</strong>
      {fidelity.repairContext.targets.length > 0 && <p>目标：{fidelity.repairContext.targets.join("、")}</p>}
      {fidelity.repairContext.instructions.map((instruction) => <p key={instruction}>{instruction}</p>)}
    </div>}
  </section>;
}

function fidelityRuleAnchor(ruleId: string): string {
  return `fidelity-rule-${ruleId.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
}

function shortHash(value?: string | null): string {
  return value ? `${value.slice(0, 10)}…` : "unavailable";
}

function PermissionCard({
  item,
  projectId,
  onResolved,
}: {
  item: ConversationItem;
  projectId: string;
  onResolved: (status: string) => Promise<void>;
}) {
  const [input, setInput] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState("");
  const permissionId = permissionIdFromItem(item);
  const metadata = metadataRecord(item);
  const tool = typeof metadata?.tool === "string" ? metadata.tool : "unknown tool";
  const reason = typeof metadata?.reason === "string" ? metadata.reason : item.text;

  async function decide(decision: "allow" | "ask" | "deny") {
    if (!permissionId || !item.runId) return;
    setSubmitting(true);
    setError("");
    try {
      let updatedInput: unknown;
      if (decision === "allow" && input.trim()) {
        try {
          updatedInput = JSON.parse(input);
        } catch {
          throw new Error("批准时，修改后的工具输入必须是合法 JSON");
        }
      }
      if (decision === "ask" && !input.trim()) {
        throw new Error("请选择“需要说明”前填写补充说明");
      }
      const resolved = await fetchJson<{ runId: string; status: string }>(
        `/api/projects/${projectId}/permissions/${encodeURIComponent(permissionId)}`,
        {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ decision, ...(updatedInput !== undefined ? { updatedInput } : {}) }),
        },
      );
      if (decision === "ask") {
        await fetchJson(
          `/api/projects/${projectId}/runs/${encodeURIComponent(item.runId)}/continue`,
          {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ userMessage: input.trim() }),
          },
        );
      }
      await onResolved(resolved.status);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "权限决策失败");
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <article className="permission-card">
      <small>PERMISSION REQUEST</small>
      <h3>{tool}</h3>
      <p>{reason}</p>
      <textarea
        aria-label="修改后的工具输入或补充说明"
        value={input}
        onChange={(event) => setInput(event.target.value)}
        placeholder={'批准：可填写修改后的工具输入 JSON\n需要说明：填写给 Agent 的补充说明'}
        disabled={submitting}
      />
      {error && <p className="permission-error">{error}</p>}
      <div className="permission-buttons">
        <button disabled={submitting} onClick={() => decide("allow")}>批准</button>
        <button disabled={submitting} onClick={() => decide("ask")}>需要说明</button>
        <button disabled={submitting} onClick={() => decide("deny")}>拒绝</button>
      </div>
    </article>
  );
}

async function fetchJson<T>(url: string, init?: RequestInit): Promise<T> {
  const response = await fetch(url, init);
  const payload = await response.json();
  if (!response.ok) throw new Error(payload.error ?? `请求失败 (${response.status})`);
  return payload as T;
}

async function waitForPackaging(
  projectId: string,
  initial: ReleasePackagingResponse,
  report: (message: string) => void,
): Promise<ReleasePackagingResponse> {
  let current = initial;
  for (let attempt = 0; attempt < 600; attempt += 1) {
    if (current.packaging.status === "validated" && current.release.status === "validated") return current;
    if (current.packaging.status === "failed" || current.release.status === "failed") {
      throw new Error(current.packaging.lastError || "Release 打包或安全校验失败");
    }
    report(`Release ${packagingStatusLabel(current.packaging.status)}…`);
    await delay(1000);
    current = await fetchJson<ReleasePackagingResponse>(
      `/api/projects/${projectId}/releases/${encodeURIComponent(current.packaging.id)}`,
      { cache: "no-store" },
    );
  }
  throw new Error("Release 打包超时，可稍后再次点击发布继续查询");
}

async function waitForPublication(
  projectId: string,
  initial: PublicationOperationResponse,
  report: (message: string) => void,
): Promise<PublicationOperationResponse> {
  let current = initial;
  for (let attempt = 0; attempt < 300; attempt += 1) {
    const status = current.operation.status;
    if (status === "completed") return current;
    if (status === "failed" || status === "cancelled") {
      throw new Error(current.operation.lastError || `发布操作 ${status}`);
    }
    report(`发布状态：${status}`);
    await delay(1000);
    current = await fetchJson<PublicationOperationResponse>(
      `/api/projects/${projectId}/publication/operations/${encodeURIComponent(current.operation.id)}`,
      { cache: "no-store" },
    );
  }
  throw new Error("发布操作超时，请稍后刷新发布状态");
}

function packagingStatusLabel(status: ReleasePackagingResponse["packaging"]["status"]): string {
  return ({
    prepared: "准备中", building: "构建镜像中", pushed: "镜像已推送", scanning: "安全扫描中",
    signing: "签名中", validated: "已验证", failed: "失败", reconcile_required: "恢复中",
  })[status];
}

function delay(milliseconds: number) {
  return new Promise((resolve) => window.setTimeout(resolve, milliseconds));
}

function briefIdFromItem(item: ConversationItem): string | undefined {
  if (!item.metadata || typeof item.metadata !== "object" || !("briefId" in item.metadata)) return;
  const briefId = item.metadata.briefId;
  return typeof briefId === "string" && briefId ? briefId : undefined;
}

function permissionIdFromItem(item: ConversationItem): string | undefined {
  const metadata = metadataRecord(item);
  return typeof metadata?.permissionId === "string" && metadata.permissionId
    ? metadata.permissionId
    : undefined;
}

function metadataRecord(item: ConversationItem): Record<string, unknown> | undefined {
  return item.metadata && typeof item.metadata === "object"
    ? item.metadata as Record<string, unknown>
    : undefined;
}
