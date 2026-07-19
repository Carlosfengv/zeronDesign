import { requireUserId } from "@/lib/auth";
import {
  failProjectRegistration,
  finishProjectRegistration,
  getProject,
} from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

export async function POST(
  _request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId } = await context.params;
    const existing = await getProject(projectId, ownerId);
    if (!existing) return Response.json({ error: "project not found" }, { status: 404 });
    if (existing.status !== "registering" && existing.status !== "registration_failed") {
      return Response.json({ project: existing });
    }
    try {
      await runtimeClient().upsertProjectAccess(projectId, {
        ownerPrincipalId: ownerId,
        workspaceNamespace: existing.workspaceNamespace,
      });
    } catch (error) {
      await failProjectRegistration(projectId, ownerId);
      throw error;
    }
    return Response.json({ project: await finishProjectRegistration(projectId, ownerId) });
  } catch (error) {
    return apiError(error);
  }
}
