import { AuthenticationRequiredError } from "./auth";
import { RuntimeApiError } from "@zerondesign/shared";

export function apiError(error: unknown): Response {
  if (error instanceof AuthenticationRequiredError) {
    return Response.json({ error: error.message }, { status: 401 });
  }
  if (error instanceof RuntimeApiError) {
    return Response.json({ error: error.message }, { status: error.status });
  }
  const message = error instanceof Error ? error.message : "unexpected error";
  return Response.json({ error: message }, { status: 500 });
}
