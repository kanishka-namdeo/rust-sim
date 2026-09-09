'use client'

/**
 * GCS v2 Operations Canvas — CanvasLoader (M8, T-A2/B1 + §13.1 #1).
 *
 * Spec: docs/GCS_V2_SPEC.md §6.2 Server/Client split + §4.1 + §13.1 #1.
 *
 * `'use client'` boundary that does `dynamic(() => import('@/components/
 * canvas/OperationsCanvas'), { ssr: false })`. The `ssr:false` flag is
 * illegal inside Server Components — known Next.js rule — so this file
 * is the legal split point: `app/canvas/page.tsx` (Server Component)
 * renders `<CanvasLoader/>`; CanvasLoader dynamically imports the canvas
 * tree (which imports maplibre-gl — ESM-only, no SSR).
 *
 * The canvas tree is heavy: MapLibre + worker + ~1500 lines of widgets.
 * `ssr:false` keeps it out of the initial HTML so the page payload stays
 * lean and the M9 default-route swap (§4.1) doesn't have to refactor the
 * v1 static-prerendered `/` page.
 */

import dynamic from 'next/dynamic'

const OperationsCanvas = dynamic(
  () => import('@/components/canvas/OperationsCanvas').then((m) => m.OperationsCanvas),
  {
    ssr: false,
    loading: () => (
      <div
        className="rsim-canvas flex h-screen w-screen items-center justify-center"
        style={{ background: 'var(--rsim-bg)', color: 'var(--rsim-text-dim)', fontFamily: 'var(--rsim-font-mono)' }}
      >
        loading operations canvas…
      </div>
    ),
  },
)

export function CanvasLoader() {
  return <OperationsCanvas />
}
