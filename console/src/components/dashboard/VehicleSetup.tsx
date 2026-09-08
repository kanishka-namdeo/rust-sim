'use client'

/**
 * Vehicle Setup (ADR-0016) — the QGroundControl / Mission-Planner-style
 * configuration workflow for this fleet: Summary, Airframe, Sensors,
 * Power, Safety, Flight Modes, Parameters.
 *
 * Everything param-derived is the live vehicle's own configuration read
 * back over the param protocol (GET /api/vehicles/{i}/setup), and every
 * write goes through the endpoints (PARAM_SET echo-confirmed / COMMAND_LONG
 * ACKed) — this view never invents state.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  Activity,
  BatteryCharging,
  Compass,
  Filter,
  Gauge,
  ListTree,
  Plane,
  RotateCw,
  Save,
  Search,
  ShieldAlert,
  Trash2,
  Upload,
  Wrench,
} from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import { Progress } from '@/components/ui/progress'
import { ScrollArea } from '@/components/ui/scroll-area'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
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
import { ConnBadge, ConnSubline } from './ConnBadge'
import { StatTile } from './StatTile'
import { fmt } from '@/lib/format'
import type { VehicleSetupApi } from '@/hooks/useVehicleSetup'
import type { CalSensor, ParamEntry, ParamStoreView, PresetSummary } from '@/lib/types'

type Section = 'summary' | 'airframe' | 'sensors' | 'power' | 'safety' | 'modes' | 'params'

export function VehicleSetup({ setup }: { setup: VehicleSetupApi }) {
  const { toast } = useToast()
  const [section, setSection] = useState<Section>('summary')
  const s = setup.summary
  const armed = s?.armed === true

  const runAction = useCallback(
    (label: string, fn: () => Promise<{ ok: boolean; detail?: string }>) => {
      void fn().then((r) => {
        toast({
          title: r.ok ? `${label} — accepted` : `${label} — rejected`,
          description: r.detail ?? (r.ok ? 'ok' : 'no detail'),
          ...(r.ok ? {} : { variant: 'destructive' as const }),
        })
      })
    },
    [toast],
  )

  return (
    <div className="flex flex-col gap-4">
      {/* -------------------------------------------------- setup header */}
      <Card>
        <CardHeader className="pb-3">
          <div className="flex flex-wrap items-start justify-between gap-3">
            <div className="min-w-0">
              <CardTitle className="flex flex-wrap items-center gap-2.5 text-base">
                <Wrench className="size-4 text-emerald-600 dark:text-emerald-400" aria-hidden="true" />
                Vehicle Setup
                {s && (
                  <Badge variant="outline" className={`font-mono ${s.armed ? 'border-rose-600/40 bg-rose-500/10 text-rose-700 dark:text-rose-400' : 'border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400'}`}>
                    {s.armed ? 'ARMED' : 'DISARMED'}
                  </Badge>
                )}
                {s?.restart_pending && (
                  <Badge variant="outline" className="animate-pulse font-mono border-amber-600/40 bg-amber-500/10 text-amber-700 dark:text-amber-400">
                    <RotateCw className="size-3" aria-hidden="true" /> RESTARTING
                  </Badge>
                )}
                <ConnBadge conn={setup.conn} port={setup.port} retryAt={setup.retryAt} lastError={setup.lastError} />
              </CardTitle>
              <div className="mt-1 text-xs text-muted-foreground">
                <ConnSubline conn={setup.conn} port={setup.port} retryAt={setup.retryAt} lastError={setup.lastError} />
              </div>
            </div>
            <div className="flex items-center gap-1.5">
              {Array.from({ length: setup.vehicleCount }, (_, i) => (
                <Button
                  key={i}
                  size="sm"
                  variant={setup.index === i ? 'default' : 'outline'}
                  className="font-mono"
                  disabled={setup.busy}
                  onClick={() => setup.setIndex(i)}
                >
                  V{i + 1}
                </Button>
              ))}
            </div>
          </div>
        </CardHeader>
      </Card>

      {/* ------------------------------------------------------- the rail */}
      <div className="grid min-w-0 gap-4 lg:grid-cols-[13rem_1fr]">
        <Card className="h-fit">
          <CardHeader className="pb-2">
            <CardTitle className="text-sm">Setup sections</CardTitle>
            <CardDescription>the QGC rail</CardDescription>
          </CardHeader>
          <CardContent className="p-2">
            <Tabs value={section} onValueChange={(v) => setSection(v as Section)} orientation="vertical">
              <TabsList className="grid h-auto w-full grid-cols-1 gap-1 bg-transparent p-0">
                {(
                  [
                    ['summary', 'Summary', <Gauge key="i" className="size-3.5" aria-hidden="true" />],
                    ['airframe', 'Airframe', <Plane key="i" className="size-3.5" aria-hidden="true" />],
                    ['sensors', 'Sensors', <Compass key="i" className="size-3.5" aria-hidden="true" />],
                    ['power', 'Power', <BatteryCharging key="i" className="size-3.5" aria-hidden="true" />],
                    ['safety', 'Safety', <ShieldAlert key="i" className="size-3.5" aria-hidden="true" />],
                    ['modes', 'Flight Modes', <ListTree key="i" className="size-3.5" aria-hidden="true" />],
                    ['params', 'Parameters', <Activity key="i" className="size-3.5" aria-hidden="true" />],
                  ] as [Section, string, React.ReactNode][]
                ).map(([v, label, icon]) => (
                  <TabsTrigger
                    key={v}
                    value={v}
                    className="flex w-full justify-start gap-2 px-3 py-2 data-[state=active]:bg-muted"
                  >
                    {icon}
                    <span className="text-xs">{label}</span>
                  </TabsTrigger>
                ))}
              </TabsList>
            </Tabs>
          </CardContent>
        </Card>

        <Card className="min-w-0">
          <Tabs value={section}>
            <TabsContent value="summary" className="mt-0">
              <SummarySection setup={setup} />
            </TabsContent>
            <TabsContent value="airframe" className="mt-0">
              <AirframeSection setup={setup} onApply={(id) => runAction('Airframe apply', () => setup.applyAirframe(id))} />
            </TabsContent>
            <TabsContent value="sensors" className="mt-0">
              <SensorsSection setup={setup} onCalibrate={(sensor) => runAction(`Calibrate ${sensor}`, () => setup.calibrate(sensor))} />
            </TabsContent>
            <TabsContent value="power" className="mt-0">
              <PowerSafetySection
                title="Power (battery)"
                description="BAT_* parameters — cells, thresholds and pack resistance, written PARAM_SET echo-confirmed"
                icon={<BatteryCharging className="size-4" aria-hidden="true" />}
                params={s?.power ?? null}
                armed={armed}
                busy={setup.busy}
                onWrite={(id, v) => runAction(`Write ${id}`, () => setup.writeParam(id, v))}
              />
            </TabsContent>
            <TabsContent value="safety" className="mt-0">
              <PowerSafetySection
                title="Safety / failsafe"
                description="RC-loss, datalink-loss, low-battery and geofence actions — the policy ladder's vehicle-side twin"
                icon={<ShieldAlert className="size-4" aria-hidden="true" />}
                params={s?.safety ?? null}
                armed={armed}
                busy={setup.busy}
                onWrite={(id, v) => runAction(`Write ${id}`, () => setup.writeParam(id, v))}
              />
            </TabsContent>
            <TabsContent value="modes" className="mt-0">
              <ModesSection setup={setup} onSet={(m) => runAction(`Mode ${m}`, () => setup.setMode(m))} />
            </TabsContent>
            <TabsContent value="params" className="mt-0" forceMount>
              <div className="data-[state=inactive]:hidden">
                <ParamsSection setup={setup} onRefresh={() => runAction('Parameter download', () => setup.refreshParams())} onWrite={(id, v) => runAction(`Write ${id}`, () => setup.writeParam(id, v))} />
              </div>
            </TabsContent>
          </Tabs>
        </Card>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

function SummarySection({ setup }: { setup: VehicleSetupApi }) {
  const s = setup.summary
  const cal = s?.calibration
  const p = s?.params
  const calChips: { label: string; ok: boolean | null }[] = cal
    ? [
        { label: 'Accelerometer', ok: cal.accel },
        { label: 'Gyroscope', ok: cal.gyro },
        { label: 'Magnetometer 0', ok: cal.mag0 },
        { label: 'Level horizon', ok: cal.level_horizon },
      ]
    : []

  return (
    <div className="p-4">
      <CardTitle className="text-sm">Vehicle summary</CardTitle>
      <CardDescription className="mb-3 mt-1">
        identity, airframe, calibration and download state — everything read from the live param cache
      </CardDescription>
      <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-4">
        <StatTile label="Airframe" value={s?.airframe.name ?? '—'} sub={s?.airframe.category || undefined} tone="accent" />
        <StatTile label="SYS_AUTOSTART" value={s?.airframe.sys_autostart ?? '—'} sub={s?.airframe.frame_type || undefined} />
        <StatTile label="Flight mode" value={s?.mode ?? '—'} sub={`FSM ${s?.fsm ?? '—'}`} />
        <StatTile label="Battery" value={`${fmt(s?.battery_pct, 0)}%`} sub={s?.voltage_v != null ? `${fmt(s.voltage_v, 2)} V` : undefined} />
        <StatTile label="Autopilot" value={s?.autopilot.type ?? '—'} sub={s?.autopilot.version || undefined} />
        <StatTile
          label="Parameters"
          value={p ? `${p.received}/${p.total}` : '—'}
          sub={p ? String(p.state) : 'no link yet'}
          tone={p?.state === 'Complete' ? 'ok' : p?.state === 'Stalled' ? 'warn' : 'default'}
        />
        <StatTile
          label="Dynamics"
          value={s ? (s.airframe.dynamics_compatible ? 'compatible' : 'config-only') : '—'}
          sub={s?.airframe.dynamics_compatible ? 'quad HIL can fly it' : 'quad dynamics mismatch (ADR-0016)'}
          tone={s?.airframe.dynamics_compatible ? 'ok' : 'warn'}
        />
        <StatTile label="sysid/compid" value={s ? `${s.sysid}/${s.compid}` : '—'} sub={`V${(s?.index ?? 0) + 1}`} />
      </div>

      {p && p.state !== 'Complete' && (
        <div className="mt-4">
          <div className="mb-1 flex items-center justify-between text-xs text-muted-foreground">
            <span>parameter download</span>
            <span className="font-mono">
              {p.received}/{p.total} · {p.state}
            </span>
          </div>
          <Progress value={p.total > 0 ? (p.received / p.total) * 100 : 0} aria-label="parameter download progress" />
        </div>
      )}

      <div className="mt-4">
        <p className="mb-2 text-xs font-medium text-muted-foreground">Sensor calibration (CAL_*_ID rule)</p>
        <div className="flex flex-wrap gap-1.5">
          {calChips.length === 0 && <span className="text-xs text-muted-foreground">no param download yet — use Parameters → Download</span>}
          {calChips.map((c) => (
            <Badge
              key={c.label}
              variant="outline"
              className={
                c.ok === true
                  ? 'border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400'
                  : c.ok === false
                    ? 'border-rose-600/40 bg-rose-500/10 text-rose-700 dark:text-rose-400'
                    : ''
              }
            >
              {c.label}: {c.ok === true ? 'calibrated' : c.ok === false ? 'not calibrated' : 'unknown'}
            </Badge>
          ))}
        </div>
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Airframe
// ---------------------------------------------------------------------------

function AirframeSection({
  setup,
  onApply,
}: {
  setup: VehicleSetupApi
  onApply: (id: number) => void
}) {
  const s = setup.summary
  const current = s?.airframe.sys_autostart ?? null
  const [filter, setFilter] = useState('')
  const f = filter.trim().toLowerCase()

  const groups = useMemo(
    () =>
      setup.groups
        .map((g) => ({
          ...g,
          airframes: g.airframes.filter(
            (a) => !f || a.name.toLowerCase().includes(f) || String(a.id).includes(f) || (a.sim_model ?? '').toLowerCase().includes(f),
          ),
        }))
        .filter((g) => g.airframes.length > 0),
    [setup.groups, f],
  )

  return (
    <div className="p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <CardTitle className="text-sm">Airframe (SYS_AUTOSTART)</CardTitle>
          <CardDescription className="mt-1 max-w-xl">
            ROMFS-derived catalog ({setup.groups.reduce((n, g) => n + g.airframes.length, 0)} frames). Apply writes
            SYS_AUTOSTART (echo-confirmed, PX4 autosaves) and restarts the sim+px4 pair — the QGC &quot;Apply and
            Restart&quot; flow. Requires disarmed.
          </CardDescription>
        </div>
        <div className="relative">
          <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
          <Input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="filter frames…"
            className="h-8 w-48 pl-8 text-xs"
            aria-label="filter airframes"
          />
        </div>
      </div>

      {s && (
        <div className="mt-3 rounded-lg border border-border bg-muted/30 p-3">
          <p className="text-xs text-muted-foreground">current</p>
          <p className="mt-0.5 text-sm font-medium">
            {s.airframe.name} <span className="ml-2 font-mono text-xs text-muted-foreground">#{s.airframe.sys_autostart}</span>
            <span className="ml-2 text-xs text-muted-foreground">{s.airframe.category}</span>
            {!s.airframe.dynamics_compatible && (
              <span className="ml-2 rounded bg-amber-500/10 px-1.5 py-0.5 text-[10px] text-amber-700 dark:text-amber-400">
                config-only vs quad HIL
              </span>
            )}
          </p>
        </div>
      )}

      <ScrollArea className="mt-3 h-[26rem] pr-3">
        <div className="flex flex-col gap-4">
          {groups.map((g) => (
            <div key={g.category}>
              <p className="mb-1.5 text-xs font-semibold tracking-wide text-muted-foreground">{g.category}</p>
              <div className="grid gap-1.5 sm:grid-cols-2">
                {g.airframes.map((a) => (
                  <div
                    key={a.id}
                    className={`flex items-center justify-between gap-2 rounded-lg border px-3 py-2 ${
                      current === a.id ? 'border-emerald-600/50 bg-emerald-600/5' : 'border-border'
                    }`}
                  >
                    <div className="min-w-0">
                      <p className="truncate text-xs font-medium">{a.name}</p>
                      <p className="truncate font-mono text-[10px] text-muted-foreground">
                        #{a.id}
                        {a.frame_type ? ` · ${a.frame_type}` : ''}
                        {a.sim_model ? ` · ${a.sim_model}` : ''}
                      </p>
                    </div>
                    <AlertDialog>
                      <AlertDialogTrigger asChild>
                        <Button
                          size="sm"
                          variant={current === a.id ? 'outline' : 'secondary'}
                          className="h-7 shrink-0 gap-1.5 px-2.5 text-[11px]"
                          disabled={setup.busy || s?.armed === true}
                        >
                          <RotateCw className="size-3" aria-hidden="true" />
                          {current === a.id ? 'Re-apply' : 'Apply'}
                        </Button>
                      </AlertDialogTrigger>
                      <AlertDialogContent>
                        <AlertDialogHeader>
                          <AlertDialogTitle>Apply airframe {a.name} (#{a.id})?</AlertDialogTitle>
                          <AlertDialogDescription>
                            Writes SYS_AUTOSTART={a.id} to vehicle V{(setup.index ?? 0) + 1} (PARAM_SET, echo-confirmed)
                            and restarts its sim+px4 pair. PX4 loads the new airframe&apos;s defaults at boot. The vehicle
                            must be disarmed{a.sim_model == null ? '' : ' and re-downloads its parameters afterwards'}.
                          </AlertDialogDescription>
                        </AlertDialogHeader>
                        <AlertDialogFooter>
                          <AlertDialogCancel>Cancel</AlertDialogCancel>
                          <AlertDialogAction onClick={(e) => { e.preventDefault(); onApply(a.id) }}>
                            Apply and restart
                          </AlertDialogAction>
                        </AlertDialogFooter>
                      </AlertDialogContent>
                    </AlertDialog>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </ScrollArea>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Sensors
// ---------------------------------------------------------------------------

const CAL_ROWS: { sensor: CalSensor; label: string; help: string }[] = [
  { sensor: 'gyro', label: 'Gyroscope', help: 'MAV_CMD 241 param1=1 — CAL_GYRO0_ID flips nonzero' },
  { sensor: 'accel', label: 'Accelerometer', help: 'MAV_CMD 241 param5=1 — 6-position calibration' },
  { sensor: 'accel_quick', label: 'Accelerometer (quick)', help: 'MAV_CMD 241 param5=4 — single-position' },
  { sensor: 'mag', label: 'Magnetometer', help: 'MAV_CMD 241 param2=1 — CAL_MAG0_ID flips nonzero' },
  { sensor: 'level', label: 'Level horizon', help: 'MAV_CMD 241 param5=2 — board offset / level' },
  { sensor: 'baro', label: 'Barometer', help: 'MAV_CMD 241 param3=1 — ground pressure' },
  { sensor: 'airspeed', label: 'Airspeed', help: 'MAV_CMD 241 param6=2 — pitot zero (planes only)' },
]

function SensorsSection({
  setup,
  onCalibrate,
}: {
  setup: VehicleSetupApi
  onCalibrate: (sensor: CalSensor) => void
}) {
  const s = setup.summary
  const cal = s?.calibration
  const flag = (b: boolean | null) =>
    b === true ? 'calibrated' : b === false ? 'not calibrated' : 'unknown (no download)'
  const flagCls = (b: boolean | null) =>
    b === true
      ? 'border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400'
      : b === false
        ? 'border-rose-600/40 bg-rose-500/10 text-rose-700 dark:text-rose-400'
        : ''

  return (
    <div className="p-4">
      <CardTitle className="text-sm">Sensors &amp; calibration</CardTitle>
      <CardDescription className="mb-3 mt-1 max-w-xl">
        the QGC calibration buttons — MAV_CMD_PREFLIGHT_CALIBRATION (241) with PX4 v1.16 commander&apos;s own param
        matrix. Requires disarmed and an idle vehicle; the CAL_*_ID confirmation lands in the next parameter download.
      </CardDescription>

      <div className="mb-3 flex flex-wrap gap-1.5">
        <Badge variant="outline" className={flagCls(cal?.accel ?? null)}>Accelerometer: {flag(cal?.accel ?? null)}</Badge>
        <Badge variant="outline" className={flagCls(cal?.gyro ?? null)}>Gyroscope: {flag(cal?.gyro ?? null)}</Badge>
        <Badge variant="outline" className={flagCls(cal?.mag0 ?? null)}>Mag 0: {flag(cal?.mag0 ?? null)}</Badge>
        <Badge variant="outline" className={flagCls(cal?.level_horizon ?? null)}>Level: {flag(cal?.level_horizon ?? null)}</Badge>
      </div>

      <div className="flex flex-col gap-1.5">
        {CAL_ROWS.map((r) => (
          <div key={r.sensor} className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border px-3 py-2">
            <div className="min-w-0">
              <p className="text-xs font-medium">{r.label}</p>
              <p className="truncate text-[10px] text-muted-foreground">{r.help}</p>
            </div>
            <Button
              size="sm"
              variant="secondary"
              className="h-7 gap-1.5 px-2.5 text-[11px]"
              disabled={setup.busy || s?.armed === true}
              onClick={() => onCalibrate(r.sensor)}
            >
              <Compass className="size-3" aria-hidden="true" />
              Calibrate
            </Button>
          </div>
        ))}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Power / Safety (param editor sections)
// ---------------------------------------------------------------------------

const POWER_LABELS: Record<string, string> = {
  BAT1_N_CELLS: 'Battery cells',
  BAT1_V_EMPTY: 'Empty cell voltage (V)',
  BAT1_V_CHARGED: 'Full cell voltage (V)',
  BAT_LOW_THR: 'Low-battery threshold',
  BAT_CRIT_THR: 'Critical threshold',
  BAT_EMERGEN_THR: 'Emergency threshold',
  BAT1_R_INTERNAL: 'Internal resistance (Ω)',
}

const SAFETY_LABELS: Record<string, string> = {
  NAV_RCL_ACT: 'RC loss action',
  NAV_DLL_ACT: 'Datalink loss action',
  COM_LOW_BAT_ACT: 'Low-battery action',
  COM_OBL_RC_ACT: 'RC override action',
  GF_ACTION: 'Geofence action',
  GF_MAX_HOR_DIST: 'Geofence max horizontal (m)',
  GF_MAX_VER_DIST: 'Geofence max vertical (m)',
  RTL_RETURN_ALT: 'RTL return altitude (m)',
}

const SAFETY_HINTS: Record<string, string> = {
  NAV_RCL_ACT: '0=none 1=hold 2=rtl 3=land 5=terminate 6=lockdown',
  NAV_DLL_ACT: 'the GCS-connection arming gate (F-2): 0 may block arming',
  COM_LOW_BAT_ACT: '0=warning 1=return 2=land 3=hold',
  GF_ACTION: '0=none 1=warning 2=hold 3=return 4=terminate',
}

function PowerSafetySection({
  title,
  description,
  icon,
  params,
  armed,
  busy,
  onWrite,
}: {
  title: string
  description: string
  icon: React.ReactNode
  params: Record<string, number | null> | null
  armed: boolean
  busy: boolean
  onWrite: (id: string, v: number) => void
}) {
  const labels = title.startsWith('Power') ? POWER_LABELS : SAFETY_LABELS
  const hints = title.startsWith('Power') ? {} : SAFETY_HINTS
  const [drafts, setDrafts] = useState<Record<string, string>>({})

  return (
    <div className="p-4">
      <CardTitle className="flex items-center gap-2 text-sm">
        {icon}
        {title}
      </CardTitle>
      <CardDescription className="mb-3 mt-1 max-w-xl">{description}</CardDescription>

      {params == null && <p className="text-xs text-muted-foreground">no parameter download yet — use Parameters → Download</p>}

      {params != null && (
        <div className="flex flex-col gap-1.5">
          {Object.keys(labels)
            .filter((id) => id in params)
            .map((id) => {
              const value = params[id]
              const draft = drafts[id] ?? (value != null ? String(value) : '')
              const dirty = value != null && draft !== String(value)
              return (
                <div key={id} className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border px-3 py-2">
                  <div className="min-w-0 flex-1">
                    <Label htmlFor={`p-${id}`} className="text-xs font-medium">
                      {labels[id] ?? id}
                    </Label>
                    <p className="truncate font-mono text-[10px] text-muted-foreground">
                      {id}
                      {hints[id] ? ` — ${hints[id]}` : ''}
                    </p>
                  </div>
                  <div className="flex items-center gap-1.5">
                    <Input
                      id={`p-${id}`}
                      inputMode="decimal"
                      className="h-8 w-28 font-mono text-xs"
                      value={draft}
                      disabled={value == null || busy || armed}
                      onChange={(e) => setDrafts((d) => ({ ...d, [id]: e.target.value }))}
                    />
                    <Button
                      size="sm"
                      variant={dirty ? 'default' : 'outline'}
                      className="h-8 px-2.5 text-[11px]"
                      disabled={!dirty || value == null || busy || armed}
                      onClick={() => {
                        const v = Number(draft)
                        if (Number.isFinite(v)) onWrite(id, v)
                      }}
                    >
                      Write
                    </Button>
                  </div>
                </div>
              )
            })}
        </div>
      )}
    </div>
  )
}

// ---------------------------------------------------------------------------
// Flight modes
// ---------------------------------------------------------------------------

function ModesSection({
  setup,
  onSet,
}: {
  setup: VehicleSetupApi
  onSet: (mode: string) => void
}) {
  const s = setup.summary
  return (
    <div className="p-4">
      <CardTitle className="text-sm">Flight modes</CardTitle>
      <CardDescription className="mb-3 mt-1 max-w-xl">
        DO_SET_MODE with fleet-modes-verified mode words — the same switch the supervisor itself uses. The mode echo
        confirms on the next heartbeat.
      </CardDescription>

      <div className="mb-3 flex items-center gap-2 text-sm">
        <span className="text-xs text-muted-foreground">current:</span>
        <Badge variant="outline" className="font-mono border-emerald-600/40 bg-emerald-600/10 text-emerald-700 dark:text-emerald-400">
          {s?.mode ?? '—'}
        </Badge>
        {s?.mode_word != null && <span className="font-mono text-[10px] text-muted-foreground">0x{(s.mode_word >>> 0).toString(16).toUpperCase().padStart(8, '0')}</span>}
      </div>

      <div className="grid grid-cols-2 gap-1.5 sm:grid-cols-3 lg:grid-cols-4">
        {setup.modes.map((m) => (
          <Button
            key={m.name}
            size="sm"
            variant={s?.mode === m.name ? 'default' : 'outline'}
            className="h-9 font-mono text-[11px]"
            disabled={setup.busy}
            onClick={() => onSet(m.name)}
          >
            {m.name}
          </Button>
        ))}
      </div>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

function ParamsSection({
  setup,
  onRefresh,
  onWrite,
}: {
  setup: VehicleSetupApi
  onRefresh: () => void
  onWrite: (id: string, v: number) => void
}) {
  const store = setup.paramStore
  const { toast } = useToast()

  // Pull the v1 hook methods out so the effects below see stable identities
  // (each is a useCallback on the hook side; setup itself is recreated on
  // every render, but the method refs are stable across renders).
  const { searchParams, listPresets, savePreset, loadPreset, deletePreset } = setup

  // -- search/filter state ----------------------------------------------------
  // `query` is the raw input; `debouncedQuery` is the trimmed string used
  // to drive the server-side search after a 250 ms debounce (AC-5.3.1).
  const [query, setQuery] = useState('')
  const [debouncedQuery, setDebouncedQuery] = useState('')
  const [group, setGroup] = useState<string>('__all__')
  const [diffOnly, setDiffOnly] = useState(false)
  const [searchStore, setSearchStore] = useState<ParamStoreView | null>(null)
  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // 250 ms debounce: keystrokes update `query` immediately (input stays
  // responsive); after the operator pauses, we commit to `debouncedQuery`,
  // which the search effect keys off of.
  useEffect(() => {
    if (debounceRef.current) clearTimeout(debounceRef.current)
    debounceRef.current = setTimeout(() => {
      setDebouncedQuery(query.trim())
    }, 250)
    return () => {
      if (debounceRef.current) clearTimeout(debounceRef.current)
    }
  }, [query])

  // fire searchParams whenever the debounced query or group changes.
  // when both are empty we drop back to the polled store (no extra fetch) —
  // `sourceStore` below derives this without a synchronous setState, which
  // keeps the react-hooks/set-state-in-effect rule quiet.
  //
  // We use a monotonic id so a slow in-flight request can't overwrite a
  // newer one (the operator types "MPC" → "MC"; the MPC response may land
  // last; we ignore it).
  const searchIdRef = useRef(0)
  useEffect(() => {
    const hasFilter = debouncedQuery !== '' || (group !== '__all__' && group !== '')
    if (!hasFilter) return
    const id = ++searchIdRef.current
    void searchParams(debouncedQuery, group).then((r) => {
      if (id !== searchIdRef.current) return
      setSearchStore(r.store)
    })
    return () => {
      // bump on cleanup so any in-flight result of this run is ignored
      searchIdRef.current++
    }
  }, [searchParams, debouncedQuery, group])

  const hasFilter = debouncedQuery !== '' || (group !== '__all__' && group !== '')
  // "searching" = a filter is active and the latest result hasn't returned
  // yet (we treat a null store as "not yet populated"; when the filter is
  // cleared we fall through to the polled store).
  const searching = hasFilter && searchStore == null

  // unique param groups (from the polled store; stable so useMemo).
  const groupOptions = useMemo(() => {
    const set = new Set<string>()
    for (const p of store?.params ?? []) {
      if (p.group) set.add(p.group)
    }
    return Array.from(set).sort()
  }, [store])

  // the displayed rows come from `searchStore` when a filter is active,
  // else from the polled `store` (the standard param-cache view).
  const sourceStore: ParamStoreView | null =
    debouncedQuery !== '' || (group !== '__all__' && group !== '') ? searchStore : store

  const rows = useMemo(() => {
    let list: ParamEntry[] = sourceStore?.params ?? []
    if (diffOnly) {
      list = list.filter((p) => p.is_changed)
    }
    return list.slice(0, 400)
  }, [sourceStore, diffOnly])

  // count of changed params in the full polled store (for the diff
  // toggle's badge — "Diff (3)")
  const changedCount = useMemo(
    () => (store?.params ?? []).filter((p) => p.is_changed).length,
    [store],
  )

  // -- drafts (per-row edit buffer) ------------------------------------------
  const [drafts, setDrafts] = useState<Record<string, string>>({})

  const pct = store && store.total > 0 ? Math.min(100, (store.received / store.total) * 100) : 0

  // -- preset modal state -----------------------------------------------------
  const [savePresetOpen, setSavePresetOpen] = useState(false)
  const [presetName, setPresetName] = useState('')
  const [savingPreset, setSavingPreset] = useState(false)

  const [loadPresetOpen, setLoadPresetOpen] = useState(false)
  const [presets, setPresets] = useState<PresetSummary[]>([])
  const [loadingPresets, setLoadingPresets] = useState(false)
  const [loadingPresetName, setLoadingPresetName] = useState<string | null>(null)
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null)

  const refreshPresets = useCallback(async () => {
    setLoadingPresets(true)
    try {
      const r = await listPresets()
      setPresets(r.presets)
      if (!r.ok) {
        toast({ title: 'Load presets — failed', description: r.detail ?? 'no detail', variant: 'destructive' })
      }
    } finally {
      setLoadingPresets(false)
    }
  }, [listPresets, toast])

  const openLoadPreset = useCallback(() => {
    setLoadPresetOpen(true)
    void refreshPresets()
  }, [refreshPresets])

  const doSavePreset = useCallback(async () => {
    const name = presetName.trim()
    if (!name) return
    setSavingPreset(true)
    try {
      const r = await savePreset(name)
      toast({
        title: r.ok ? 'Preset saved' : 'Save preset — rejected',
        description: r.detail ?? (r.ok ? 'ok' : 'no detail'),
        ...(r.ok ? {} : { variant: 'destructive' as const }),
      })
      if (r.ok) {
        setSavePresetOpen(false)
        setPresetName('')
        // refresh the presets panel so the new row appears below the table.
        void refreshPresets()
      }
    } finally {
      setSavingPreset(false)
    }
  }, [presetName, savePreset, toast, refreshPresets])

  const doLoadPreset = useCallback(
    async (name: string) => {
      setLoadingPresetName(name)
      try {
        const r = await loadPreset(name)
        toast({
          title: r.ok ? `Preset '${name}' loaded` : `Load preset — rejected`,
          description: r.detail ?? (r.ok ? 'ok' : 'no detail'),
          ...(r.ok ? {} : { variant: 'destructive' as const }),
        })
        if (r.ok) {
          setLoadPresetOpen(false)
        }
      } finally {
        setLoadingPresetName(null)
      }
    },
    [loadPreset, toast],
  )

  const doDeletePreset = useCallback(
    async (name: string) => {
      const r = await deletePreset(name)
      toast({
        title: r.ok ? `Preset '${name}' deleted` : `Delete preset — rejected`,
        description: r.detail ?? (r.ok ? 'ok' : 'no detail'),
        ...(r.ok ? {} : { variant: 'destructive' as const }),
      })
      if (r.ok) {
        setConfirmDelete(null)
        void refreshPresets()
      }
    },
    [deletePreset, toast, refreshPresets],
  )

  return (
    <div className="p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <CardTitle className="text-sm">Parameters ({store?.params.length ?? 0} cached)</CardTitle>
          <CardDescription className="mt-1 max-w-xl">
            the live PARAM_VALUE cache — every row was echoed by the vehicle itself. Writes are PARAM_SET with
            echo-confirmation; PX4 autosaves so values survive reboots. Changed-from-default rows are
            highlighted in pale yellow (AC-5.3.4).
          </CardDescription>
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          {/* ---- search input (250 ms debounce → /api/vehicles/{i}/params?search=) ---- */}
          <div className="relative">
            <Search className="absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
            <Input
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="search params by id…"
              className="h-8 w-56 pl-8 font-mono text-xs"
              aria-label="search parameters by id substring"
            />
            {searching && (
              <span className="absolute right-2 top-1/2 size-3 -translate-y-1/2 animate-pulse rounded-full bg-amber-400" aria-hidden="true" />
            )}
          </div>
          {/* ---- group dropdown (populated from unique groups) ---- */}
          <Select value={group} onValueChange={setGroup}>
            <SelectTrigger size="sm" className="h-8 w-32 gap-1.5 font-mono text-[11px]" aria-label="filter by param group">
              <Filter className="size-3 text-muted-foreground" aria-hidden="true" />
              <SelectValue placeholder="group" />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="__all__">all groups</SelectItem>
              {groupOptions.map((g) => (
                <SelectItem key={g} value={g}>
                  {g}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
          {/* ---- Diff against defaults toggle ---- */}
          <Button
            size="sm"
            variant={diffOnly ? 'default' : 'outline'}
            className="h-8 gap-1.5 text-[11px]"
            onClick={() => setDiffOnly((v) => !v)}
            title="Show only parameters whose value differs from PX4's compiled-in default (AC-5.3.4)"
            aria-pressed={diffOnly}
          >
            <Activity className="size-3" aria-hidden="true" />
            Diff against defaults
            {changedCount > 0 && (
              <Badge variant={diffOnly ? 'secondary' : 'outline'} className="ml-0.5 h-4 px-1 text-[9px]">{changedCount}</Badge>
            )}
          </Button>
          {/* ---- Save as preset (modal trigger) ---- */}
          <Button
            size="sm"
            variant="secondary"
            className="h-8 gap-1.5 text-[11px]"
            disabled={setup.busy || (store?.params.length ?? 0) === 0}
            onClick={() => {
              setPresetName('')
              setSavePresetOpen(true)
            }}
            title="Save the current parameter set as a named preset (POST /api/vehicles/{i}/param-presets)"
          >
            <Save className="size-3" aria-hidden="true" />
            Save as preset
          </Button>
          {/* ---- Load preset (modal trigger) ---- */}
          <Button
            size="sm"
            variant="secondary"
            className="h-8 gap-1.5 text-[11px]"
            onClick={openLoadPreset}
            title="Load a saved preset and apply each param via PARAM_SET (POST /api/vehicles/{i}/param-presets/{name}/load)"
          >
            <Upload className="size-3" aria-hidden="true" />
            Load preset
          </Button>
          {/* ---- Download (PARAM_REQUEST_LIST) ---- */}
          <Button size="sm" variant="secondary" className="h-8 gap-1.5 text-[11px]" disabled={setup.busy} onClick={onRefresh}>
            <RotateCw className="size-3" aria-hidden="true" />
            Download
          </Button>
        </div>
      </div>

      {store && (
        <div className="mt-3">
          <div className="mb-1 flex items-center justify-between text-xs text-muted-foreground">
            <span>
              download {store.state} · {store.received}/{store.total}
            </span>
            <span className="font-mono text-[10px]">
              {searching ? 'searching…' : `${rows.length} shown`}
              {diffOnly && rows.length > 0 && ' · diff only'}
            </span>
          </div>
          <Progress value={pct} aria-label="parameter download progress" />
        </div>
      )}

      <ScrollArea className="mt-3 h-[24rem] pr-3">
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="h-8 text-[11px]">id</TableHead>
              <TableHead className="h-8 w-16 text-[11px]">group</TableHead>
              <TableHead className="h-8 w-32 text-[11px]">value</TableHead>
              <TableHead className="h-8 w-24 text-[11px]">default</TableHead>
              <TableHead className="h-8 w-16 text-[11px]">type</TableHead>
              <TableHead className="h-8 w-24 text-[11px]"></TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.map((p) => {
              const draft = drafts[p.id] ?? String(p.value)
              const dirty = draft !== String(p.value)
              const changed = p.is_changed
              return (
                <TableRow
                  key={p.id}
                  className={changed ? 'bg-amber-50 dark:bg-amber-950/30' : ''}
                >
                  <TableCell className="py-1 font-mono text-[11px]">
                    {p.id}
                    {changed && (
                      <span
                        className="ml-1.5 inline-block size-1.5 rounded-full bg-amber-500 align-middle"
                        aria-label="changed from default"
                        title="value differs from PX4 default"
                      />
                    )}
                  </TableCell>
                  <TableCell className="py-1 font-mono text-[10px] text-muted-foreground">{p.group || '—'}</TableCell>
                  <TableCell className="py-1">
                    <Input
                      inputMode="decimal"
                      className={`h-7 font-mono text-[11px] ${dirty ? 'border-sky-500/60 text-sky-700 dark:text-sky-300' : ''}`}
                      value={draft}
                      disabled={setup.busy}
                      onChange={(e) => setDrafts((d) => ({ ...d, [p.id]: e.target.value }))}
                    />
                  </TableCell>
                  <TableCell className="py-1 font-mono text-[10px] text-muted-foreground">
                    {p.default == null ? '—' : fmt(p.default, 4)}
                  </TableCell>
                  <TableCell className="py-1 font-mono text-[10px] text-muted-foreground">
                    {p.kind || (p.type === 9 ? 'f32' : p.type === 6 ? 'i32' : p.type === 1 ? 'i8' : `t${p.type}`)}
                  </TableCell>
                  <TableCell className="py-1">
                    <Button
                      size="sm"
                      variant={dirty ? 'default' : 'ghost'}
                      className="h-7 px-2 text-[10px]"
                      disabled={!dirty || setup.busy}
                      onClick={() => {
                        const v = Number(draft)
                        if (Number.isFinite(v)) onWrite(p.id, v)
                      }}
                    >
                      Write
                    </Button>
                  </TableCell>
                </TableRow>
              )
            })}
            {rows.length === 0 && (
              <TableRow>
                <TableCell colSpan={6} className="py-6 text-center text-xs text-muted-foreground">
                  {sourceStore == null || sourceStore.params.length === 0
                    ? diffOnly
                      ? 'no changed-from-default params — every row matches PX4 defaults'
                      : 'no parameters cached — press Download (PARAM_REQUEST_LIST)'
                    : 'no matches'}
                </TableCell>
              </TableRow>
            )}
          </TableBody>
        </Table>
      </ScrollArea>

      {/* ---------------------------------- Presets panel (below the table) */}
      <div className="mt-4 rounded-md border border-border/60 p-3">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <ListTree className="size-3.5 text-muted-foreground" aria-hidden="true" />
            <span className="text-xs font-medium">Saved presets (vehicle {setup.index + 1})</span>
            <Badge variant="outline" className="h-4 px-1 text-[9px]">{presets.length}</Badge>
          </div>
          <Button
            size="sm"
            variant="ghost"
            className="h-7 gap-1.5 text-[11px]"
            disabled={loadingPresets}
            onClick={() => void refreshPresets()}
          >
            <RotateCw className={`size-3 ${loadingPresets ? 'animate-spin' : ''}`} aria-hidden="true" />
            Refresh
          </Button>
        </div>
        <div className="mt-2">
          {presets.length === 0 ? (
            <div className="rounded-md bg-muted/40 px-3 py-4 text-center text-[11px] text-muted-foreground">
              {loadingPresets ? 'loading presets…' : 'no presets saved — click "Save as preset" above to store the current parameter set'}
            </div>
          ) : (
            <ul className="flex flex-col divide-y divide-border/40">
              {presets.map((p) => (
                <li key={p.name} className="flex items-center justify-between py-1.5">
                  <div className="flex min-w-0 items-center gap-2">
                    <span className="truncate font-mono text-[11px]">{p.name}</span>
                    <Badge variant="outline" className="h-4 px-1 text-[9px]">{p.param_count} params</Badge>
                    {p.created_at && (
                      <span className="font-mono text-[9px] text-muted-foreground">
                        {p.created_at.length > 19 ? p.created_at.slice(0, 19).replace('T', ' ') : p.created_at}
                      </span>
                    )}
                  </div>
                  <div className="flex items-center gap-1">
                    <Button
                      size="sm"
                      variant="outline"
                      className="h-6 gap-1 px-2 text-[10px]"
                      disabled={loadingPresetName != null || setup.busy}
                      onClick={() => void doLoadPreset(p.name)}
                      title={`Load preset '${p.name}' and apply each param via PARAM_SET`}
                    >
                      {loadingPresetName === p.name ? (
                        <RotateCw className="size-3 animate-spin" aria-hidden="true" />
                      ) : (
                        <Upload className="size-3" aria-hidden="true" />
                      )}
                      Load
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="h-6 px-2 text-[10px] text-rose-600 hover:bg-rose-500/10 hover:text-rose-700 dark:text-rose-400"
                      disabled={loadingPresetName != null || setup.busy}
                      onClick={() => setConfirmDelete(p.name)}
                      title={`Delete preset '${p.name}'`}
                    >
                      <Trash2 className="size-3" aria-hidden="true" />
                    </Button>
                  </div>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>

      {/* ----------------------------- Save Preset modal */}
      <AlertDialog open={savePresetOpen} onOpenChange={setSavePresetOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle className="flex items-center gap-2">
              <Save className="size-4" aria-hidden="true" /> Save parameter preset
            </AlertDialogTitle>
            <AlertDialogDescription>
              Saves the current parameter cache ({store?.params.length ?? 0} params) for vehicle {setup.index + 1}
              {' '}as a named preset. POST <code className="rounded bg-muted px-1 font-mono text-[10px]">/api/vehicles/{setup.index}/param-presets</code>
              {' '}on <code className="rounded bg-muted px-1 font-mono text-[10px]">:{setup.catalogPort}</code>.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="flex flex-col gap-2">
            <Label htmlFor="preset-name" className="text-xs">Preset name</Label>
            <Input
              id="preset-name"
              value={presetName}
              onChange={(e) => setPresetName(e.target.value)}
              placeholder="e.g. aggressive-corners"
              className="font-mono text-sm"
              autoFocus
              onKeyDown={(e) => {
                if (e.key === 'Enter' && presetName.trim() && !savingPreset) {
                  void doSavePreset()
                }
              }}
            />
            <p className="text-[11px] text-muted-foreground">
              QGC convention: lowercase, dashes, no spaces. The preset is stored on the catalog (:8300) keyed by name.
            </p>
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={savingPreset}>Cancel</AlertDialogCancel>
            <AlertDialogAction
              disabled={!presetName.trim() || savingPreset}
              onClick={(e) => {
                e.preventDefault()
                void doSavePreset()
              }}
            >
              {savingPreset ? 'Saving…' : 'Save preset'}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* ----------------------------- Load Preset modal */}
      <AlertDialog open={loadPresetOpen} onOpenChange={setLoadPresetOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle className="flex items-center gap-2">
              <Upload className="size-4" aria-hidden="true" /> Load preset
            </AlertDialogTitle>
            <AlertDialogDescription>
              Loads a saved preset and applies each parameter via PARAM_SET (echo-confirmed). The
              {' '}<code className="rounded bg-muted px-1 font-mono text-[10px]">:{setup.catalogPort}</code>
              {' '}catalog returns the params, then this UI writes them to vehicle {setup.index + 1}
              {' '}through <code className="rounded bg-muted px-1 font-mono text-[10px]">POST /api/vehicles/{setup.index}/params</code>.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <div className="max-h-[16rem] overflow-y-auto rounded-md border border-border/60">
            {loadingPresets ? (
              <div className="px-3 py-6 text-center text-xs text-muted-foreground">loading presets…</div>
            ) : presets.length === 0 ? (
              <div className="px-3 py-6 text-center text-xs text-muted-foreground">
                no presets saved — close this dialog, then click &ldquo;Save as preset&rdquo; to create one
              </div>
            ) : (
              <ul className="flex flex-col divide-y divide-border/40">
                {presets.map((p) => (
                  <li key={p.name} className="flex items-center justify-between px-3 py-2">
                    <div className="flex min-w-0 items-center gap-2">
                      <span className="truncate font-mono text-xs">{p.name}</span>
                      <Badge variant="outline" className="h-4 px-1 text-[9px]">{p.param_count} params</Badge>
                    </div>
                    <Button
                      size="sm"
                      className="h-7 gap-1 px-2 text-[10px]"
                      disabled={loadingPresetName != null || setup.busy}
                      onClick={() => void doLoadPreset(p.name)}
                    >
                      {loadingPresetName === p.name ? (
                        <RotateCw className="size-3 animate-spin" aria-hidden="true" />
                      ) : (
                        <Upload className="size-3" aria-hidden="true" />
                      )}
                      Load
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </div>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={loadingPresetName != null}>Close</AlertDialogCancel>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      {/* ----------------------------- Delete Preset confirm */}
      <AlertDialog
        open={confirmDelete != null}
        onOpenChange={(o) => { if (!o) setConfirmDelete(null) }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle className="flex items-center gap-2">
              <Trash2 className="size-4 text-rose-600" aria-hidden="true" /> Delete preset?
            </AlertDialogTitle>
            <AlertDialogDescription>
              This permanently deletes <code className="rounded bg-muted px-1 font-mono text-[11px]">{confirmDelete}</code>
              {' '}from <code className="rounded bg-muted px-1 font-mono text-[11px]">:{setup.catalogPort}</code>.
              {' '}This action cannot be undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              className="bg-rose-600 hover:bg-rose-700"
              onClick={(e) => {
                e.preventDefault()
                if (confirmDelete) void doDeletePreset(confirmDelete)
              }}
            >
              Delete
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
