'use client'

/**
 * Mode 1 — rustsitsim Sim Console.
 * Live telemetry, sensor panel, status header, trajectory + strip charts,
 * fault console and E-STOP (SPEC §4 + §12).
 */

import { useCallback } from 'react'
import { Activity, BatteryFull, Compass, Gauge, OctagonAlert, Satellite, Wind } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Progress } from '@/components/ui/progress'
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
import { useSimConsole } from '@/hooks/useSimConsole'
import { ConnBadge, ConnSubline } from './ConnBadge'
import { FaultConsole } from './FaultConsole'
import { NedMap } from './NedMap'
import { StatTile } from './StatTile'
import { StripChart } from './StripChart'
import { batteryTone, clamp, fmt, fmtMissionClock, fmtSigned, fmtVec, quatToEulerDeg } from '@/lib/format'
import type { SimPhase } from '@/lib/types'

const EMERALD = '#10b981'
const AMBER = '#f59e0b'
const ORANGE = '#fb923c'
const ZINC = '#a1a1aa'

export function SimConsole() {
  const sim = useSimConsole()
  const { toast } = useToast()
  const frame = sim.frame
  const att = frame ? quatToEulerDeg(frame.state.q_wxyz) : null
  const phase = sim.status?.phase ?? frame?.phase ?? '—'
  const px4 = sim.status?.px4_connected ?? false
  const loopClosed = sim.status?.loop_closed ?? false
  const battery = frame?.state.battery_pct ?? null
  const batTone = batteryTone(battery)

  const doEstop = useCallback(async () => {
    const ok = await sim.estop()
    if (ok) {
      toast({
        title: 'E-STOP sent',
        description:
          sim.conn === 'live'
            ? 'POST /api/estop?XTransformPort=8200 accepted — run stops at end of current tick'
            : 'internal simulator stopped — new sortie after cool-down',
      })
    } else {
      toast({ title: 'E-STOP failed', description: 'backend rejected POST /api/estop', variant: 'destructive' })
    }
  }, [sim, toast])

  const connecting = sim.conn === 'connecting' && !frame

  return (
    <div className="flex flex-col gap-4">
      {/* -------------------------------------------------- status header */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <CardTitle className="flex flex-wrap items-center gap-2.5 text-base">
                <Activity className="size-4 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
                rustsitsim — PX4 lockstep HIL
                <PhaseBadge phase={phase} />
                <ConnBadge conn={sim.conn} port={sim.port} retryAt={sim.retryAt} lastError={sim.lastError} />
              </CardTitle>
              <div className="mt-1 text-xs text-muted-foreground">
                <ConnSubline conn={sim.conn} port={sim.port} retryAt={sim.retryAt} lastError={sim.lastError} />
              </div>
            </div>
            <AlertDialog>
              <AlertDialogTrigger asChild>
                <Button variant="destructive" size="sm" className="gap-2">
                  <OctagonAlert className="size-4" aria-hidden="true" />
                  E-STOP
                </Button>
              </AlertDialogTrigger>
              <AlertDialogContent>
                <AlertDialogHeader>
                  <AlertDialogTitle>Confirm E-STOP</AlertDialogTitle>
                  <AlertDialogDescription>
                    Stops the run at the end of the current tick and flushes the replay. Motors spool down immediately.
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
                    E-STOP now
                  </AlertDialogAction>
                </AlertDialogFooter>
              </AlertDialogContent>
            </AlertDialog>
          </div>
        </CardHeader>
        <CardContent>
          {connecting ? (
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-6">
              {Array.from({ length: 6 }).map((_, i) => (
                <Skeleton key={i} className="h-16 rounded-lg" />
              ))}
            </div>
          ) : (
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-6">
              <StatTile label="Virtual time" value={fmtMissionClock(sim.status?.t_us ?? frame?.t_us)} sub="mm:ss.s" />
              <StatTile
                label="PX4 link"
                value={px4 ? 'CONNECTED' : 'DOWN'}
                tone={px4 ? 'ok' : 'crit'}
                sub="TCP lockstep master"
              />
              <StatTile
                label="Loop"
                value={loopClosed ? 'CLOSED' : 'OPEN'}
                tone={loopClosed ? 'ok' : 'warn'}
                sub="HIL sensor ⇄ actuator"
              />
              <StatTile
                label="Tick p95"
                value={fmt(sim.status?.tick_p95_us ?? frame?.stats.tick_p95_us, 0)}
                unit="µs"
                sub="sim-loop latency"
              />
              <StatTile label="Rate" value={fmt(sim.status?.rate_hz, 1)} unit="Hz" sub="control plane" />
              <StatTile
                label="Battery"
                value={fmt(battery, 1)}
                unit="%"
                tone={batTone === 'ok' ? 'ok' : batTone === 'warn' ? 'warn' : batTone === 'crit' ? 'crit' : 'default'}
                sub={batTone === 'crit' ? 'critical reserve' : batTone === 'warn' ? 'low' : 'nominal'}
              />
            </div>
          )}
        </CardContent>
      </Card>

      {/* ---------------------------------------------- trajectory + telemetry */}
      <div className="grid gap-4 xl:grid-cols-3">
        <Card className="xl:col-span-2">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm">Trajectory — NED top-down (north up)</CardTitle>
            <CardDescription>
              position history · {sim.series.track.length} samples · decimated to 300 points on draw
            </CardDescription>
          </CardHeader>
          <CardContent>
            {connecting ? (
              <Skeleton className="h-64 w-full rounded-lg sm:h-72" />
            ) : frame ? (
              <NedMap
                track={sim.series.track}
                markers={[
                  {
                    n: frame.state.pos_ned_m[0],
                    e: frame.state.pos_ned_m[1],
                    label: 'SIM',
                    sub: `${fmt(-frame.state.pos_ned_m[2], 1)} m AGL`,
                    color: EMERALD,
                    shape: 'vehicle',
                    heading_deg: att?.yaw_deg ?? 0,
                  },
                ]}
                minHalfExtent={26}
                ariaLabel="Top-down NED trajectory of the simulated vehicle"
              />
            ) : (
              <MapEmptyState />
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm">Telemetry — ground truth</CardTitle>
            <CardDescription>state block of the §4.2 WS frame</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-3">
            {connecting || !frame ? (
              <div className="grid grid-cols-2 gap-2">
                {Array.from({ length: 4 }).map((_, i) => (
                  <Skeleton key={i} className="h-16 rounded-lg" />
                ))}
              </div>
            ) : (
              <>
                <div className="grid grid-cols-2 gap-2">
                  <StatTile
                    label="Position NED"
                    value={fmtVec(frame.state.pos_ned_m, 2)}
                    unit="m"
                    icon={<Compass aria-hidden="true" />}
                  />
                  <StatTile
                    label="Velocity NED"
                    value={fmtVec(frame.state.vel_ned_ms, 2)}
                    unit="m/s"
                    icon={<Wind aria-hidden="true" />}
                  />
                  <StatTile label="Roll" value={fmtSigned(att?.roll_deg, 1)} unit="°" />
                  <StatTile label="Pitch" value={fmtSigned(att?.pitch_deg, 1)} unit="°" />
                  <StatTile label="Yaw (ψ)" value={fmt(att?.yaw_deg, 1)} unit="°" sub="from q_wxyz (ZYX)" />
                  <StatTile
                    label="Battery"
                    value={fmt(battery, 1)}
                    unit="%"
                    tone={batTone === 'ok' ? 'ok' : batTone === 'warn' ? 'warn' : 'crit'}
                    icon={<BatteryFull aria-hidden="true" />}
                  />
                </div>

                {/* battery bar */}
                <div className="flex items-center gap-2" aria-label={`Battery ${fmt(battery, 0)} percent`}>
                  <Progress
                    value={clamp(battery ?? 0, 0, 100)}
                    className={`h-2 flex-1 ${
                      batTone === 'crit'
                        ? '[&>div]:bg-rose-500'
                        : batTone === 'warn'
                          ? '[&>div]:bg-amber-500'
                          : '[&>div]:bg-emerald-500'
                    }`}
                  />
                  <span className="font-mono text-xs tabular-nums text-muted-foreground">{fmt(battery, 1)}%</span>
                </div>

                {/* motor outputs */}
                <div>
                  <p className="mb-1.5 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                    Motor outputs (normalized)
                  </p>
                  <div className="flex items-end gap-1.5">
                    {frame.state.motors.map((m, i) => (
                      <div key={i} className="flex min-w-0 flex-1 flex-col items-center gap-1">
                        <div className="relative h-20 w-full overflow-hidden rounded bg-muted">
                          <div
                            className="absolute bottom-0 w-full rounded bg-emerald-500/80 transition-[height] duration-100"
                            style={{ height: `${clamp(m, 0, 1) * 100}%` }}
                          />
                        </div>
                        <span className="font-mono text-[10px] text-muted-foreground">M{i}</span>
                        <span className="font-mono text-[10px] font-semibold tabular-nums">{fmt(m, 2)}</span>
                      </div>
                    ))}
                  </div>
                </div>
              </>
            )}
          </CardContent>
        </Card>
      </div>

      {/* ---------------------------------------------- strip charts + sensors */}
      <div className="grid gap-4 xl:grid-cols-3">
        <Card className="xl:col-span-2">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm">Strip charts — 60 s spooling window</CardTitle>
            <CardDescription>altitude, attitude angles, motor commands · 10 Hz</CardDescription>
          </CardHeader>
          <CardContent className="flex flex-col gap-4">
            {connecting || !frame ? (
              <>
                <Skeleton className="h-36 rounded-lg" />
                <Skeleton className="h-36 rounded-lg" />
                <Skeleton className="h-36 rounded-lg" />
              </>
            ) : (
              <>
                <div>
                  <p className="mb-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">Altitude</p>
                  <StripChart
                    series={[{ label: 'alt', color: EMERALD, data: sim.series.alt }]}
                    unit="m"
                    minV={0}
                    ariaLabel="Altitude strip chart in meters over the last 60 seconds"
                  />
                </div>
                <div>
                  <p className="mb-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                    Attitude (deg)
                  </p>
                  <StripChart
                    series={[
                      { label: 'roll', color: AMBER, data: extract(sim.series.att, 'roll') },
                      { label: 'pitch', color: EMERALD, data: extract(sim.series.att, 'pitch') },
                      { label: 'yaw', color: ZINC, data: extract(sim.series.att, 'yaw') },
                    ]}
                    unit="°"
                    ariaLabel="Roll, pitch and yaw strip chart in degrees over the last 60 seconds"
                  />
                </div>
                <div>
                  <p className="mb-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                    Motor commands
                  </p>
                  <StripChart
                    series={frame.state.motors.map((_, i) => ({
                      label: `M${i}`,
                      color: [EMERALD, AMBER, ORANGE, ZINC][i] ?? ZINC,
                      data: sim.series.motors.map((p) => ({ t: p.t, v: p.v[i] ?? 0 })),
                    }))}
                    ariaLabel="Motor command strip chart, normalized, over the last 60 seconds"
                  />
                </div>
              </>
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader className="pb-2">
            <CardTitle className="text-sm">Sensors — last emitted</CardTitle>
            <CardDescription>noisy values seen by PX4 (SPEC §4.2)</CardDescription>
          </CardHeader>
          <CardContent>
            {connecting || !frame ? (
              <div className="grid grid-cols-2 gap-2">
                {Array.from({ length: 4 }).map((_, i) => (
                  <Skeleton key={i} className="h-16 rounded-lg" />
                ))}
              </div>
            ) : (
              <div className="grid grid-cols-2 gap-2">
                <StatTile
                  label="Accel body"
                  value={fmtVec(frame.sensors.accel_ms2, 2)}
                  unit="m/s²"
                  className="col-span-2"
                />
                <StatTile label="Gyro body" value={fmtVec(frame.sensors.gyro_rads, 3)} unit="rad/s" className="col-span-2" />
                <StatTile label="Baro altitude" value={fmt(frame.sensors.baro_alt_m, 1)} unit="m" icon={<Gauge aria-hidden="true" />} />
                <StatTile
                  label="GPS"
                  value={gpsFixLabel(frame.sensors.gps_fix)}
                  sub={`${frame.sensors.gps_sat} satellites`}
                  tone={frame.sensors.gps_fix >= 3 ? 'ok' : frame.sensors.gps_fix >= 2 ? 'warn' : 'crit'}
                  icon={<Satellite aria-hidden="true" />}
                />
              </div>
            )}
          </CardContent>
        </Card>
      </div>

      {/* ------------------------------------------------------ fault console */}
      <Card>
        <CardHeader className="pb-2">
          <CardTitle className="text-sm">Fault console</CardTitle>
          <CardDescription>runtime injection — same schema as timeline events (§7.3)</CardDescription>
        </CardHeader>
        <CardContent>
          <FaultConsole
            faults={sim.faults}
            conn={sim.conn}
            onInject={(p) => sim.injectFault(p)}
            onClear={(id) => sim.clearFault(id)}
          />
        </CardContent>
      </Card>
    </div>
  )
}

function PhaseBadge({ phase }: { phase: SimPhase }) {
  const p = phase.toUpperCase()
  const cls =
    p === 'RUN'
      ? 'border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400'
      : p === 'WAIT'
        ? 'border-amber-600/40 bg-amber-500/10 text-amber-700 dark:text-amber-400'
        : p === 'DONE'
          ? 'border-border bg-muted text-muted-foreground'
          : 'border-rose-600/40 bg-rose-500/10 text-rose-700 dark:text-rose-400'
  return (
    <Badge variant="outline" className={`font-mono ${cls}`} aria-label={`Phase ${p}`}>
      {p}
    </Badge>
  )
}

function MapEmptyState() {
  return (
    <div className="flex h-64 flex-col items-center justify-center gap-1.5 rounded-lg border border-dashed border-border text-center sm:h-72">
      <Activity className="size-5 text-muted-foreground" aria-hidden="true" />
      <p className="text-sm font-medium text-muted-foreground">Waiting for telemetry frames</p>
      <p className="text-xs text-muted-foreground/80">WS /ws/telemetry silent — status is being polled via REST</p>
    </div>
  )
}

function gpsFixLabel(fix: number): string {
  if (fix >= 3) return '3D'
  if (fix === 2) return '2D'
  if (fix === 1) return 'NO-FIX'
  return 'DENIED'
}

function extract(
  att: { t: number; roll: number; pitch: number; yaw: number }[],
  key: 'roll' | 'pitch' | 'yaw',
): { t: number; v: number }[] {
  return att.map((a) => ({ t: a.t, v: a[key] }))
}
