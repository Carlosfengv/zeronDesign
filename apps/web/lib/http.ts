import { AuthenticationRequiredError, AuthorizationForbiddenError } from "./auth";
import { WorkspaceUnavailableError } from "./db";
import { ProjectNotFoundError, ProjectRegistrationIncompleteError } from "./project-access";
import { RuntimeApiError } from "@zerondesign/shared";
import { ZodError } from "zod";

export function apiError(error: unknown): Response {
  if (error instanceof AuthenticationRequiredError) {
    return Response.json({ error: error.message }, { status: 401 });
  }
  if (error instanceof AuthorizationForbiddenError) {
    return Response.json({ error: error.message }, { status: 403 });
  }
  if (error instanceof ProjectNotFoundError) {
    return Response.json({ error: error.message }, { status: 404 });
  }
  if (error instanceof ProjectRegistrationIncompleteError) {
    return Response.json({ error: error.message }, { status: 409 });
  }
  if (error instanceof WorkspaceUnavailableError) {
    return Response.json({ error: error.message }, { status: 403 });
  }
  if (error instanceof RuntimeApiError) {
    const errorCode = runtimeErrorCode(error.payload);
    return Response.json(
      { error: error.message, ...(errorCode ? { errorCode } : {}) },
      { status: error.status },
    );
  }
  if (isZodValidationError(error)) {
    return Response.json({ error: "invalid request", issues: error.issues }, { status: 400 });
  }
  const message = error instanceof Error ? error.message : "unexpected error";
  return Response.json({ error: message }, { status: 500 });
}

function isZodValidationError(error: unknown): error is ZodError {
  return error instanceof ZodError || (
    typeof error === "object"
    && error !== null
    && "name" in error
    && "issues" in error
    && error.name === "ZodError"
    && Array.isArray(error.issues)
  );
}

function runtimeErrorCode(payload: unknown): string | undefined {
  if (
    typeof payload === "object"
    && payload !== null
    && "errorCode" in payload
    && typeof payload.errorCode === "string"
    && payload.errorCode.trim()
  ) {
    return payload.errorCode;
  }
  return undefined;
}
