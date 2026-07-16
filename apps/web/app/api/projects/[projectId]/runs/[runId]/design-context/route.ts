import { RuntimeApiError } from "@zerondesign/shared";
import { requireUserId } from "@/lib/auth";
import { getProject, ownsProjectRun } from "@/lib/db";
import { apiError } from "@/lib/http";
import { runtimeClient } from "@/lib/runtime";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET(
  _request: Request,
  context: { params: Promise<{ projectId: string; runId: string }> },
) {
  try {
    const ownerId = await requireUserId();
    const { projectId, runId } = await context.params;
    const project = getProject(projectId, ownerId);
    if (!project || !ownsProjectRun({ projectId: project.id, runId, ownerId })) {
      return Response.json({ error: "run not found" }, { status: 404 });
    }
    const client = runtimeClient({
      userId: ownerId,
      projectId: project.runtimeProjectId,
      operations: ["project.read"],
    });
    const [manifest, diagnostics] = await Promise.all([
      client.getRunDesignContextManifest(runId),
      client.getRunDesignContextDiagnostics(runId),
    ]);

    // Sync always targets the currently bound active revision.  The browser
    // never invents its effective hash; Runtime still validates all target
    // visibility and frozen-source preconditions when the plan is created.
    let syncTarget: {
      designProfileId: string;
      designProfileVersion: number;
      effectiveProfileHash: string;
    } | null = null;
    if (manifest.package.surface === "website") {
      try {
        const binding = await client.getProjectDesignProfile(project.runtimeProjectId);
        const profile = binding.designProfile;
        if (profile) {
          const fidelity = await client.getDesignProfileFidelityReport(
            profile.id,
            profile.version,
            { surface: "website", template: manifest.package.template },
          );
          syncTarget = {
            designProfileId: profile.id,
            designProfileVersion: profile.version,
            effectiveProfileHash: fidelity.effectiveProfileHash,
          };
        }
      } catch (error) {
        // A Run remains inspectable when there is no current project binding
        // or the binding cannot target this frozen template.  Do not turn a
        // read-only DCP drawer into a false sync error.
        if (!(error instanceof RuntimeApiError) || error.status !== 404) throw error;
      }
    }
    return Response.json({ manifest, diagnostics, syncTarget });
  } catch (error) {
    return apiError(error);
  }
}
