import { ConfirmDesignProfileSyncRequestSchema } from "@zerondesign/shared";
import { requireUserId } from "@/lib/auth";
import { getProject, ownsProjectRun, recordProjectRun } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string; runId: string; operationId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId, runId, operationId } = await context.params;
    const project = getProject(projectId, ownerId);
    if (!project || !ownsProjectRun({ projectId: project.id, runId, ownerId })) {
      return Response.json({ error: "run not found" }, { status: 404 });
    }
    const input = ConfirmDesignProfileSyncRequestSchema.parse(await request.json());
    const operation = await runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.write"],
    }).confirmDesignProfileSync(runId, operationId, input);
    if (operation.childRunId) {
      recordProjectRun({
        runId: operation.childRunId,
        projectId: project.id,
        phase: "edit",
      });
    }
    return Response.json(operation);
  } catch (error) {
    return apiError(error);
  }
}
