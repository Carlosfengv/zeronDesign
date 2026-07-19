import { z } from "zod";
import { requirePlatformAdminId } from "@/lib/auth";
import { setWorkspaceStatus } from "@/lib/db";
import { apiError } from "@/lib/http";

export const runtime = "nodejs";

const UpdateWorkspaceSchema = z.object({
  status: z.enum(["active", "disabled"]),
});

export async function PATCH(
  request: Request,
  context: { params: Promise<{ namespace: string }> },
) {
  try {
    await requirePlatformAdminId();
    const { namespace } = await context.params;
    const input = UpdateWorkspaceSchema.parse(await request.json());
    const workspace = await setWorkspaceStatus(namespace, input.status);
    if (!workspace) return Response.json({ error: "workspace not found" }, { status: 404 });
    return Response.json({ workspace });
  } catch (error) {
    return apiError(error);
  }
}
