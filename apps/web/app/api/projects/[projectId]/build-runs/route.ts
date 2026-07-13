import { z } from "zod";
import { requireUserId } from "@/lib/auth";
import { getProject, recordProjectRun } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

const StartBuildSchema = z.object({ briefId: z.string().trim().min(1) });

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId } = await context.params;
    const project = getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const { briefId } = StartBuildSchema.parse(await request.json());
    const brief = await runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.read"],
    }).getBrief(briefId);
    if (brief.projectId !== project.runtimeProjectId || brief.status !== "confirmed") {
      return Response.json({ error: "confirmed brief not found" }, { status: 409 });
    }
    const result = await runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.write"],
    }).startRun({
      projectId: project.runtimeProjectId,
      phase: "build",
      agentProfile: "build",
      inputContext: { briefId },
    });
    recordProjectRun({ runId: result.runId, projectId: project.id, phase: "build" });
    return Response.json(result, { status: 202 });
  } catch (error) {
    return apiError(error);
  }
}
