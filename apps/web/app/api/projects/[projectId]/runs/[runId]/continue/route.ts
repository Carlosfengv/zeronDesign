import { z } from "zod";
import { requireUserId } from "@/lib/auth";
import { getProject, ownsProjectRun } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

const ContinueSchema = z.object({ userMessage: z.string().trim().min(1).max(100_000) });

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string; runId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId, runId } = await context.params;
    const project = await getProject(projectId, ownerId);
    if (!project || !(await ownsProjectRun({ runId, projectId: project.id, ownerId }))) {
      return Response.json({ error: "run not found" }, { status: 404 });
    }
    const input = ContinueSchema.parse(await request.json());
    const result = await runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.write"],
    }).continueRun(runId, input);
    return Response.json(result);
  } catch (error) {
    return apiError(error);
  }
}
