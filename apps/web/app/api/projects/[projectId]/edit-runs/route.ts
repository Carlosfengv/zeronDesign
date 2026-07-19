import { z } from "zod";
import { requireUserId } from "@/lib/auth";
import { getProject, recordProjectRun } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

const StartEditSchema = z.object({ message: z.string().trim().min(1).max(100_000) });

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId } = await context.params;
    const project = await getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const { message } = StartEditSchema.parse(await request.json());
    const readClient = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.read"],
    });
    const state = await readClient.getProjectRuntimeState(project.runtimeProjectId);
    const writeClient = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.write"],
    });
    const started = await writeClient.startRun({
      projectId: project.runtimeProjectId,
      phase: "edit",
      agentProfile: "edit",
      inputContext: {
        baseVersionId: state.currentVersionId,
        sandboxBindingId: state.sandboxBindingId,
      },
    });
    await recordProjectRun({ runId: started.runId, projectId: project.id, phase: "edit" });
    const resumed = await writeClient.continueRun(started.runId, { userMessage: message });
    return Response.json(resumed, { status: 202 });
  } catch (error) {
    return apiError(error);
  }
}
