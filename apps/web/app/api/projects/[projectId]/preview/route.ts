import { requireUserId } from "@/lib/auth";
import { getProject } from "@/lib/db";
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
    const project = await getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const preview = await runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["preview.read"],
    }).getPreviewCurrent(project.runtimeProjectId);
    return Response.json({
      ...preview,
      previewUrl: `/projects/${encodeURIComponent(project.id)}/preview/`,
    });
  } catch (error) {
    return apiError(error);
  }
}
