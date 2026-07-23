import { z } from "zod";
import { requireUserId } from "@/lib/auth";
import { getProject } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const QuerySchema = z.object({
  phase: z.enum(["build", "edit", "repair"]),
});

export async function GET(
  request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId } = await context.params;
    const project = await getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const { phase } = QuerySchema.parse({
      phase: new URL(request.url).searchParams.get("phase"),
    });
    const catalog = await runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.read"],
    }).listModelServices(project.runtimeProjectId, phase, phase);
    const currentState = phase === "edit"
      ? await runtimeClient({
          userId: ownerId,
          projectId: project.runtimeProjectId,
          operations: ["project.read"],
        }).getProjectRuntimeState(project.runtimeProjectId)
      : null;
    return Response.json({
      ...catalog,
      defaultModelServiceId: currentState?.modelServiceId ?? null,
    });
  } catch (error) {
    return apiError(error);
  }
}
