import {
  CreateDesignProfileRequestSchema,
} from "@zerondesign/shared";
import { apiError } from "@/lib/http";
import { requireOwnedProject } from "@/lib/project-access";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

export async function GET(
  request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const { projectId } = await context.params;
    const { userId } = await requireOwnedProject(projectId);
    const includeArchived = new URL(request.url).searchParams.get("includeArchived") === "true";
    return Response.json(await runtimeClient({
      userId,
      projectId,
      operations: ["project.read"],
    }).listDesignProfiles({ projectId, includeArchived }));
  } catch (error) {
    return apiError(error);
  }
}

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const { projectId } = await context.params;
    const { userId } = await requireOwnedProject(projectId);
    const input = CreateDesignProfileRequestSchema.parse(await request.json());
    if (!("projectId" in input.profile.scope)
      || input.profile.scope.projectId !== projectId
      || input.projectId && input.projectId !== projectId) {
      return Response.json({ error: "profile scope must match the current project" }, { status: 400 });
    }
    return Response.json(await runtimeClient({
      userId,
      projectId,
      operations: ["project.write"],
    }).createDesignProfile({ ...input, projectId }), { status: 201 });
  } catch (error) {
    return apiError(error);
  }
}
