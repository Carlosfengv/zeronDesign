import { z } from "zod";
import { requireUserId } from "@/lib/auth";
import { getProject, ownsProjectRun } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";

const CreateSyncPlanSchema = z.object({ idempotencyKey: z.string().trim().min(1).max(200) });

export async function POST(
  request: Request,
  context: { params: Promise<{ projectId: string; runId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId, runId } = await context.params;
    const project = getProject(projectId, ownerId);
    if (!project || !ownsProjectRun({ projectId: project.id, runId, ownerId })) {
      return Response.json({ error: "run not found" }, { status: 404 });
    }
    const { idempotencyKey } = CreateSyncPlanSchema.parse(await request.json());
    const client = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      // The BFF derives source identity from the protected frozen manifest
      // before invoking the write-only Runtime plan endpoint.
      operations: ["project.read", "project.write"],
    });
    const manifest = await client.getRunDesignContextManifest(runId);
    if (manifest.package.surface !== "website" || !manifest.package.contentHash) {
      return Response.json({ error: "Website Run with a frozen DCP is required for Profile Sync" }, { status: 409 });
    }
    const binding = await client.getProjectDesignProfile(project.runtimeProjectId);
    const profile = binding.designProfile;
    if (!profile) return Response.json({ error: "active project DesignProfile not found" }, { status: 409 });
    const fidelity = await client.getDesignProfileFidelityReport(
      profile.id,
      profile.version,
      { surface: "website", template: manifest.package.template },
    );
    const operation = await client.planDesignProfileSync(runId, {
      targetDesignProfileId: profile.id,
      targetDesignProfileVersion: profile.version,
      targetEffectiveProfileHash: fidelity.effectiveProfileHash,
      expectedSourceContentHash: manifest.package.contentHash,
      idempotencyKey,
    });
    return Response.json(operation, { status: 201 });
  } catch (error) {
    return apiError(error);
  }
}
