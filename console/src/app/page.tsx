/**
 * GCS v2 Operations Canvas — default route (M9 route swap).
 *
 * Spec: docs/GCS_V2_SPEC.md §4.1 (route swap) + §13.1 #1.
 *
 * M9: the shell moves up to `/` — renders <CanvasLoader/> (the dynamic
 * import boundary for the Operations Canvas). The v1 tab tree moves to
 * `/legacy` (a migration safety net, not a user surface) until M14
 * removes it.
 *
 * Server Component shell (no 'use client' here — `ssr:false` is illegal
 * inside Server Components, so CanvasLoader is the legal split point).
 *
 * The three v1 browser scripts (`scripts/browser_*.sh`) re-point from `/`
 * to `/legacy` at this swap (one-line URL change, stream D).
 */

import { CanvasLoader } from '@/components/canvas/CanvasLoader'

export const metadata = {
  title: 'Operations Canvas — rustsitsim · mavfleet',
  description: 'GCS v2 single-screen operator surface (MapLibre + edge HUD)',
}

export default function Home() {
  return <CanvasLoader />
}
