import { requireUserId } from "@/lib/auth";
import { getProject, updatePublicationJob } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET(
  _request: Request,
  context: { params: Promise<{ projectId: string; operationId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId, operationId } = await context.params;
    const project = await getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const client = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["publication.read"],
    });
    const result = await client.getPublicationOperation(operationId);
    if (result.operation.projectId !== project.runtimeProjectId) {
      return Response.json({ error: "publication operation not found" }, { status: 404 });
    }
    await updatePublicationJob({
      projectId: project.id,
      status: result.operation.status,
      lastError: result.operation.lastError,
    });
    return Response.json(result);
  } catch (error) {
    return apiError(error);
  }
}
