'use client'

/** Fleet table (mavfleet SPEC §13): per-vehicle FSM state, mode, battery, NED, heartbeat age, task. */

import { Badge } from '@/components/ui/badge'
import { Progress } from '@/components/ui/progress'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { batteryTone, fmt, fmtSigned } from '@/lib/format'
import { fsmStyle } from '@/lib/fsm'
import type { FleetTask, FleetVehicle } from '@/lib/types'
import { cn } from '@/lib/utils'

export function FleetTable({
  vehicles,
  tasks,
  selected,
  onSelect,
}: {
  vehicles: FleetVehicle[]
  tasks: FleetTask[]
  selected: string | null
  onSelect: (id: string) => void
}) {
  const taskById = new Map(tasks.map((t) => [t.id, t]))

  return (
    <div className="overflow-x-auto">
      <Table className="min-w-[720px]">
        <TableHeader>
          <TableRow>
            <TableHead className="w-24">Vehicle</TableHead>
            <TableHead className="w-14 text-center">Sysid</TableHead>
            <TableHead className="w-28">Mode</TableHead>
            <TableHead className="w-24">FSM</TableHead>
            <TableHead className="w-32">Battery</TableHead>
            <TableHead className="w-52">Position NED (m)</TableHead>
            <TableHead className="w-20 text-right">HB age</TableHead>
            <TableHead className="w-24">Task</TableHead>
            <TableHead className="min-w-40">Health</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {vehicles.length === 0 && (
            <TableRow>
              <TableCell colSpan={9} className="py-8 text-center text-sm text-muted-foreground">
                No vehicles reported yet
              </TableCell>
            </TableRow>
          )}
          {vehicles.map((v) => {
            const tone = batteryTone(v.battery_pct)
            const hb = v.heartbeat_age_s
            const task = v.task_id ? taskById.get(v.task_id) : undefined
            return (
              <TableRow
                key={v.id}
                onClick={() => onSelect(v.id)}
                aria-selected={selected === v.id}
                className={cn('cursor-pointer', selected === v.id && 'bg-emerald-500/5 outline outline-1 outline-emerald-500/40')}
              >
                <TableCell className="font-mono font-semibold">{v.id}</TableCell>
                <TableCell className="text-center font-mono text-muted-foreground">{v.sysid}</TableCell>
                <TableCell className="text-muted-foreground">{v.mode}</TableCell>
                <TableCell>
                  <Badge variant="outline" className={cn('font-mono text-[10px]', fsmStyle(v.fsm).css)}>
                    {v.fsm}
                  </Badge>
                </TableCell>
                <TableCell>
                  <div className="flex items-center gap-2">
                    <Progress
                      value={Math.max(0, Math.min(100, v.battery_pct))}
                      className={cn(
                        'h-1.5 w-16',
                        tone === 'crit' ? '[&>div]:bg-rose-500' : tone === 'warn' ? '[&>div]:bg-amber-500' : '[&>div]:bg-emerald-500',
                      )}
                    />
                    <span
                      className={cn(
                        'font-mono text-xs tabular-nums',
                        tone === 'crit' ? 'text-rose-600 dark:text-rose-400' : tone === 'warn' ? 'text-amber-600 dark:text-amber-400' : 'text-foreground',
                      )}
                    >
                      {fmt(v.battery_pct, 1)}%
                    </span>
                  </div>
                </TableCell>
                <TableCell className="font-mono text-xs tabular-nums text-muted-foreground">
                  {fmtSigned(v.position_ned_m[0], 1)} / {fmtSigned(v.position_ned_m[1], 1)} / {fmtSigned(v.position_ned_m[2], 1)}
                </TableCell>
                <TableCell
                  className={cn(
                    'text-right font-mono text-xs tabular-nums',
                    hb > 3 ? 'text-rose-600 dark:text-rose-400' : hb > 1.5 ? 'text-amber-600 dark:text-amber-400' : 'text-muted-foreground',
                  )}
                >
                  {fmt(hb, 2)} s
                </TableCell>
                <TableCell>
                  {task ? (
                    <span className="font-mono text-xs">{task.id}</span>
                  ) : (
                    <span className="text-xs text-muted-foreground">—</span>
                  )}
                </TableCell>
                <TableCell>
                  {v.health.length === 0 ? (
                    <span className="text-xs text-muted-foreground/70">ok</span>
                  ) : (
                    <div className="flex flex-wrap gap-1">
                      {v.health.map((h) => (
                        <Badge
                          key={h}
                          variant="outline"
                          className={cn(
                            'px-1.5 text-[9px] font-mono',
                            h.startsWith('BATTERY_CRIT') || h.startsWith('HEARTBEAT')
                              ? 'border-rose-500/40 bg-rose-500/10 text-rose-700 dark:text-rose-400'
                              : 'border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-400',
                          )}
                        >
                          {h}
                        </Badge>
                      ))}
                    </div>
                  )}
                </TableCell>
              </TableRow>
            )
          })}
        </TableBody>
      </Table>
    </div>
  )
}
