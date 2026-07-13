import {
  createHash,
  createPrivateKey,
  createPublicKey,
  randomUUID,
  sign,
  type KeyObject,
} from "node:crypto";
import { createRuntimeClient } from "@zerondesign/shared";

export type RuntimeOperation =
  | "project.read"
  | "project.write"
  | "preview.read"
  | "publication.read"
  | "publication.write";

export type RuntimePrincipalContext = {
  userId: string;
  projectId: string;
  operations: RuntimeOperation[];
};

export function runtimeClient(principal?: RuntimePrincipalContext) {
  return createRuntimeClient({
    baseUrl: runtimeBaseUrl(),
    internalAdminToken: process.env.RUNTIME_INTERNAL_ADMIN_TOKEN?.trim(),
    publicPrincipalToken: principal ? issueRuntimePrincipal(principal) : undefined,
  });
}

export function runtimeBaseUrl(): string {
  const baseUrl = process.env.RUNTIME_BASE_URL?.trim().replace(/\/+$/, "");
  if (!baseUrl) throw new Error("RUNTIME_BASE_URL is required");
  return baseUrl;
}

export function runtimePublicHeaders(principal: RuntimePrincipalContext): Record<string, string> {
  return { authorization: `Bearer ${issueRuntimePrincipal(principal)}` };
}

function issueRuntimePrincipal(context: RuntimePrincipalContext): string {
  const encodedKey = process.env.RUNTIME_PRINCIPAL_PRIVATE_KEY_BASE64?.trim();
  if (!encodedKey) {
    const developmentToken = process.env.RUNTIME_PUBLIC_PRINCIPAL_TOKEN?.trim();
    if (process.env.NODE_ENV !== "production" && developmentToken) return developmentToken;
    if (process.env.NODE_ENV !== "production") return "";
    throw new Error("RUNTIME_PRINCIPAL_PRIVATE_KEY_BASE64 is required in production");
  }
  const privateKey = privateKeyFromBase64(encodedKey);
  const publicDer = createPublicKey(privateKey).export({ format: "der", type: "spki" });
  const keySuffix = createHash("sha256").update(publicDer).digest("hex").slice(0, 16);
  const now = Math.floor(Date.now() / 1000);
  const header = encodeJson({ alg: "EdDSA", typ: "JWT", kid: `ed25519-${keySuffix}` });
  const payload = encodeJson({
    iss: process.env.RUNTIME_PRINCIPAL_ISSUER?.trim() || "anydesign-bff",
    aud: process.env.RUNTIME_PRINCIPAL_AUDIENCE?.trim() || "anydesign-runtime-public",
    sub: context.userId,
    jti: randomUUID(),
    iat: now,
    exp: now + 60,
    projectId: context.projectId,
    operations: [...new Set(context.operations)],
  });
  const signingInput = `${header}.${payload}`;
  const signature = sign(null, Buffer.from(signingInput), privateKey).toString("base64url");
  return `${signingInput}.${signature}`;
}

function privateKeyFromBase64(encoded: string): KeyObject {
  return createPrivateKey({ key: Buffer.from(encoded, "base64"), format: "der", type: "pkcs8" });
}

function encodeJson(value: unknown): string {
  return Buffer.from(JSON.stringify(value)).toString("base64url");
}
