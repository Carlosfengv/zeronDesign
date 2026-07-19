import { createHmac, timingSafeEqual } from "node:crypto";
import { cookies } from "next/headers";

export class AuthenticationRequiredError extends Error {}
export class AuthorizationForbiddenError extends Error {}

type SessionPayload = { sub: string; exp: number };

export async function requireUserId(): Promise<string> {
  const session = (await cookies()).get("zerondesign_session")?.value;
  const secret = process.env.ZERONDESIGN_SESSION_SECRET?.trim();
  if (session && secret && secret.length >= 32) {
    const payload = verifySession(session, secret);
    if (payload) return payload.sub;
  }

  const developmentUser = process.env.ZERONDESIGN_DEV_USER_ID?.trim();
  if (process.env.NODE_ENV !== "production" && developmentUser) return developmentUser;
  throw new AuthenticationRequiredError("authentication required");
}

export async function requirePlatformAdminId(): Promise<string> {
  const userId = await requireUserId();
  const administrators = new Set(
    (process.env.ZERONDESIGN_PLATFORM_ADMIN_IDS ?? "")
      .split(",")
      .map((value) => value.trim())
      .filter(Boolean),
  );
  if (!administrators.has(userId)) {
    throw new AuthorizationForbiddenError("platform administrator authorization required");
  }
  return userId;
}

export function issueSession(userId: string, expiresAt: Date, secret: string): string {
  if (!userId.trim() || secret.length < 32) throw new Error("valid user and 32-byte session secret required");
  const encoded = Buffer.from(JSON.stringify({
    sub: userId,
    exp: Math.floor(expiresAt.getTime() / 1000),
  } satisfies SessionPayload)).toString("base64url");
  return `${encoded}.${signature(encoded, secret)}`;
}

function verifySession(session: string, secret: string): SessionPayload | null {
  const [encoded, suppliedSignature, extra] = session.split(".");
  if (!encoded || !suppliedSignature || extra) return null;
  const expected = Buffer.from(signature(encoded, secret));
  const supplied = Buffer.from(suppliedSignature);
  if (expected.length !== supplied.length || !timingSafeEqual(expected, supplied)) return null;
  try {
    const payload = JSON.parse(Buffer.from(encoded, "base64url").toString("utf8")) as Partial<SessionPayload>;
    if (typeof payload.sub !== "string" || !payload.sub.trim()) return null;
    if (typeof payload.exp !== "number" || payload.exp <= Math.floor(Date.now() / 1000)) return null;
    return { sub: payload.sub, exp: payload.exp };
  } catch {
    return null;
  }
}

function signature(encoded: string, secret: string): string {
  return createHmac("sha256", secret).update(encoded).digest("base64url");
}
