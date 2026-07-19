import { requireUserId } from "@/lib/auth";
import { getProject } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeBaseUrl, runtimePublicHeaders } from "@/lib/runtime";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET(
  _request: Request,
  context: { params: Promise<{ projectId: string; artifactPath?: string[] }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId, artifactPath = [] } = await context.params;
    const project = await getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });

    const suffix = artifactPath.map(encodeURIComponent).join("/");
    const runtimePrefix = `/artifacts/${project.runtimeProjectId}/current`;
    const upstream = await fetch(
      `${runtimeBaseUrl()}${runtimePrefix}${suffix ? `/${suffix}` : "/"}`,
      {
        headers: runtimePublicHeaders({
          userId: ownerId,
          projectId: project.runtimeProjectId,
          operations: ["preview.read"],
        }),
        cache: "no-store",
      },
    );
    if (!upstream.ok) {
      return new Response(await upstream.text(), {
        status: upstream.status,
        headers: { "content-type": upstream.headers.get("content-type") ?? "application/json" },
      });
    }

    const contentType = upstream.headers.get("content-type") ?? "application/octet-stream";
    const headers = new Headers({
      "content-type": contentType,
      "cache-control": contentType.startsWith("text/html")
        ? "private, no-store"
        : "private, max-age=300",
      "x-content-type-options": "nosniff",
    });
    if (!contentType.startsWith("text/html")) {
      return new Response(upstream.body, { status: 200, headers });
    }

    const productPrefix = `/projects/${encodeURIComponent(project.id)}/preview`;
    let html = await upstream.text();
    html = html
      .replaceAll(runtimePrefix, productPrefix)
      .replaceAll('href="/_next/', `href="${productPrefix}/_next/`)
      .replaceAll('src="/_next/', `src="${productPrefix}/_next/`)
      .replaceAll('href="/_astro/', `href="${productPrefix}/_astro/`)
      .replaceAll('src="/_astro/', `src="${productPrefix}/_astro/`)
      .replaceAll('href="/docs', `href="${productPrefix}/docs`)
      .replaceAll('href="/"', `href="${productPrefix}/"`);
    return new Response(html, { status: 200, headers });
  } catch (error) {
    return apiError(error);
  }
}
