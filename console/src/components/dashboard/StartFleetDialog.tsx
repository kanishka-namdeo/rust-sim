'use client'

/**
 * Start Fleet modal — the §5.4 / §8.4 step-5 surface.
 *
 * AlertDialog-based modal (the existing pattern in FleetC2.tsx for E-STOP).
 * Mode dropdown defaults to "Parallel"; "Sequential" reveals the gate
 * dropdown ("First waypoint reached" / "Takeoff complete") and a timeout
 * input (default 30 s). "Start" POSTs `/api/fleet/start` on :8400, then the
 * modal closes and a toast summarises the per-vehicle result list
 * (started/failed/timeout).
 */

import { useCallback, useState } from 'react'
import { Loader2, Play } from 'lucide-react'
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import { useToast } from '@/hooks/use-toast'
import type { FleetC2Api } from '@/hooks/useFleetC2'
import type { FleetStartMode, FleetStartResult, SequentialGate } from '@/lib/types'

export function StartFleetDialog({ fleet, disabled }: { fleet: FleetC2Api; disabled: boolean }) {
  const { toast } = useToast()
  const [open, setOpen] = useState(false)
  const [mode, setMode] = useState<FleetStartMode>('parallel')
  const [gate, setGate] = useState<SequentialGate>('first_waypoint')
  const [timeoutS, setTimeoutS] = useState(30)
  const [starting, setStarting] = useState(false)

  /** Reset to the SPEC defaults every time the modal is opened so the
   * operator can't accidentally inherit the previous sequential timeout. */
  const onOpenChange = useCallback((next: boolean) => {
    setOpen(next)
    if (next) {
      setMode('parallel')
      setGate('first_waypoint')
      setTimeoutS(30)
    }
  }, [])

  const onStart = useCallback(async () => {
    setStarting(true)
    try {
      const res = await fleet.startFleet(mode, mode === 'sequential' ? gate : undefined, mode === 'sequential' ? timeoutS : undefined)
      if (res.ok) {
        const summary = summarise(res.results)
        toast({
          title: `Fleet start ${summary.allStarted ? 'accepted' : 'partial'}`,
          description: summary.line,
        })
        if (summary.anyTimeout) {
          toast({
            title: 'Sequential gate timeout',
            description: 'one or more vehicles did not meet the gate in time — fleet aborted',
            variant: 'destructive',
          })
        }
      } else {
        toast({
          title: 'Fleet start failed',
          description: res.error ?? 'POST /api/fleet/start rejected',
          variant: 'destructive',
        })
      }
      setOpen(false)
    } finally {
      setStarting(false)
    }
  }, [fleet, mode, gate, timeoutS, toast])

  const sequential = mode === 'sequential'

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogTrigger asChild>
        <Button size="sm" className="gap-1.5" disabled={disabled || starting} aria-label="Start fleet — orchestrate all bound missions">
          {starting ? <Loader2 className="size-3.5 animate-spin" aria-hidden="true" /> : <Play className="size-3.5" aria-hidden="true" />}
          Start Fleet
        </Button>
      </AlertDialogTrigger>
      <AlertDialogContent className="sm:max-w-md">
        <AlertDialogHeader>
          <AlertDialogTitle>Start fleet</AlertDialogTitle>
          <AlertDialogDescription>
            Orchestrate every bound mission (§5.4). Parallel starts all vehicles at once; Sequential waits for the gate on each vehicle before the next.
          </AlertDialogDescription>
        </AlertDialogHeader>

        <div className="flex flex-col gap-3">
          <div className="flex flex-col gap-1.5">
            <Label htmlFor="fleet-start-mode" className="text-xs">Mode</Label>
            <Select value={mode} onValueChange={(v) => setMode(v as FleetStartMode)}>
              <SelectTrigger id="fleet-start-mode" className="w-full">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value="parallel">Parallel</SelectItem>
                <SelectItem value="sequential">Sequential</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {sequential && (
            <>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="fleet-start-gate" className="text-xs">Sequential gate</Label>
                <Select value={gate} onValueChange={(v) => setGate(v as SequentialGate)}>
                  <SelectTrigger id="fleet-start-gate" className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="first_waypoint">First waypoint reached</SelectItem>
                    <SelectItem value="takeoff_complete">Takeoff complete</SelectItem>
                  </SelectContent>
                </Select>
              </div>
              <div className="flex flex-col gap-1.5">
                <Label htmlFor="fleet-start-timeout" className="text-xs">Gate timeout (s)</Label>
                <Input
                  id="fleet-start-timeout"
                  type="number"
                  inputMode="numeric"
                  min={1}
                  max={300}
                  step={1}
                  value={timeoutS}
                  onChange={(e) => {
                    const n = Number(e.target.value)
                    setTimeoutS(Number.isFinite(n) && n > 0 ? Math.min(300, Math.round(n)) : 30)
                  }}
                />
                <p className="text-[11px] text-muted-foreground">
                  Fleet aborts if any vehicle fails to meet the gate within this window.
                </p>
              </div>
            </>
          )}
        </div>

        <AlertDialogFooter>
          <AlertDialogCancel disabled={starting}>Cancel</AlertDialogCancel>
          <Button onClick={(e) => { e.preventDefault(); void onStart() }} disabled={starting}>
            {starting ? <Loader2 className="size-3.5 animate-spin" aria-hidden="true" /> : <Play className="size-3.5" aria-hidden="true" />}
            Start
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}

/** Build a one-line summary of the start result for the toast. */
function summarise(results: FleetStartResult[]): { allStarted: boolean; anyTimeout: boolean; line: string } {
  if (results.length === 0) return { allStarted: false, anyTimeout: false, line: 'no vehicles reported' }
  const started = results.filter((r) => r.status === 'started').length
  const failed = results.filter((r) => r.status === 'failed').length
  const timedOut = results.filter((r) => r.status === 'timeout').length
  const allStarted = started === results.length
  const anyTimeout = timedOut > 0
  const parts: string[] = [`${started}/${results.length} started`]
  if (failed > 0) parts.push(`${failed} failed`)
  if (timedOut > 0) parts.push(`${timedOut} timeout`)
  return { allStarted, anyTimeout, line: parts.join(' · ') }
}
