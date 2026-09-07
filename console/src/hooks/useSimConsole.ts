'use client'

/**
 * rustsitsim console data hook (:8200).
 *
 * Lifecycle: probe REST /api/status through the gateway → if the Rust backend
 * answers, go LIVE and drive everything from the §4.2 WS stream (+ 2 s status
 * polling). If it does not, run the client-side SimMockEngine at 10 Hz and
 * keep retrying the live endpoint every 12 s (LIVE ⇄ SIMULATED is a required
 * dual mode, clearly surfaced in the UI). Commands (faults / estop) hit the
 * real REST plane when LIVE and the mock engine when SIMULATED, so the fault
 * console and estop are always operable.
 */

import { useCallback, useEffect, useRef, useState } from 'react'
import { gw, normalizeSimFrame, normalizeSimStatus, probePlane, wsUrl, fetchGw, unwrapEnvelope } from '@/lib/conn'
import { SimMockEngine } from '@/lib/mock-sim'
import { quatToEulerDeg } from '@/lib/format'
import type { ActiveFault, ConnState, PosSample, SimFrame, SimStatus } from '@/lib/types'

export const SIM_PORT = 8200

export interface AttSample {
  t: number
  roll: number
  pitch: number
  yaw: number
}

export interface SimSeries {
  track: PosSample[]
  alt: { t: number; v: number }[]
  att: AttSample[]
  motors: { t: number; v: number[] }[]
}

const EMPTY_SERIES: SimSeries = { track: [], alt: [], att: [], motors: [] }
const TRACK_CAP = 1800 // 3 min @ 10 Hz
const STRIP_CAP = 600 // 60 s @ 10 Hz

export function useSimConsole() {
  const [conn, setConn] = useState<ConnState>('connecting')
  const [lastError, setLastError] = useState<string | null>(null)
  const [retryAt, setRetryAt] = useState<number | null>(null)
  const [frame, setFrame] = useState<SimFrame | null>(null)
  const [status, setStatus] = useState<SimStatus | null>(null)
  const [series, setSeries] = useState<SimSeries>(EMPTY_SERIES)
  const [faults, setFaults] = useState<ActiveFault[]>([])
  const [frameCount, setFrameCount] = useState(0)

  const engineRef = useRef<SimMockEngine | null>(null)
  const connRef = useRef<ConnState>('connecting')
  const seriesRef = useRef<SimSeries>(EMPTY_SERIES)
  const frameRef = useRef<SimFrame | null>(null)

  const setConnBoth = useCallback((c: ConnState) => {
    connRef.current = c
    setConn(c)
  }, [])

  /** Ingest one frame (live WS or mock engine) into React state. */
  const applyFrame = useCallback((f: SimFrame) => {
    const s = seriesRef.current
    const t = f.t_us / 1e6
    const att = quatToEulerDeg(f.state.q_wxyz)

    const track = s.track.slice(-TRACK_CAP + 1)
    track.push({ t, n: f.state.pos_ned_m[0], e: f.state.pos_ned_m[1], d: f.state.pos_ned_m[2], yaw_deg: att.yaw_deg })

    const alt = s.alt.slice(-STRIP_CAP + 1)
    alt.push({ t, v: -f.state.pos_ned_m[2] })

    const attS = s.att.slice(-STRIP_CAP + 1)
    // unwrap yaw for a continuous strip-chart line (no 359°→1° jumps)
    const prev = attS[attS.length - 1]
    let yawDisp = att.yaw_deg
    if (prev) {
      while (yawDisp - prev.yaw > 180) yawDisp -= 360
      while (yawDisp - prev.yaw < -180) yawDisp += 360
    }
    attS.push({ t, roll: att.roll_deg, pitch: att.pitch_deg, yaw: yawDisp })

    const motors = s.motors.slice(-STRIP_CAP + 1)
    motors.push({ t, v: f.state.motors })

    const next: SimSeries = { track, alt, att: attS, motors }
    seriesRef.current = next
    frameRef.current = f
    setSeries(next)
    setFrame(f)
    setFaults(f.faults_active)
    setFrameCount((c) => c + 1)
  }, [])

  useEffect(() => {
    let cancelled = false
    let ws: WebSocket | null = null
    let mockTimer: ReturnType<typeof setInterval> | null = null
    let statusTimer: ReturnType<typeof setInterval> | null = null
    let retryTimer: ReturnType<typeof setInterval> | null = null
    let wsFailures = 0
    let pollFailures = 0

    const engine = new SimMockEngine()
    engineRef.current = engine

    const clearMock = () => {
      if (mockTimer) {
        clearInterval(mockTimer)
        mockTimer = null
      }
    }
    const clearStatus = () => {
      if (statusTimer) {
        clearInterval(statusTimer)
        statusTimer = null
      }
    }
    const teardownLive = () => {
      if (ws) {
        ws.onclose = null
        ws.onerror = null
        ws.onmessage = null
        try {
          ws.close()
        } catch {
          /* already closed */
        }
        ws = null
      }
      clearStatus()
    }

    const startMock = (reason: string) => {
      if (cancelled) return
      teardownLive()
      setConnBoth('simulated')
      setLastError(reason)
      setRetryAt(Date.now() + 12000)
      if (mockTimer) return
      // boot the mock from a clean state
      const fresh = new SimMockEngine()
      engineRef.current = fresh
      seriesRef.current = EMPTY_SERIES
      setSeries(EMPTY_SERIES)
      mockTimer = setInterval(() => {
        const f = fresh.tick(0.1)
        applyFrame(f)
        setStatus(fresh.status())
      }, 100)
    }

    const openWs = () => {
      if (cancelled) return
      let opened = false
      try {
        ws = new WebSocket(wsUrl(SIM_PORT))
      } catch {
        startMock('WS construction failed')
        return
      }
      ws.onmessage = (ev) => {
        opened = true
        wsFailures = 0
        try {
          const raw = JSON.parse(typeof ev.data === 'string' ? ev.data : '{}')
          applyFrame(normalizeSimFrame(raw, frameRef.current))
        } catch {
          /* malformed frame — skip */
        }
      }
      ws.onclose = () => {
        if (cancelled) return
        wsFailures++
        if (wsFailures > 3) {
          startMock('WS stream closed — backend offline')
          return
        }
        setTimeout(() => {
          if (!cancelled && connRef.current === 'live') openWs()
        }, 2500)
      }
      ws.onerror = () => {
        // onclose follows; nothing to do here
      }
      // if no frame arrives within 5 s, REST still works → keep status only
      setTimeout(() => {
        if (!cancelled && !opened && connRef.current === 'live' && ws) {
          setLastError('WS stream silent — status via REST polling only')
        }
      }, 5000)
    }

    const pollStatus = () => {
      fetchGw(gw(SIM_PORT, '/api/status'), { method: 'GET' }, 2500)
        .then(async (res) => {
          if (!res.ok) throw new Error(`HTTP ${res.status}`)
          const j = await res.json()
          if (cancelled) return
          pollFailures = 0
          setStatus(normalizeSimStatus(unwrapEnvelope(j), null))
          setRetryAt(null)
        })
        .catch(() => {
          if (cancelled) return
          pollFailures++
          if (pollFailures >= 2 && connRef.current === 'live') {
            startMock('REST status polling failed — backend offline')
          }
        })
    }

    const goLive = () => {
      if (cancelled) return
      clearMock()
      setConnBoth('live')
      setLastError(null)
      setRetryAt(null)
      pollStatus()
      openWs()
      statusTimer = setInterval(pollStatus, 2000)
    }

    // ---- initial attempt + periodic live retry (dual mode requirement) ----
    probePlane(gw(SIM_PORT, '/api/status')).then((alive) => {
      if (cancelled) return
      if (alive) goLive()
      else startMock('backend :8200 unreachable')
    })

    retryTimer = setInterval(() => {
      if (cancelled || connRef.current === 'live') return
      probePlane(gw(SIM_PORT, '/api/status'), 2000).then((alive) => {
        if (cancelled || !alive || connRef.current === 'live') return
        goLive()
      })
      if (connRef.current === 'simulated') setRetryAt(Date.now() + 12000)
    }, 12000)

    return () => {
      cancelled = true
      teardownLive()
      clearMock()
      if (retryTimer) clearInterval(retryTimer)
    }
  }, [applyFrame, setConnBoth])

  // ---------------------------------------------------------------- commands

  const injectFault = useCallback(
    async (params: Record<string, number | string>): Promise<boolean> => {
      if (connRef.current === 'live') {
        try {
          const res = await fetchGw(gw(SIM_PORT, '/api/faults'), {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(params),
          })
          return res.ok
        } catch {
          return false
        }
      }
      const engine = engineRef.current
      if (!engine) return false
      engine.injectFault(params)
      return true
    },
    [],
  )

  const clearFault = useCallback(async (id: string): Promise<boolean> => {
    if (connRef.current === 'live') {
      try {
        const res = await fetchGw(gw(SIM_PORT, `/api/faults/${encodeURIComponent(id)}`), { method: 'DELETE' })
        return res.ok
      } catch {
        return false
      }
    }
    return engineRef.current?.clearFault(id) ?? false
  }, [])

  const estop = useCallback(async (): Promise<boolean> => {
    if (connRef.current === 'live') {
      try {
        const res = await fetchGw(gw(SIM_PORT, '/api/estop'), { method: 'POST' })
        return res.ok
      } catch {
        return false
      }
    }
    engineRef.current?.estop()
    return true
  }, [])

  return {
    conn,
    port: SIM_PORT,
    lastError,
    retryAt,
    frame,
    status,
    series,
    faults,
    frameCount,
    injectFault,
    clearFault,
    estop,
  }
}

export type SimConsoleApi = ReturnType<typeof useSimConsole>
