import { requireUserId } from "@/lib/auth";
import { getProject, updatePublicationJob } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET(
  _request: Request,
  context: { params: Promise<{ projectId: string; packagingId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId, packagingId } = await context.params;
    const project = await getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const client = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["publication.read"],
    });
    const result = await client.getReleasePackaging(packagingId);
    if (result.release.projectId !== project.runtimeProjectId) {
      return Response.json({ error: "release packaging not found" }, { status: 404 });
    }
    await updatePublicationJob({
      projectId: project.id,
      status: result.packaging.status,
      lastError: result.packaging.lastError,
    });
    return Response.json(result);
  } catch (error) {
    return apiError(error);
  }
}
