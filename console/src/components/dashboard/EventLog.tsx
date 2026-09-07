'use client'

/**
 * Safety & event log (mavfleet SPEC §5.2 / §13): the supervisor's streaming
 * event list as a severity-colored, filterable table + the §8.1 policy ladder.
 */

import { useState } from 'react'
import { Info, OctagonAlert, ShieldAlert, TriangleAlert } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ScrollArea } from '@/components/ui/scroll-area'
import { fmtWallClock } from '@/lib/format'
import type { FleetEvent, FleetVehicle } from '@/lib/types'
import { cn } from '@/lib/utils'

type FilterKind = 'all' | 'safety' | 'allocation' | 'link' | 'fsm' | 'run'

const FILTERS: { id: FilterKind; label: string; kinds: string[] }[] = [
  { id: 'all', label: 'All', kinds: [] },
  { id: 'safety', label: 'Safety', kinds: ['supervisor', 'fault'] },
  { id: 'allocation', label: 'Allocation', kinds: ['award', 'auction'] },
  { id: 'link', label: 'Link', kinds: ['link'] },
  { id: 'fsm', label: 'FSM', kinds: ['fsm'] },
  { id: 'run', label: 'Run', kinds: ['boundary'] },
]

export function EventLog({ events }: { events: FleetEvent[] }) {
  const [filter, setFilter] = useState<FilterKind>('all')
  const active = FILTERS.find((f) => f.id === filter) ?? FILTERS[0]
  const shown = (active.kinds.length === 0 ? events : events.filter((e) => active.kinds.includes(e.kind))).slice(-120).reverse()

  return (
    <div className="flex min-h-0 flex-col gap-2">
      <div className="flex flex-wrap items-center gap-1.5" role="group" aria-label="Event log filter">
        {FILTERS.map((f) => (
          <Button
            key={f.id}
            variant={filter === f.id ? 'secondary' : 'ghost'}
            size="sm"
            className="h-7 rounded-full px-2.5 text-xs"
            aria-pressed={filter === f.id}
            onClick={() => setFilter(f.id)}
          >
            {f.label}
          </Button>
        ))}
        <Badge variant="secondary" className="ml-auto font-mono text-[10px]">{events.length} events</Badge>
      </div>

      <ScrollArea className="max-h-96 flex-1 rounded-lg border border-border">
        <ul className="divide-y divide-border" aria-live="polite" aria-label="Fleet event log">
          {shown.length === 0 && (
            <li className="px-3 py-8 text-center text-sm text-muted-foreground">No events match this filter yet</li>
          )}
          {shown.map((e, i) => (
            <li key={`${e.t}-${i}`} className="flex items-start gap-2.5 px-3 py-2">
              <span className="mt-0.5 shrink-0" aria-hidden="true">
                {e.severity === 'critical' ? (
                  <OctagonAlert className="size-3.5 text-rose-600 dark:text-rose-400" />
                ) : e.severity === 'warn' ? (
                  <TriangleAlert className="size-3.5 text-amber-600 dark:text-amber-400" />
                ) : (
                  <Info className="size-3.5 text-muted-foreground" />
                )}
              </span>
              <div className="min-w-0 flex-1">
                <p className={cn('text-xs leading-snug', e.severity === 'critical' && 'font-medium text-rose-700 dark:text-rose-400', e.severity === 'warn' && 'text-amber-800 dark:text-amber-300')}>
                  {e.detail}
                </p>
                <p className="mt-0.5 flex flex-wrap items-center gap-x-2 font-mono text-[9px] text-muted-foreground">
                  <span>{fmtWallClock(e.t)}</span>
                  <span>t+{e.t_s.toFixed(1)}s</span>
                  <span className="uppercase">{e.kind}</span>
                  {e.vehicle && <span>{e.vehicle}</span>}
                </p>
              </div>
            </li>
          ))}
        </ul>
      </ScrollArea>
    </div>
  )
}

/** §8.1 escalation ladder with live triggered-state highlight. */
export function SafetyLadder({ vehicles, aborted }: { vehicles: FleetVehicle[]; aborted: boolean }) {
  const anyFlag = (name: string) => vehicles.some((v) => v.health.includes(name))
  const active = vehicles.filter((v) => v.fsm === 'ACTIVE')
  const separation = active.some((a, i) =>
    active.some((b, j) => {
      if (i >= j) return false
      const dh = Math.hypot(a.position_ned_m[0] - b.position_ned_m[0], a.position_ned_m[1] - b.position_ned_m[1])
      const dv = Math.abs(a.position_ned_m[2] - b.position_ned_m[2])
      return dh < 4 && dv < 2
    }),
  )

  const policies: { n: number; name: string; condition: string; action: string; triggered: boolean }[] = [
    { n: 1, name: 'E-stop', condition: 'operator estop', action: 'LAND all, ABORT', triggered: aborted },
    { n: 2, name: 'Geofence breach', condition: 'outside polygon / above ceiling', action: 'RTL · LAND if >10 m out', triggered: anyFlag('GEOFENCE_WARN') },
    { n: 3, name: 'Heartbeat loss', condition: 'no heartbeat 3 s', action: 'RTL · LAND retry at 10 s', triggered: anyFlag('HEARTBEAT_LOST') },
    { n: 4, name: 'Battery critical', condition: '< 20 %', action: 'LAND', triggered: anyFlag('BATTERY_CRIT') },
    { n: 5, name: 'Battery low', condition: '< 30 %', action: 'RTL after current task', triggered: anyFlag('BATTERY_LOW') },
    { n: 6, name: 'Staleness', condition: 'telemetry stale 1.5 s', action: 'flag · RTL if ACTIVE >5 s', triggered: anyFlag('LINK_STALE') },
    { n: 7, name: 'Separation', condition: 'ACTIVE within 4 m H / 2 m V', action: 'altitude-divergence override', triggered: separation },
    { n: 8, name: 'Command failure', condition: '3 consecutive failures', action: 'RTL · FAULT if path dead', triggered: false },
  ]

  return (
    <div className="flex flex-col gap-2">
      <h4 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-muted-foreground">
        <ShieldAlert className="size-3.5" aria-hidden="true" /> Safety ladder (§8.1)
      </h4>
      <ul className="flex flex-col gap-1">
        {policies.map((p) => (
          <li
            key={p.n}
            className={cn(
              'flex items-start gap-2 rounded-md border px-2.5 py-1.5',
              p.triggered ? 'border-amber-500/50 bg-amber-500/10' : 'border-border/60 bg-card/40',
            )}
          >
            <span className={cn('mt-0.5 font-mono text-[10px] font-semibold', p.triggered ? 'text-amber-700 dark:text-amber-400' : 'text-muted-foreground')}>
              {p.n}
            </span>
            <div className="min-w-0 flex-1">
              <p className={cn('text-xs font-medium', p.triggered && 'text-amber-800 dark:text-amber-300')}>
                {p.name}
                {p.triggered && <span className="ml-1.5 text-[9px] font-semibold uppercase tracking-wide">triggered</span>}
              </p>
              <p className="font-mono text-[9px] text-muted-foreground">
                {p.condition} → {p.action}
              </p>
            </div>
          </li>
        ))}
      </ul>
    </div>
  )
}
