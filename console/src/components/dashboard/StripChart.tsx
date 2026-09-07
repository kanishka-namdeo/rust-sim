'use client'

/**
 * Rolling-window strip chart on canvas 2D (SPEC rustsitsim §12.1: 60 s windows
 * of altitude / attitude / motor commands — no charting framework).
 */

import { useEffect, useRef } from 'react'

export interface StripSeries {
  label: string
  color: string
  data: { t: number; v: number }[]
}

export interface StripChartProps {
  series: StripSeries[]
  windowSec?: number
  unit?: string
  minV?: number
  maxV?: number
  className?: string
  ariaLabel: string
}

const PAD_L = 44
const PAD_R = 10
const PAD_T = 20
const PAD_B = 18

export function StripChart({ series, windowSec = 60, unit, minV, maxV, className, ariaLabel }: StripChartProps) {
  const wrapRef = useRef<HTMLDivElement>(null)
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const propsRef = useRef({ series, windowSec, unit, minV, maxV })

  useEffect(() => {
    propsRef.current = { series, windowSec, unit, minV, maxV }
  }, [series, windowSec, unit, minV, maxV])

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
      if (!ctx || w < 40 || h < 30) return
      const { series: sers, windowSec: win, unit: unitLbl, minV: fixedMin, maxV: fixedMax } = propsRef.current

      const muted = cssVar('--muted-foreground', '#71717a')
      const border = cssVar('--border', '#e4e4e7')
      const fg = cssVar('--foreground', '#18181b')

      ctx.clearRect(0, 0, w, h)

      // time window
      let tNow = -Infinity
      for (const s of sers) {
        const last = s.data[s.data.length - 1]
        if (last && Number.isFinite(last.t)) tNow = Math.max(tNow, last.t)
      }
      if (!Number.isFinite(tNow)) tNow = 0
      const t0 = tNow - win

      // value range
      let lo = fixedMin ?? Infinity
      let hi = fixedMax ?? -Infinity
      if (fixedMin == null || fixedMax == null) {
        for (const s of sers) {
          for (const p of s.data) {
            if (p.t < t0 || !Number.isFinite(p.v)) continue
            if (fixedMin == null) lo = Math.min(lo, p.v)
            if (fixedMax == null) hi = Math.max(hi, p.v)
          }
        }
        if (!Number.isFinite(lo) || !Number.isFinite(hi)) {
          lo = 0
          hi = 1
        }
        if (hi - lo < 1e-9) {
          hi += 0.5
          lo -= 0.5
        }
        const pad = (hi - lo) * 0.12
        lo -= pad
        hi += pad
      }

      const plotX0 = PAD_L
      const plotX1 = w - PAD_R
      const plotY0 = PAD_T
      const plotY1 = h - PAD_B
      const plotW = plotX1 - plotX0
      const plotH = plotY1 - plotY0

      const X = (t: number) => plotX0 + ((t - t0) / win) * plotW
      const Y = (v: number) => plotY1 - ((v - lo) / (hi - lo)) * plotH

      // horizontal grid + y labels
      ctx.font = '10px ui-sans-serif, system-ui, sans-serif'
      ctx.textBaseline = 'middle'
      for (let i = 0; i <= 4; i++) {
        const v = lo + ((hi - lo) * i) / 4
        const y = Y(v)
        ctx.strokeStyle = border
        ctx.globalAlpha = i === 0 || i === 4 ? 0.9 : 0.5
        ctx.lineWidth = 1
        ctx.beginPath()
        ctx.moveTo(plotX0, y)
        ctx.lineTo(plotX1, y)
        ctx.stroke()
        ctx.globalAlpha = 1
        ctx.fillStyle = muted
        ctx.textAlign = 'right'
        ctx.fillText(fmtTick(v), plotX0 - 6, y)
      }

      // vertical grid + x labels (-60s .. now)
      ctx.textAlign = 'center'
      ctx.textBaseline = 'top'
      for (let i = 0; i <= 6; i++) {
        const t = t0 + (win * i) / 6
        const x = X(t)
        ctx.strokeStyle = border
        ctx.globalAlpha = 0.35
        ctx.beginPath()
        ctx.moveTo(x, plotY0)
        ctx.lineTo(x, plotY1)
        ctx.stroke()
        ctx.globalAlpha = 1
        ctx.fillStyle = muted
        const rel = Math.round(t - tNow)
        ctx.fillText(rel === 0 ? 'now' : `${rel}s`, x, plotY1 + 4)
      }

      // series
      for (const s of sers) {
        if (s.data.length < 2) continue
        const stride = Math.max(1, Math.ceil(s.data.length / 300))
        ctx.strokeStyle = s.color
        ctx.lineWidth = 1.75
        ctx.lineJoin = 'round'
        ctx.beginPath()
        let started = false
        for (let i = 0; i < s.data.length; i += stride) {
          const p = s.data[i]
          if (p.t < t0 - 2 || !Number.isFinite(p.v)) continue
          const x = X(p.t)
          const y = Y(p.v)
          if (!started) {
            ctx.moveTo(x, y)
            started = true
          } else ctx.lineTo(x, y)
        }
        const lastP = s.data[s.data.length - 1]
        if (lastP && Number.isFinite(lastP.v) && lastP.t >= t0) ctx.lineTo(X(lastP.t), Y(lastP.v))
        ctx.stroke()
      }

      // legend (top-right, in-plot)
      let lx = w - PAD_R - 4
      ctx.textBaseline = 'middle'
      ctx.textAlign = 'right'
      for (let i = sers.length - 1; i >= 0; i--) {
        const s = sers[i]
        const label = unitLbl ? `${s.label} (${unitLbl})` : s.label
        ctx.font = '10px ui-sans-serif, system-ui, sans-serif'
        const tw = ctx.measureText(label).width
        ctx.fillStyle = s.color
        ctx.fillRect(lx - tw - 14, PAD_T / 2 - 2 - 4, 8, 8)
        ctx.fillStyle = fg
        ctx.fillText(label, lx - 4, PAD_T / 2)
        lx -= tw + 26
      }
    }

    raf = requestAnimationFrame(draw)
    return () => {
      cancelAnimationFrame(raf)
      ro.disconnect()
    }
  }, [])

  return (
    <div ref={wrapRef} className={`relative h-36 w-full sm:h-40 ${className ?? ''}`}>
      <canvas ref={canvasRef} role="img" aria-label={ariaLabel} className="absolute inset-0 h-full w-full" />
    </div>
  )
}

function fmtTick(v: number): string {
  const a = Math.abs(v)
  if (a >= 100) return v.toFixed(0)
  if (a >= 10) return v.toFixed(0)
  if (a >= 1) return v.toFixed(1)
  return v.toFixed(2)
}
