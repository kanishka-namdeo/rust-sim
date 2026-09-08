'use client'

/**
 * Mode 2 — mavfleet Fleet C2.
 * Fleet map with geofence, fleet table, task/allocation board, safety & event
 * log, mission bindings (§5.4 AC-5.4.1), Start Fleet modal (§5.4 AC-5.4.2),
 * swarming-pattern library (§5.4 AC-5.4.3), and the single fleet abort
 * surface: E-STOP (SPEC §3.4 / §13).
 */

import { useCallback, useState } from 'react'
import { OctagonAlert, Radar, Satellite, Timer, Waypoints } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Skeleton } from '@/components/ui/skeleton'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { useToast } from '@/hooks/use-toast'
import { useFleetC2 } from '@/hooks/useFleetC2'
import { ConnBadge, ConnSubline } from './ConnBadge'
import { EventLog, SafetyLadder } from './EventLog'
import { FleetTable } from './FleetTable'
import { MissionBindingsPanel } from './MissionBindingsPanel'
import { NedMap, type MapMarker } from './NedMap'
import { PatternsDialog } from './PatternsDialog'
import { StartFleetDialog } from './StartFleetDialog'
import { StatTile } from './StatTile'
import { TaskPanel } from './TaskPanel'
import { fmt } from '@/lib/format'
import { fsmStyle } from '@/lib/fsm'
import type { FleetTask } from '@/lib/types'

export function FleetC2() {
  const fleet = useFleetC2()
  const { toast } = useToast()
  const [selected, setSelected] = useState<string | null>(null)
  const snap = fleet.snapshot

  const doEstop = useCallback(async () => {
    const ok = await fleet.estop()
    if (ok) {
      toast({
        title: 'Fleet E-STOP sent',
        description:
          fleet.conn === 'live'
            ? 'POST /api/fleet/estop?XTransformPort=8400 accepted — every vehicle LANDs, run ABORTED'
            : 'internal fleet simulator — every vehicle LANDs, run ABORTED',
      })
    } else {
      toast({ title: 'Fleet E-STOP failed', description: 'backend rejected POST /api/fleet/estop', variant: 'destructive' })
    }
  }, [fleet, toast])

  const connecting = fleet.conn === 'connecting' && !snap
  const aborted = snap?.phase === 'ABORTED'
  const counts = countByFsm(snap?.vehicles ?? [])

  // ---- map markers -------------------------------------------------------
  const markers: MapMarker[] = []
  const taskById = new Map((snap?.tasks ?? []).map((t) => [t.id, t]))
  for (const v of snap?.vehicles ?? []) {
    const st = fsmStyle(v.fsm)
    const task = v.task_id ? taskById.get(v.task_id) : undefined
    markers.push({
      n: v.position_ned_m[0],
      e: v.position_ned_m[1],
      label: v.id,
      sub: `${fmt(v.battery_pct, 0)}%`,
      color: st.hex,
      shape: 'vehicle',
      heading_deg: v.yaw_deg,
      filled: v.fsm === 'ACTIVE' || v.fsm === 'RTL',
      lineTo:
        v.fsm === 'ACTIVE' && task
          ? { n: task.pos_ned_m[0], e: task.pos_ned_m[1] }
          : null,
    })
  }
  for (const t of snap?.tasks ?? []) {
    markers.push({
      n: t.pos_ned_m[0],
      e: t.pos_ned_m[1],
      label: t.id,
      sub: t.status === 'done' ? 'done' : t.assigned_to ?? 'pending',
      color: taskColor(t),
      shape: 'task',
      filled: t.status === 'in_progress' || t.status === 'done',
    })
  }
  const trails = (snap?.vehicles ?? []).map((v) => ({ color: fsmStyle(v.fsm).hex, points: v.breadcrumb }))

  return (
    <div className="flex flex-col gap-4">
      {/* -------------------------------------------------- fleet header */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <CardTitle className="flex flex-wrap items-center gap-2.5 text-base">
                <Radar className="size-4 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
                mavfleet — Fleet C2
                <PhaseBadgeFleet phase={snap?.phase ?? 'CONNECTING'} />
                <ConnBadge conn={fleet.conn} port={fleet.port} retryAt={fleet.retryAt} lastError={fleet.lastError} />
              </CardTitle>
              <div className="mt-1 text-xs text-muted-foreground">
                <ConnSubline conn={fleet.conn} port={fleet.port} retryAt={fleet.retryAt} lastError={fleet.lastError} />
              </div>
            </div>
            <div className="flex flex-wrap items-center gap-1.5">
              <PatternsDialog fleet={fleet} disabled={aborted} />
              <StartFleetDialog fleet={fleet} disabled={aborted || fleet.bindings.length === 0} />
              <AlertDialog>
                <AlertDialogTrigger asChild>
                  <Button variant="destructive" size="sm" className="gap-2">
                    <OctagonAlert className="size-4" aria-hidden="true" />
                    FLEET E-STOP
                  </Button>
                </AlertDialogTrigger>
                <AlertDialogContent>
                  <AlertDialogHeader>
                    <AlertDialogTitle>Confirm fleet E-STOP</AlertDialogTitle>
                    <AlertDialogDescription>
                      Every vehicle LANDs immediately and the run is marked ABORTED. Policy 1 short-circuits the whole
                      escalation ladder.
                    </AlertDialogDescription>
                  </AlertDialogHeader>
                  <AlertDialogFooter>
                    <AlertDialogCancel>Cancel</AlertDialogCancel>
                    <AlertDialogAction
                      onClick={(e) => {
                        e.preventDefault()
                        void doEstop()
                      }}
                      className="bg-destructive text-white hover:bg-destructive/90"
                    >
                      E-STOP all vehicles
                    </AlertDialogAction>
                  </AlertDialogFooter>
                </AlertDialogContent>
              </AlertDialog>
            </div>
          </div>
        </CardHeader>
        <CardContent>
          {connecting ? (
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-5">
              {Array.from({ length: 5 }).map((_, i) => (
                <Skeleton key={i} className="h-16 rounded-lg" />
              ))}
            </div>
          ) : (
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-5">
              <StatTile label="Fleet phase" value={snap?.phase ?? '—'} tone={aborted ? 'crit' : 'ok'} />
              <StatTile label="Vehicles" value={snap?.vehicles.length ?? 0} sub={`${counts.ACTIVE ?? 0} ACTIVE · ${counts.READY ?? 0} READY`} />
              <StatTile label="Sim time" value={`${fmt(snap?.t_s, 1)} s`} sub="virtual clock" icon={<Timer aria-hidden="true" />} />
              <StatTile label="Tasks" value={snap?.tasks.length ?? 0} sub={`${snap?.tasks.filter((t) => t.status === 'done').length ?? 0} done`} icon={<Waypoints aria-hidden="true" />} />
              <StatTile
                label="Geofence"
                value={snap ? `±${fmt(Math.abs(snap.geofence.points[1]?.[0] ?? 45), 0)} m` : '—'}
                sub={snap ? `ceiling ${snap.geofence.ceiling_m} m · floor ${snap.geofence.floor_m} m` : ''}
                icon={<Satellite aria-hidden="true" />}
              />
            </div>
          )}
        </CardContent>
      </Card>

      {/* ----------------------------------------------- map + task board */}
      <div className="grid min-w-0 gap-4 xl:grid-cols-3">
        <Card className="xl:col-span-2">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm">Fleet map — NED north-up</CardTitle>
            <CardDescription>
              geofence polygon · vehicles colored by FSM · 2 s breadcrumbs · task markers with assignment lines
            </CardDescription>
          </CardHeader>
          <CardContent>
            {connecting ? (
              <Skeleton className="h-64 w-full rounded-lg sm:h-72" />
            ) : snap ? (
              <NedMap
                markers={markers}
                trails={trails}
                polygon={snap.geofence.points}
                polygonLabel="geofence"
                minHalfExtent={52}
                ariaLabel="Fleet map: geofence polygon with vehicle and task markers in NED frame"
              />
            ) : (
              <div className="flex h-64 flex-col items-center justify-center gap-1.5 rounded-lg border border-dashed border-border text-center sm:h-72">
                <Radar className="size-5 text-muted-foreground" aria-hidden="true" />
                <p className="text-sm font-medium text-muted-foreground">Waiting for fleet frames</p>
                <p className="text-xs text-muted-foreground/80">WS /ws/fleet silent — /api/fleet is polled as fallback</p>
              </div>
            )}
          </CardContent>
        </Card>

        <Card className="min-w-0">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm">Tasks &amp; allocation</CardTitle>
            <CardDescription>contract-net auctions · sequential single-item (§6.3)</CardDescription>
          </CardHeader>
          <CardContent className="min-w-0">
            {connecting ? <Skeleton className="h-72 rounded-lg" /> : <TaskPanel tasks={snap?.tasks ?? []} auctions={fleet.auctions} />}
          </CardContent>
        </Card>
      </div>

      {/* ----------------------------------------------- mission bindings */}
      <MissionBindingsPanel fleet={fleet} vehicles={snap?.vehicles ?? []} />

      {/* ------------------------------------------------------ fleet table */}
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-sm">Fleet table</CardTitle>
          <CardDescription>click a row to track a vehicle · heartbeat ages refresh at 10 Hz</CardDescription>
        </CardHeader>
        <CardContent>
          {connecting ? (
            <Skeleton className="h-40 rounded-lg" />
          ) : (
            <FleetTable
              vehicles={snap?.vehicles ?? []}
              tasks={snap?.tasks ?? []}
              selected={selected}
              onSelect={(id) => setSelected((s) => (s === id ? null : id))}
            />
          )}
        </CardContent>
      </Card>

      {/* ----------------------------------------------- event log + safety */}
      <div className="grid min-w-0 gap-4 xl:grid-cols-3">
        <Card className="xl:col-span-2">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm">Safety &amp; event log</CardTitle>
            <CardDescription>the operator&apos;s flight recorder — rising-edge decisions only (§5.2)</CardDescription>
          </CardHeader>
          <CardContent className="min-w-0">
            <EventLog events={fleet.events} />
          </CardContent>
        </Card>
        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm">Safety supervisor</CardTitle>
            <CardDescription>ordered policy ladder, first hit short-circuits (§8.1)</CardDescription>
          </CardHeader>
          <CardContent>
            <SafetyLadder vehicles={snap?.vehicles ?? []} aborted={aborted} />
          </CardContent>
        </Card>
      </div>
    </div>
  )
}

function PhaseBadgeFleet({ phase }: { phase: string }) {
  const p = (phase ?? '').toUpperCase()
  const cls =
    p === 'RUNNING'
      ? 'border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400'
      : p === 'INIT'
        ? 'border-amber-600/40 bg-amber-500/10 text-amber-700 dark:text-amber-400'
        : p === 'ABORTED'
          ? 'border-rose-600/40 bg-rose-500/10 text-rose-700 dark:text-rose-400'
          : 'border-border bg-muted text-muted-foreground'
  return (
    <Badge variant="outline" className={`font-mono ${cls}`} aria-label={`Fleet phase ${p}`}>
      {p}
    </Badge>
  )
}

function countByFsm(vehicles: { fsm: string }[]): Record<string, number> {
  const out: Record<string, number> = {}
  for (const v of vehicles) out[v.fsm] = (out[v.fsm] ?? 0) + 1
  return out
}

function taskColor(t: FleetTask): string {
  switch (t.status) {
    case 'pending':
      return '#f59e0b'
    case 'assigned':
      return '#fb923c'
    case 'in_progress':
      return '#10b981'
    case 'done':
      return '#a1a1aa'
    default:
      return '#f43f5e'
  }
}
