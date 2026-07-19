import { randomUUID } from "node:crypto";
import { z } from "zod";
import { requireUserId } from "@/lib/auth";
import { getProject, recordReleasePackaging } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const CreateReleaseSchema = z.object({ versionId: z.string().trim().min(1).optional() });

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId } = await context.params;
    const project = await getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const input = CreateReleaseSchema.parse(await request.json());
    const readClient = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["preview.read"],
    });
    const versionId = input.versionId
      ?? (await readClient.getPreviewCurrent(project.runtimeProjectId)).versionId;
    const writeClient = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["publication.write"],
    });
    const result = await writeClient.createRelease(
      project.runtimeProjectId,
      versionId,
      { runtimeProfileId: "static-web-v1" },
      `release:${project.id}:${versionId}:${randomUUID()}`,
    );
    await recordReleasePackaging({
      projectId: project.id,
      versionId,
      releaseId: result.release.id,
      packagingId: result.packaging.id,
      status: result.packaging.status,
      lastError: result.packaging.lastError,
    });
    return Response.json(result, { status: 202 });
  } catch (error) {
    return apiError(error);
  }
}
