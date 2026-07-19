import { requireUserId } from "@/lib/auth";
import { getProject } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeBaseUrl, runtimeClient, runtimePublicHeaders } from "@/lib/runtime";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET(
  _request: Request,
  context: {
    params: Promise<{ projectId: string; versionId: string; artifactPath?: string[] }>;
  },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId, versionId, artifactPath = [] } = await context.params;
    const project = await getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "version not found" }, { status: 404 });
    const principal = {
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["preview.read" as const],
    };
    await runtimeClient(principal).getPreviewVersion(project.runtimeProjectId, versionId);

    const suffix = artifactPath.map(encodeURIComponent).join("/");
    const runtimePrefix = `/artifacts/${project.runtimeProjectId}/versions/${versionId}`;
    const upstream = await fetch(
      `${runtimeBaseUrl()}${runtimePrefix}${suffix ? `/${suffix}` : "/"}`,
      { headers: runtimePublicHeaders(principal), cache: "no-store" },
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
        : "private, max-age=31536000, immutable",
      "x-content-type-options": "nosniff",
    });
    if (!contentType.startsWith("text/html")) {
      return new Response(upstream.body, { status: 200, headers });
    }
    const reviewPrefix = `/projects/${encodeURIComponent(project.id)}/versions/${encodeURIComponent(versionId)}/review`;
    let html = await upstream.text();
    html = html
      .replaceAll(runtimePrefix, reviewPrefix)
      .replaceAll('href="/_next/', `href="${reviewPrefix}/_next/`)
      .replaceAll('src="/_next/', `src="${reviewPrefix}/_next/`)
      .replaceAll('href="/_astro/', `href="${reviewPrefix}/_astro/`)
      .replaceAll('src="/_astro/', `src="${reviewPrefix}/_astro/`)
      .replaceAll('href="/docs', `href="${reviewPrefix}/docs`)
      .replaceAll('href="/"', `href="${reviewPrefix}/"`);
    return new Response(html, { status: 200, headers });
  } catch (error) {
    return apiError(error);
  }
}
