'use client'

/**
 * North-up 2D NED canvas map (SPEC rustsitsim §12.1 / mavfleet §13: canvas 2D,
 * no charting framework). North = +n points up, East = +e points right.
 * Used for the sim trajectory plot and the fleet geofence map.
 */

import { useEffect, useRef } from 'react'
import type { PosSample } from '@/lib/types'

export interface MapMarker {
  n: number
  e: number
  label: string
  color: string
  shape: 'vehicle' | 'task' | 'home' | 'point'
  heading_deg?: number
  sub?: string
  filled?: boolean
  lineTo?: { n: number; e: number } | null
}

export interface NedMapProps {
  track?: PosSample[]
  trails?: { color: string; points: { n: number; e: number }[] }[]
  markers?: MapMarker[]
  polygon?: [number, number][] // [n, e] vertices
  polygonLabel?: string
  minHalfExtent?: number
  className?: string
  ariaLabel: string
}

const EMERALD = '#10b981'
const AMBER = '#f59e0b'

export function NedMap({
  track,
  trails,
  markers,
  polygon,
  polygonLabel,
  minHalfExtent = 25,
  className,
  ariaLabel,
}: NedMapProps) {
  const wrapRef = useRef<HTMLDivElement>(null)
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const propsRef = useRef({ track, trails, markers, polygon, polygonLabel, minHalfExtent })

  useEffect(() => {
    propsRef.current = { track, trails, markers, polygon, polygonLabel, minHalfExtent }
  }, [track, trails, markers, polygon, polygonLabel, minHalfExtent])

  useEffect(() => {
    const canvas = canvasRef.current
    const wrap = wrapRef.current
    if (!canvas || !wrap) return

    let raf = 0
    let w = 0
    let h = 0

    const ro = new ResizeObserver((entries) => {
      const rect = entries[0]?.contentRect
      if (!rect) return
      const dpr = Math.min(2, window.devicePixelRatio || 1)
      w = rect.width
      h = rect.height
      canvas.width = Math.max(1, Math.round(w * dpr))
      canvas.height = Math.max(1, Math.round(h * dpr))
      const ctx = canvas.getContext('2d')
      if (ctx) ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    })
    ro.observe(wrap)

    const cssVar = (name: string, fallback: string): string => {
      const v = getComputedStyle(document.body).getPropertyValue(name).trim()
      return v || fallback
    }

    const draw = () => {
      raf = requestAnimationFrame(draw)
      const ctx = canvas.getContext('2d')
      if (!ctx || w < 10 || h < 10) return
      const { track: tr, trails: tls, markers: mks, polygon: poly, polygonLabel: polyLabel, minHalfExtent: minHalf } =
        propsRef.current

      const fg = cssVar('--foreground', '#18181b')
      const muted = cssVar('--muted-foreground', '#71717a')
      const border = cssVar('--border', '#e4e4e7')

      ctx.clearRect(0, 0, w, h)

      // ---- world bounds ----
      let minN = Infinity
      let maxN = -Infinity
      let minE = Infinity
      let maxE = -Infinity
      const include = (n: number, e: number) => {
        if (!Number.isFinite(n) || !Number.isFinite(e)) return
        minN = Math.min(minN, n)
        maxN = Math.max(maxN, n)
        minE = Math.min(minE, e)
        maxE = Math.max(maxE, e)
      }
      if (poly) for (const [n, e] of poly) include(n, e)
      for (const p of tr ?? []) include(p.n, p.e)
      for (const t of tls ?? []) for (const p of t.points) include(p.n, p.e)
      for (const m of mks ?? []) {
        include(m.n, m.e)
        if (m.lineTo) include(m.lineTo.n, m.lineTo.e)
      }
      if (!Number.isFinite(minN)) {
        minN = 0
        maxN = 0
        minE = 0
        maxE = 0
      }
      let halfN = Math.max((maxN - minN) / 2 + 6, minHalf)
      let halfE = Math.max((maxE - minE) / 2 + 6, minHalf)
      const half = Math.max(halfN, halfE) // square-ish map
      halfN = half
      halfE = half
      const cn = (minN + maxN) / 2
      const ce = (minE + maxE) / 2

      const cx = w / 2
      const cy = h / 2
      const scale = Math.min(w, h) / (2 * half) // px per meter
      const X = (e: number) => cx + (e - ce) * scale
      const Y = (n: number) => cy - (n - cn) * scale

      // ---- grid ----
      const step = niceStep(half / 5)
      ctx.lineWidth = 1
      ctx.strokeStyle = border
      ctx.globalAlpha = 0.55
      const nStart = Math.ceil((cn - half) / step) * step
      for (let n = nStart; n <= cn + half; n += step) {
        ctx.beginPath()
        ctx.moveTo(X(ce - half), Y(n))
        ctx.lineTo(X(ce + half), Y(n))
        ctx.stroke()
      }
      const eStart = Math.ceil((ce - half) / step) * step
      for (let e = eStart; e <= ce + half; e += step) {
        ctx.beginPath()
        ctx.moveTo(X(e), Y(cn - half))
        ctx.lineTo(X(e), Y(cn + half))
        ctx.stroke()
      }
      ctx.globalAlpha = 1

      // axis labels
      ctx.font = '10px ui-sans-serif, system-ui, sans-serif'
      ctx.fillStyle = muted
      ctx.textAlign = 'left'
      ctx.textBaseline = 'top'
      ctx.fillText('E →', 8, h - 14)
      ctx.textBaseline = 'bottom'
      ctx.fillText('N ↑', 8, 12)
      // grid step label
      ctx.textAlign = 'right'
      ctx.textBaseline = 'top'
      ctx.fillText(`${step} m grid`, w - 8, h - 14)

      // ---- geofence polygon ----
      if (poly && poly.length >= 3) {
        ctx.beginPath()
        poly.forEach(([n, e], i) => {
          const x = X(e)
          const y = Y(n)
          if (i === 0) ctx.moveTo(x, y)
          else ctx.lineTo(x, y)
        })
        ctx.closePath()
        ctx.fillStyle = AMBER
        ctx.globalAlpha = 0.06
        ctx.fill()
        ctx.globalAlpha = 1
        ctx.strokeStyle = AMBER
        ctx.lineWidth = 1.5
        ctx.setLineDash([6, 4])
        ctx.stroke()
        ctx.setLineDash([])
        if (polyLabel) {
          const [fn, fe] = poly[0]
          ctx.fillStyle = AMBER
          ctx.textAlign = 'left'
          ctx.textBaseline = 'bottom'
          ctx.fillText(polyLabel, X(fe) + 4, Y(fn) - 4)
        }
      }

      // ---- sim trajectory (decimated to ≤300 points) ----
      if (tr && tr.length > 1) {
        const stride = Math.max(1, Math.ceil(tr.length / 300))
        ctx.strokeStyle = EMERALD
        ctx.lineWidth = 2
        ctx.lineJoin = 'round'
        ctx.lineCap = 'round'
        ctx.beginPath()
        let started = false
        for (let i = 0; i < tr.length; i += stride) {
          const p = tr[i]
          const x = X(p.e)
          const y = Y(p.n)
          if (!started) {
            ctx.moveTo(x, y)
            started = true
          } else ctx.lineTo(x, y)
        }
        // always include the newest point
        const last = tr[tr.length - 1]
        ctx.lineTo(X(last.e), Y(last.n))
        ctx.stroke()
        // origin marker
        ctx.fillStyle = muted
        ctx.beginPath()
        ctx.arc(X(tr[0].e), Y(tr[0].n), 2.5, 0, 2 * Math.PI)
        ctx.fill()
      }

      // ---- breadcrumbs ----
      for (const t of tls ?? []) {
        if (t.points.length < 2) continue
        ctx.strokeStyle = t.color
        ctx.globalAlpha = 0.45
        ctx.lineWidth = 1.5
        ctx.beginPath()
        t.points.forEach((p, i) => {
          const x = X(p.e)
          const y = Y(p.n)
          if (i === 0) ctx.moveTo(x, y)
          else ctx.lineTo(x, y)
        })
        ctx.stroke()
        ctx.globalAlpha = 1
      }

      // ---- markers ----
      for (const m of mks ?? []) {
        const x = X(m.e)
        const y = Y(m.n)

        if (m.lineTo) {
          ctx.strokeStyle = m.color
          ctx.globalAlpha = 0.5
          ctx.setLineDash([4, 4])
          ctx.lineWidth = 1
          ctx.beginPath()
          ctx.moveTo(x, y)
          ctx.lineTo(X(m.lineTo.e), Y(m.lineTo.n))
          ctx.stroke()
          ctx.setLineDash([])
          ctx.globalAlpha = 1
        }

        if (m.shape === 'vehicle') {
          drawVehicleGlyph(ctx, x, y, m.heading_deg ?? 0, m.color, m.filled !== false)
        } else if (m.shape === 'task') {
          drawTaskGlyph(ctx, x, y, m.color, m.filled)
        } else if (m.shape === 'home') {
          ctx.strokeStyle = m.color
          ctx.lineWidth = 1.5
          ctx.beginPath()
          ctx.arc(x, y, 3.5, 0, 2 * Math.PI)
          ctx.stroke()
        }

        // label
        ctx.font = '600 10px ui-sans-serif, system-ui, sans-serif'
        ctx.fillStyle = m.color
        ctx.textAlign = 'center'
        ctx.textBaseline = 'bottom'
        const label = m.sub ? `${m.label}` : m.label
        ctx.fillText(label, x, y - 12)
        if (m.sub) {
          ctx.font = '10px ui-sans-serif, system-ui, sans-serif'
          ctx.fillStyle = muted
          ctx.textBaseline = 'top'
          ctx.fillText(m.sub, x, y + 12)
        }
      }

      // ---- scale bar ----
      const barM = step * 2
      const barPx = barM * scale
      ctx.strokeStyle = fg
      ctx.fillStyle = fg
      ctx.lineWidth = 1.5
      const bx = w - 14 - barPx
      const by = h - 26
      ctx.beginPath()
      ctx.moveTo(bx, by)
      ctx.lineTo(bx + barPx, by)
      ctx.moveTo(bx, by - 3)
      ctx.lineTo(bx, by + 3)
      ctx.moveTo(bx + barPx, by - 3)
      ctx.lineTo(bx + barPx, by + 3)
      ctx.stroke()
      ctx.font = '10px ui-sans-serif, system-ui, sans-serif'
      ctx.textAlign = 'center'
      ctx.textBaseline = 'bottom'
      ctx.fillText(`${barM} m`, bx + barPx / 2, by - 5)
    }

    raf = requestAnimationFrame(draw)
    return () => {
      cancelAnimationFrame(raf)
      ro.disconnect()
    }
  }, [])

  return (
    <div ref={wrapRef} className={`relative h-64 w-full sm:h-72 ${className ?? ''}`}>
      <canvas ref={canvasRef} role="img" aria-label={ariaLabel} className="absolute inset-0 h-full w-full" />
    </div>
  )
}

function drawVehicleGlyph(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  headingDeg: number,
  color: string,
  filled: boolean,
) {
  ctx.save()
  ctx.translate(x, y)
  ctx.rotate((headingDeg * Math.PI) / 180) // 0 = north = up
  ctx.beginPath()
  ctx.moveTo(0, -8) // nose
  ctx.lineTo(6.5, 7)
  ctx.lineTo(0, 4)
  ctx.lineTo(-6.5, 7)
  ctx.closePath()
  if (filled) {
    ctx.fillStyle = color
    ctx.fill()
  }
  ctx.strokeStyle = color
  ctx.lineWidth = 1.5
  ctx.stroke()
  ctx.restore()
}

function drawTaskGlyph(ctx: CanvasRenderingContext2D, x: number, y: number, color: string, filled?: boolean) {
  ctx.save()
  ctx.translate(x, y)
  ctx.rotate(Math.PI / 4)
  const s = 6
  ctx.beginPath()
  ctx.rect(-s / 2, -s / 2, s, s)
  if (filled) {
    ctx.fillStyle = color
    ctx.fill()
  }
  ctx.strokeStyle = color
  ctx.lineWidth = 1.5
  ctx.stroke()
  ctx.restore()
}

function niceStep(raw: number): number {
  const pow = Math.pow(10, Math.floor(Math.log10(Math.max(1, raw))))
  for (const m of [1, 2, 5, 10]) {
    if (pow * m >= raw) return pow * m
  }
  return pow * 10
}
