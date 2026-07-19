import { UpdateDesignProfileRequestSchema } from "@zerondesign/shared";
import { apiError } from "@/lib/http";
import { requireOwnedProject } from "@/lib/project-access";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

export async function GET(
  _request: Request,
  context: { params: Promise<{ projectId: string; profileId: string }> },
) {
  try {
    const { projectId, profileId } = await context.params;
    const { userId } = await requireOwnedProject(projectId);
    return Response.json(await runtimeClient({
      userId,
      projectId,
      operations: ["project.read"],
    }).getDesignProfile(profileId));
  } catch (error) {
    return apiError(error);
  }
}

export async function PUT(
  request: Request,
  context: { params: Promise<{ projectId: string; profileId: string }> },
) {
  try {
    const { projectId, profileId } = await context.params;
    const { userId } = await requireOwnedProject(projectId);
    const input = UpdateDesignProfileRequestSchema.parse(await request.json());
    return Response.json(await runtimeClient({
      userId,
      projectId,
      operations: ["project.write"],
    }).updateDesignProfile(profileId, input));
  } catch (error) {
    return apiError(error);
  }
}
