import { ImportDesignProfileRequestSchema } from "@zerondesign/shared";
import { apiError } from "@/lib/http";
import { requireOwnedProject } from "@/lib/project-access";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const { projectId } = await context.params;
    const { userId } = await requireOwnedProject(projectId);
    const input = ImportDesignProfileRequestSchema.parse(await request.json());
    if (!("projectId" in input.scope) || input.scope.projectId !== projectId) {
      return Response.json({ error: "import scope must match the current project" }, { status: 400 });
    }
    return Response.json(await runtimeClient({
      userId,
      projectId,
      operations: ["project.write"],
    }).importDesignProfile(input), { status: 201 });
  } catch (error) {
    return apiError(error);
  }
}
