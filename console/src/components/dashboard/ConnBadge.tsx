'use client'

/** LIVE / SIMULATED / CONNECTING badge + retry countdown (dual-mode contract). */

import { useEffect, useState } from 'react'
import { Loader2, RadioTower, TriangleAlert } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import type { ConnState } from '@/lib/types'

export function ConnBadge({
  conn,
  port,
  retryAt,
  lastError,
}: {
  conn: ConnState
  port: number
  retryAt: number | null
  lastError: string | null
}) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [])

  const retryIn = retryAt != null ? Math.max(0, Math.ceil((retryAt - now) / 1000)) : null

  if (conn === 'connecting') {
    return (
      <Badge variant="secondary" className="gap-1.5 border border-border" aria-live="polite">
        <Loader2 className="animate-spin" aria-hidden="true" />
        <span className="font-mono">:{port}</span>
        <span className="sr-only">connecting to backend</span>
        connecting
      </Badge>
    )
  }

  if (conn === 'live') {
    return (
      <Badge className="gap-1.5 border border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400" aria-live="polite">
        <RadioTower className="animate-pulse" aria-hidden="true" />
        <span className="font-mono">:{port}</span>
        LIVE
      </Badge>
    )
  }

  return (
    <Badge className="gap-1.5 border border-amber-600/40 bg-amber-500/10 text-amber-700 dark:text-amber-400" aria-live="polite">
      <TriangleAlert aria-hidden="true" />
      SIMULATED DATA — backend offline
      <span className="sr-only">using simulated telemetry; live endpoint unreachable</span>
    </Badge>
  )
}

/** One-line live-retry status under a card header. */
export function ConnSubline({
  conn,
  port,
  retryAt,
  lastError,
}: {
  conn: ConnState
  port: number
  retryAt: number | null
  lastError: string | null
}) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(id)
  }, [])
  if (conn === 'live') {
    return <p className="text-xs text-muted-foreground">10 Hz telemetry stream · WS /?XTransformPort={port} · REST /api/status</p>
  }
  if (conn === 'connecting') {
    return <p className="text-xs text-muted-foreground">probing gateway route :{port} …</p>
  }
  const retryIn = retryAt != null ? Math.max(0, Math.ceil((retryAt - now) / 1000)) : null
  return (
    <p className="text-xs text-muted-foreground">
      client-side simulation · live retry {retryIn != null ? `in ${retryIn} s` : 'pending'}
      {lastError ? ` · ${lastError}` : ''}
    </p>
  )
}
