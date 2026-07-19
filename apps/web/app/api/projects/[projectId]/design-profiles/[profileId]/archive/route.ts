import { apiError } from "@/lib/http";
import { requireOwnedProject } from "@/lib/project-access";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

export async function POST(
  _request: Request,
  context: { params: Promise<{ projectId: string; profileId: string }> },
) {
  try {
    const { projectId, profileId } = await context.params;
    const { userId } = await requireOwnedProject(projectId);
    return Response.json(await runtimeClient({
      userId,
      projectId,
      operations: ["project.write"],
    }).archiveDesignProfile(profileId));
  } catch (error) {
    return apiError(error);
  }
}
