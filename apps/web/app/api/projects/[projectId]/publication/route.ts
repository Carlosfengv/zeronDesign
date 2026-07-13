import { randomUUID } from "node:crypto";
import { RuntimeApiError } from "@zerondesign/shared";
import { z } from "zod";
import { requireUserId } from "@/lib/auth";
import {
  getProject,
  getPublicationJob,
  recordPublicationIntent,
  recordPublicationOperation,
} from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

const PublicationActionSchema = z.discriminatedUnion("action", [
  z.object({ action: z.literal("publish"), releaseId: z.string().min(1) }),
  z.object({ action: z.literal("rollback"), releaseId: z.string().min(1) }),
  z.object({ action: z.literal("unpublish") }),
]);

export async function GET(
  _request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId } = await context.params;
    const project = getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const client = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["publication.read"],
    });
    const releases = await client.listWorkReleases(project.runtimeProjectId);
    let deployment = null;
    try {
      deployment = await client.getDeploymentState(project.runtimeProjectId);
    } catch (error) {
      if (!(error instanceof RuntimeApiError) || error.status !== 404) throw error;
    }
    const job = getPublicationJob(project.id);
    const activeJob = job && !["completed", "failed", "cancelled"].includes(job.status)
      ? job
      : null;
    return Response.json({
      deployment,
      releases: releases.releases,
      activeJob,
    });
  } catch (error) {
    return apiError(error);
  }
}

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId } = await context.params;
    const project = getProject(projectId, ownerId);
    if (!project) return Response.json({ error: "project not found" }, { status: 404 });
    const input = PublicationActionSchema.parse(await request.json());
    const readClient = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["publication.read"],
    });
    let currentReleaseId: string | undefined;
    let expectedGeneration = 0;
    try {
      const deployment = await readClient.getDeploymentState(project.runtimeProjectId);
      currentReleaseId = deployment.runtime.currentReleaseId ?? undefined;
      expectedGeneration = deployment.runtime.desiredGeneration;
    } catch (error) {
      if (!(error instanceof RuntimeApiError) || error.status !== 404) throw error;
    }
    const writeClient = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["publication.write"],
    });
    const previousJob = getPublicationJob(project.id);
    const requestedReleaseId = input.action === "unpublish" ? undefined : input.releaseId;
    const reusableIntent = previousJob?.phase === "publication"
      && previousJob.status === "requesting"
      && previousJob.action === input.action
      && previousJob.releaseId === requestedReleaseId
      && previousJob.idempotencyKey
      && previousJob.expectedGeneration !== undefined;
    const idempotencyKey = reusableIntent
      ? previousJob.idempotencyKey!
      : `publication:${project.id}:${input.action}:${randomUUID()}`;
    if (reusableIntent) {
      currentReleaseId = previousJob.expectedCurrentReleaseId;
      expectedGeneration = previousJob.expectedGeneration!;
    } else {
      recordPublicationIntent({
        projectId: project.id,
        action: input.action,
        releaseId: requestedReleaseId,
        idempotencyKey,
        expectedGeneration,
        expectedCurrentReleaseId: currentReleaseId,
      });
    }
    if (input.action === "unpublish") {
      if (!currentReleaseId) {
        return Response.json({ error: "project is not currently published" }, { status: 409 });
      }
      const result = await writeClient.unpublishWork(
        project.runtimeProjectId,
        {
          expectedCurrentReleaseId: currentReleaseId,
          expectedGeneration,
          runtimeProfileId: "static-web-v1",
        },
        idempotencyKey,
      );
      recordPublicationOperation({
        projectId: project.id,
        action: input.action,
        operationId: result.operation.id,
        status: result.operation.status,
        lastError: result.operation.lastError,
      });
      return Response.json(result, { status: 202 });
    }
    if (input.action === "rollback" && !currentReleaseId) {
      return Response.json({ error: "rollback requires an existing publication" }, { status: 409 });
    }
    const publicationRequest = {
      releaseId: input.releaseId,
      ...(currentReleaseId ? { expectedCurrentReleaseId: currentReleaseId } : {}),
      expectedGeneration,
      runtimeProfileId: "static-web-v1" as const,
    };
    const result = input.action === "rollback"
      ? await writeClient.rollbackWork(project.runtimeProjectId, publicationRequest, idempotencyKey)
      : await writeClient.publishWork(project.runtimeProjectId, publicationRequest, idempotencyKey);
    recordPublicationOperation({
      projectId: project.id,
      action: input.action,
      releaseId: input.releaseId,
      operationId: result.operation.id,
      status: result.operation.status,
      lastError: result.operation.lastError,
    });
    return Response.json(result, { status: 202 });
  } catch (error) {
    return apiError(error);
  }
}
