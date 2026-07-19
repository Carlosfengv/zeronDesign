import { z } from "zod";
import { requireUserId } from "@/lib/auth";
import {
  beginProjectRegistration,
  failProjectRegistration,
  finishProjectRegistration,
  getProject,
  listProjects,
} from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

const CreateProjectSchema = z.object({
  name: z.string().trim().min(1).max(120),
  kind: z.enum(["website", "docs"]),
  workspaceNamespace: z
    .string()
    .trim()
    .min(1)
    .max(63)
    .regex(/^ws-[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/),
});

export async function GET() {
  try {
    const ownerId = await requireUserId();
    return Response.json({ projects: await listProjects(ownerId) });
  } catch (error) {
    return apiError(error);
  }
}

export async function POST(request: Request) {
  try {
    const ownerId = await requireUserId();
    const input = CreateProjectSchema.parse(await request.json());
    const projectId = crypto.randomUUID();
    await beginProjectRegistration({ id: projectId, ownerId, ...input });
    try {
      await runtimeClient().upsertProjectAccess(projectId, {
        ownerPrincipalId: ownerId,
        workspaceNamespace: input.workspaceNamespace,
      });
    } catch (error) {
      await failProjectRegistration(projectId, ownerId);
      const response = apiError(error);
      const payload = await response.json() as Record<string, unknown>;
      return Response.json(
        { ...payload, project: await getProject(projectId, ownerId), retryable: true },
        { status: response.status },
      );
    }
    const project = await finishProjectRegistration(projectId, ownerId);
    return Response.json({ project }, { status: 201 });
  } catch (error) {
    return apiError(error);
  }
}
