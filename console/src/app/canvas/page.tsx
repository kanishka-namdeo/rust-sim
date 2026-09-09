/**
 * GCS v2 Operations Canvas — /canvas route (M8, T-A2 + §13.1 #1).
 *
 * Spec: docs/GCS_V2_SPEC.md §4.1 route mechanism + §13.1 M8 skeleton #1.
 *
 * Server Component shell rendering <CanvasLoader/>. The default route `/`
 * stays serving the untouched v1 page — M8 ships at /canvas, M9 swaps
 * (the shell moves up to `/`, v1 tree → `/legacy` until M14 removes it).
 *
 * A dedicated route (not a `/?canvas=1` query flag) is deliberate (v1.2):
 * v1's `page.tsx` is a statically-prerendered `'use client'` page, and
 * branching it on a query param forces either `useSearchParams()` at page
 * top level (a known Next.js build failure without a Suspense boundary) or
 * a server-shell refactor of the 700-line v1 page at M8; a sibling route
 * touches zero v1 code.
 */

import { CanvasLoader } from '@/components/canvas/CanvasLoader'

export const metadata = {
  title: 'Operations Canvas — rustsitsim · mavfleet',
  description: 'GCS v2 single-screen operator surface (MapLibre + edge HUD)',
}

export default function CanvasPage() {
  return <CanvasLoader />
}
