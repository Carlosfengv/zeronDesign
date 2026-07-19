import { productDatabaseHealth } from "@/lib/db";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET() {
  try {
    return Response.json(await productDatabaseHealth());
  } catch (error) {
    return Response.json(
      { ok: false, error: error instanceof Error ? error.message : "product catalog unavailable" },
      { status: 503 },
    );
  }
}
