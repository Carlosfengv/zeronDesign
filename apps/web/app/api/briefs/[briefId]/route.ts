import { apiError } from "@/lib/http";
import { requireUserId } from "@/lib/auth";
import { getProject } from "@/lib/db";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

export async function GET(
  request: Request,
  context: { params: Promise<{ briefId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { briefId } = await context.params;
    const projectId = new URL(request.url).searchParams.get("projectId") ?? "";
    const project = await getProject(projectId, ownerId);
    if (!project) {
      return Response.json({ error: "brief not found" }, { status: 404 });
    }
    const brief = await runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.read"],
    }).getBrief(briefId);
    if (brief.projectId !== project.runtimeProjectId) return Response.json({ error: "brief not found" }, { status: 404 });
    return Response.json(brief);
  } catch (error) {
    return apiError(error);
  }
}
