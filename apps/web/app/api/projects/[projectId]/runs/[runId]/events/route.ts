import { requireUserId } from "@/lib/auth";
import { getProject, ownsProjectRun } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeBaseUrl, runtimePublicHeaders } from "@/lib/runtime";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET(
  request: Request,
  context: { params: Promise<{ projectId: string; runId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId, runId } = await context.params;
    const project = await getProject(projectId, ownerId);
    if (!project || !(await ownsProjectRun({ projectId, runId, ownerId }))) {
      return Response.json({ error: "run not found" }, { status: 404 });
    }
    const lastEventId = request.headers.get("last-event-id");
    const upstream = await fetch(
      `${runtimeBaseUrl()}/runs/${encodeURIComponent(runId)}/events`,
      {
        headers: {
          accept: "text/event-stream",
          ...runtimePublicHeaders({
            userId: ownerId,
            projectId: project.runtimeProjectId,
            operations: ["project.read"],
          }),
          ...(lastEventId ? { "last-event-id": lastEventId } : {}),
        },
        cache: "no-store",
        signal: request.signal,
      },
    );
    if (!upstream.ok) {
      return new Response(await upstream.text(), {
        status: upstream.status,
        headers: { "content-type": upstream.headers.get("content-type") ?? "application/json" },
      });
    }
    return new Response(upstream.body, {
      status: 200,
      headers: {
        "content-type": "text/event-stream",
        "cache-control": "no-cache, no-store, must-revalidate",
        connection: "keep-alive",
        "x-accel-buffering": "no",
      },
    });
  } catch (error) {
    return apiError(error);
  }
}
