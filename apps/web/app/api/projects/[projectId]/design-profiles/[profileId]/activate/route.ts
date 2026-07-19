import { ActivateDesignProfileRequestSchema } from "@zerondesign/shared";
import { apiError } from "@/lib/http";
import { requireOwnedProject } from "@/lib/project-access";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string; profileId: string }> },
) {
  try {
    const { projectId, profileId } = await context.params;
    const { userId } = await requireOwnedProject(projectId);
    const input = ActivateDesignProfileRequestSchema.parse(await request.json());
    return Response.json(await runtimeClient({
      userId,
      projectId,
      operations: ["project.write"],
    }).activateDesignProfile(profileId, input));
  } catch (error) {
    return apiError(error);
  }
}
