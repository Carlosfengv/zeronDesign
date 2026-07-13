import { z } from "zod";
import { requireUserId } from "@/lib/auth";
import { createProject, listProjects } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

const CreateProjectSchema = z.object({
  name: z.string().trim().min(1).max(120),
  kind: z.enum(["website", "docs"]),
});

export async function GET() {
  try {
    const ownerId = await requireUserId();
    return Response.json({ projects: listProjects(ownerId) });
  } catch (error) {
    return apiError(error);
  }
}

export async function POST(request: Request) {
  try {
    const ownerId = await requireUserId();
    const input = CreateProjectSchema.parse(await request.json());
    const projectId = crypto.randomUUID();
    if (process.env.RUNTIME_INTERNAL_ADMIN_TOKEN) {
      await runtimeClient().upsertProjectAccess(projectId, {
        ownerPrincipalId: ownerId,
      });
    }
    const project = createProject({ id: projectId, ownerId, ...input });
    return Response.json({ project }, { status: 201 });
  } catch (error) {
    return apiError(error);
  }
}
