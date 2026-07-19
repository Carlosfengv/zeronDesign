import { BindProjectDesignProfileRequestSchema } from "@zerondesign/shared";
import { apiError } from "@/lib/http";
import { requireOwnedProject } from "@/lib/project-access";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

export async function GET(
  _request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const { projectId } = await context.params;
    const { userId } = await requireOwnedProject(projectId);
    return Response.json(await runtimeClient({
      userId,
      projectId,
      operations: ["project.read"],
    }).getProjectDesignProfile(projectId));
  } catch (error) {
    return apiError(error);
  }
}

export async function PUT(
  request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const { projectId } = await context.params;
    const { userId } = await requireOwnedProject(projectId);
    const input = BindProjectDesignProfileRequestSchema.parse(await request.json());
    return Response.json(await runtimeClient({
      userId,
      projectId,
      operations: ["project.write"],
    }).bindProjectDesignProfile(projectId, input));
  } catch (error) {
    return apiError(error);
  }
}

export async function DELETE(
  _request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const { projectId } = await context.params;
    const { userId } = await requireOwnedProject(projectId);
    return Response.json(await runtimeClient({
      userId,
      projectId,
      operations: ["project.write"],
    }).unbindProjectDesignProfile(projectId));
  } catch (error) {
    return apiError(error);
  }
}
