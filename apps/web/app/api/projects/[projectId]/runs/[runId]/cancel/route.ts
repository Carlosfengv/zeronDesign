import { requireUserId } from "@/lib/auth";
import { getProject, ownsProjectRun } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

export async function POST(
  _request: Request,
  context: { params: Promise<{ projectId: string; runId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId, runId } = await context.params;
    const project = getProject(projectId, ownerId);
    if (!project || !ownsProjectRun({ runId, projectId: project.id, ownerId })) {
      return Response.json({ error: "run not found" }, { status: 404 });
    }
    const result = await runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.write"],
    }).cancelRun(runId);
    return Response.json(result);
  } catch (error) {
    return apiError(error);
  }
}
