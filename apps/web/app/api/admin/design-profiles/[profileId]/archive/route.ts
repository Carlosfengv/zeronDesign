import { requirePlatformAdminId } from "@/lib/auth";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

export async function POST(
  request: Request,
  context: { params: Promise<{ profileId: string }> },
) {
  try {
    await requirePlatformAdminId();
    if (!request.headers.get("x-change-reason")?.trim()) {
      return Response.json({ error: "x-change-reason is required" }, { status: 400 });
    }
    const { profileId } = await context.params;
    return Response.json(await runtimeClient().archiveDesignProfile(profileId));
  } catch (error) {
    return apiError(error);
  }
}
