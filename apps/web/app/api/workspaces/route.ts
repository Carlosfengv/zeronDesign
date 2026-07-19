import { requireUserId } from "@/lib/auth";
import { listWorkspaces } from "@/lib/db";
import { apiError } from "@/lib/http";

export const runtime = "nodejs";

export async function GET() {
  try {
    const userId = await requireUserId();
    return Response.json({ workspaces: await listWorkspaces(userId) });
  } catch (error) {
    return apiError(error);
  }
}
