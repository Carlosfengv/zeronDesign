import { z } from "zod";
import { requireUserId } from "@/lib/auth";
import { getProject, recordProjectRun } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

const StartBriefSchema = z.object({ prompt: z.string().trim().min(1).max(100_000) });

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId } = await context.params;
    const project = await getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const { prompt } = StartBriefSchema.parse(await request.json());
    const result = await runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.write"],
    }).startRun({
      projectId: project.runtimeProjectId,
      phase: "brief",
      agentProfile: "brief",
      inputContext: {
        contentSources: [
          { id: crypto.randomUUID(), kind: "prompt", text: prompt, readable: true },
        ],
      },
    });
    await recordProjectRun({ runId: result.runId, projectId: project.id, phase: "brief" });
    return Response.json(result, { status: 202 });
  } catch (error) {
    return apiError(error);
  }
}
