import { requireUserId } from "./auth";
import { getProject, type ProductProject } from "./db";

export class ProjectNotFoundError extends Error {}
export class ProjectRegistrationIncompleteError extends Error {}

export async function requireOwnedProject(projectId: string): Promise<{
  userId: string;
  project: ProductProject;
}> {
  const userId = await requireUserId();
  const project = await getProject(projectId, userId);
  if (!project) throw new ProjectNotFoundError("project not found");
  if (project.status === "registering" || project.status === "registration_failed") {
    throw new ProjectRegistrationIncompleteError("project Workspace registration is incomplete");
  }
  return { userId, project };
}
