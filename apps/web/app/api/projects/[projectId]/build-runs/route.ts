import { z } from "zod";
import { requireUserId } from "@/lib/auth";
import { getProject, recordProjectRun } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

const StartBuildSchema = z.object({
  briefId: z.string().trim().min(1),
  modelServiceId: z.string().trim().min(1).max(128),
});

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId } = await context.params;
    const project = await getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const { briefId, modelServiceId } = StartBuildSchema.parse(await request.json());
    const readClient = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.read"],
    });
    const [brief, catalog] = await Promise.all([
      readClient.getBrief(briefId),
      readClient.listModelServices(project.runtimeProjectId, "build", "build"),
    ]);
    if (brief.projectId !== project.runtimeProjectId || brief.status !== "confirmed") {
      return Response.json({ error: "confirmed brief not found" }, { status: 409 });
    }
    if (!catalog.items.some((item) => item.id === modelServiceId)) {
      return Response.json({ error: "selected model service is unavailable" }, { status: 409 });
    }
    const result = await runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.write"],
    }).startRun({
      projectId: project.runtimeProjectId,
      phase: "build",
      agentProfile: "build",
      inputContext: { briefId, modelResourceId: modelServiceId },
    });
    await recordProjectRun({ runId: result.runId, projectId: project.id, phase: "build" });
    return Response.json(result, { status: 202 });
  } catch (error) {
    return apiError(error);
  }
}
