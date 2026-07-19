import { z } from "zod";
import { requireUserId } from "@/lib/auth";
import { getProject } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

const PermissionDecisionSchema = z.object({
  decision: z.enum(["allow", "ask", "deny"]),
  updatedInput: z.unknown().optional(),
});

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string; permissionId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId, permissionId } = await context.params;
    const project = await getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const decision = PermissionDecisionSchema.parse(await request.json());
    const result = await runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.write"],
    }).resolvePermission(permissionId, decision);
    return Response.json(result);
  } catch (error) {
    return apiError(error);
  }
}
