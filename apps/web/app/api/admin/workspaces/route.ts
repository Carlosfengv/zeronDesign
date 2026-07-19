import { z } from "zod";
import { requirePlatformAdminId } from "@/lib/auth";
import { listAllWorkspaces, registerWorkspace } from "@/lib/db";
import { apiError } from "@/lib/http";

export const runtime = "nodejs";

const RegisterWorkspaceSchema = z.object({
  namespace: z.string().trim().min(4).max(63)
    .regex(/^ws-[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/),
  name: z.string().trim().min(1).max(120),
  ownerPrincipalId: z.string().trim().min(1).max(200),
});

export async function GET() {
  try {
    await requirePlatformAdminId();
    return Response.json({ workspaces: await listAllWorkspaces() });
  } catch (error) {
    return apiError(error);
  }
}

export async function POST(request: Request) {
  try {
    await requirePlatformAdminId();
    const input = RegisterWorkspaceSchema.parse(await request.json());
    const workspace = await registerWorkspace(input);
    return Response.json({ workspace }, { status: 201 });
  } catch (error) {
    return apiError(error);
  }
}
