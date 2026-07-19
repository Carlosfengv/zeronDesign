import { CreateDesignProfileRequestSchema } from "@zerondesign/shared";
import { requirePlatformAdminId } from "@/lib/auth";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

export async function GET() {
  try {
    await requirePlatformAdminId();
    return Response.json(await runtimeClient().listDesignProfiles({ includeArchived: true }));
  } catch (error) {
    return apiError(error);
  }
}

export async function POST(request: Request) {
  try {
    await requirePlatformAdminId();
    if (!request.headers.get("x-change-reason")?.trim()) {
      return Response.json({ error: "x-change-reason is required" }, { status: 400 });
    }
    const input = CreateDesignProfileRequestSchema.parse(await request.json());
    if (!("platform" in input.profile.scope) || input.projectId) {
      return Response.json({ error: "admin profiles must use platform scope" }, { status: 400 });
    }
    return Response.json(await runtimeClient().createDesignProfile(input), { status: 201 });
  } catch (error) {
    return apiError(error);
  }
}
