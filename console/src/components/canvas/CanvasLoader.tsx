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

import { useEffect, useState, Suspense, type ReactNode, type ComponentType } from 'react'

// Lazy-load with explicit error handling so the canvas mount surfaces
// any module-load / render-time failure (the M8 spike: silent dynamic-import
// failures under Turbopack's ESM-only maplibre-gl had to be debugged by
// patching this loader).
const OperationsCanvasLazy = (() => {
  let p: Promise<{ default: ComponentType } | { OperationsCanvas: ComponentType }> | null = null
  return () => {
    if (!p) {
      p = import('@/components/canvas/OperationsCanvas').then(
        (m) => m,
        (err) => {
          console.error('[CanvasLoader] dynamic import failed:', err)
          throw err
        },
      )
    }
    return p
  }
})()

function Loading(): ReactNode {
  return (
    <div
      className="rsim-canvas flex h-screen w-screen items-center justify-center"
      style={{ background: 'var(--rsim-bg)', color: 'var(--rsim-text-dim)', fontFamily: 'var(--rsim-font-mono)' }}
    >
      loading operations canvas…
    </div>
  )
}

function ErrorFallback({ error }: { error: Error }): ReactNode {
  return (
    <div
      className="rsim-canvas flex h-screen w-screen flex-col items-center justify-center gap-2"
      style={{ background: 'var(--rsim-bg)', color: 'var(--rsim-danger)', fontFamily: 'var(--rsim-font-mono)', padding: 24 }}
    >
      <div style={{ fontSize: 16, fontWeight: 600 }}>canvas failed to load</div>
      <pre style={{ fontSize: 11, color: 'var(--rsim-text-dim)', maxWidth: 600, overflow: 'auto' }}>
        {error.message}
        {'\n'}
        {error.stack}
      </pre>
    </div>
  )
}

export function CanvasLoader() {
  const [Comp, setComp] = useState<ComponentType | null>(null)
  const [err, setErr] = useState<Error | null>(null)
  useEffect(() => {
    OperationsCanvasLazy().then(
      (m) => {
        const C = (m as { OperationsCanvas?: ComponentType; default?: ComponentType }).OperationsCanvas
          ?? (m as { default?: ComponentType }).default
        if (!C) {
          setErr(new Error('OperationsCanvas export not found in dynamic import'))
          return
        }
        setComp(() => C)
      },
      (e: Error) => setErr(e),
    )
  }, [])
  if (err) return <ErrorFallback error={err} />
  if (!Comp) return <Loading />
  return (
    <Suspense fallback={<Loading />}>
      <Comp />
    </Suspense>
  )
}

