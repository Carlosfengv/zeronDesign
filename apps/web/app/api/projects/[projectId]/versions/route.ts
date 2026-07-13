import { RuntimeApiError, type ConversationItem } from "@zerondesign/shared";
import { requireUserId } from "@/lib/auth";
import { getProject, listProjectVersionIds, recordProjectVersion } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET(
  _request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId } = await context.params;
    const project = getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const client = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.read", "preview.read"],
    });
    const conversation = await client.getConversation(project.runtimeProjectId);
    const versionIds = new Set(listProjectVersionIds(project.id));
    for (const item of conversation.items) {
      const versionId = versionIdFromConversation(item);
      if (versionId) versionIds.add(versionId);
    }
    let currentVersionId: string | undefined;
    try {
      const current = await client.getPreviewCurrent(project.runtimeProjectId);
      currentVersionId = current.versionId;
      versionIds.add(current.versionId);
    } catch (error) {
      if (!(error instanceof RuntimeApiError) || error.status !== 404) throw error;
    }
    const versions = (
      await Promise.all([...versionIds].map(async (versionId) => {
        try {
          const version = await client.getPreviewVersion(project.runtimeProjectId, versionId);
          recordProjectVersion({ projectId: project.id, versionId, status: version.status });
          return {
            ...version,
            current: versionId === currentVersionId,
            reviewUrl: `/projects/${encodeURIComponent(project.id)}/versions/${encodeURIComponent(versionId)}/review/`,
          };
        } catch (error) {
          if (error instanceof RuntimeApiError && error.status === 404) return null;
          throw error;
        }
      }))
    ).filter((version): version is NonNullable<typeof version> => Boolean(version));
    versions.sort((left, right) => Number(right.current) - Number(left.current));
    return Response.json({ projectId: project.id, versions });
  } catch (error) {
    return apiError(error);
  }
}

function versionIdFromConversation(item: ConversationItem): string | undefined {
  if (!item.metadata || typeof item.metadata !== "object" || !("versionId" in item.metadata)) return;
  const versionId = item.metadata.versionId;
  return typeof versionId === "string" && versionId ? versionId : undefined;
}
