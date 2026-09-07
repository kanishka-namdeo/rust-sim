import { NextResponse } from "next/server";

/** GET /api — console identity stub (the live data planes are the Rust
 * backends: REST + WS reached through the gateway via XTransformPort, or
 * directly in NEXT_PUBLIC_RSIM_API_STYLE=direct mode). */
export async function GET() {
  return NextResponse.json({
    service: "rustsim-console",
    sim: ":8200",
    fleet: ":8400",
  });
}
