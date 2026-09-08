'use client'

/**
 * Single-page operator console for rustsitsim (:8200) + mavfleet (:8400)
 * + fleet-catalog (:8300). Mode switcher: Plan / Fly / Sim Console / Fleet
 * C2 / Operator Map / Vehicle Setup. All consoles stay mounted (hidden by
 * CSS) so their telemetry engines keep running across mode switches.
 */

import { useState } from 'react'
import { Drone, MapPin, Network, PlaneTakeoff, Route, Wrench } from 'lucide-react'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { SimConsole } from '@/components/dashboard/SimConsole'
import { FleetC2 } from '@/components/dashboard/FleetC2'
import { OperatorMap } from '@/components/dashboard/OperatorMap'
import { VehicleSetup } from '@/components/dashboard/VehicleSetup'
import { PlanView } from '@/components/dashboard/PlanView'
import { FlyView } from '@/components/dashboard/FlyView'
import { ThemeToggle } from '@/components/dashboard/ThemeToggle'
import { useVehicleSetup } from '@/hooks/useVehicleSetup'
import { useOperatorMap } from '@/hooks/useOperatorMap'

type Mode = 'plan' | 'fly' | 'sim' | 'fleet' | 'map' | 'setup'

export default function Home() {
  const [mode, setMode] = useState<Mode>('plan')
  // the setup engine lives at page level so it survives tab switches
  // (vehicle count follows the fleet snapshot when live; 2 = the demo
  // scenario / mock fleet)
  const setup = useVehicleSetup(2)
  // the operator-map engine likewise (ADR-0017: its own fleet-plane client)
  const op = useOperatorMap()

  return (
    <div className="flex min-h-screen flex-col bg-background text-foreground">
      <header className="sticky top-0 z-40 border-b border-border bg-background/85 backdrop-blur supports-[backdrop-filter]:bg-background/70">
        <div className="mx-auto flex w-full max-w-7xl flex-wrap items-center gap-x-4 gap-y-2 px-4 py-2.5">
          <div className="flex min-w-0 items-center gap-2.5">
            <span
              className="flex size-8 shrink-0 items-center justify-center rounded-lg bg-emerald-600/10 text-emerald-600 dark:text-emerald-400"
              aria-hidden="true"
            >
              <Drone className="size-4.5" />
            </span>
            <div className="min-w-0">
              <h1 className="truncate text-sm font-semibold leading-tight">PX4 Operator Console</h1>
              <p className="truncate text-[11px] text-muted-foreground">rustsitsim · mavfleet · fleet-catalog — sandbox gateway plane</p>
            </div>
          </div>

          <nav aria-label="Console mode" className="order-3 w-full sm:order-none sm:w-auto">
            <Tabs value={mode} onValueChange={(v) => setMode(v as Mode)}>
              <TabsList className="grid h-10 w-full grid-cols-6 sm:w-auto">
                <TabsTrigger value="plan" className="gap-1.5 px-3">
                  <Route className="size-3.5" aria-hidden="true" />
                  Plan
                  <span className="sr-only">mission editor with map, geofence drawing, validate, save, upload</span>
                </TabsTrigger>
                <TabsTrigger value="fly" className="gap-1.5 px-3">
                  <PlaneTakeoff className="size-3.5" aria-hidden="true" />
                  Fly
                  <span className="sr-only">live map, attitude HUD, instruments, pre-arm checks, action bar</span>
                </TabsTrigger>
                <TabsTrigger value="sim" className="gap-1.5 px-3">
                  <Drone className="size-3.5" aria-hidden="true" />
                  Sim Console
                  <span className="sr-only">rustsitsim HIL simulator</span>
                </TabsTrigger>
                <TabsTrigger value="fleet" className="gap-1.5 px-3">
                  <Network className="size-3.5" aria-hidden="true" />
                  Fleet C2
                  <span className="sr-only">mavfleet fleet manager</span>
                </TabsTrigger>
                <TabsTrigger value="map" className="gap-1.5 px-3">
                  <MapPin className="size-3.5" aria-hidden="true" />
                  Operator Map
                  <span className="sr-only">geo map with direct SITL control</span>
                </TabsTrigger>
                <TabsTrigger value="setup" className="gap-1.5 px-3">
                  <Wrench className="size-3.5" aria-hidden="true" />
                  Vehicle Setup
                  <span className="sr-only">QGC-style vehicle configuration</span>
                </TabsTrigger>
              </TabsList>
            </Tabs>
          </nav>

          <div className="ml-auto flex items-center gap-1.5">
            <span className="hidden font-mono text-[10px] text-muted-foreground md:inline" aria-hidden="true">
              :8200 / :8300 / :8400
            </span>
            <ThemeToggle />
          </div>
        </div>
      </header>

      <main className="mx-auto w-full max-w-7xl flex-1 px-4 py-4">
        <Tabs value={mode} onValueChange={(v) => setMode(v as Mode)}>
          <TabsContent value="plan" forceMount className="mt-0 data-[state=inactive]:hidden">
            <PlanView />
          </TabsContent>
          <TabsContent value="fly" forceMount className="mt-0 data-[state=inactive]:hidden">
            <FlyView op={op} />
          </TabsContent>
          <TabsContent value="sim" forceMount className="mt-0 data-[state=inactive]:hidden">
            <SimConsole />
          </TabsContent>
          <TabsContent value="fleet" forceMount className="mt-0 data-[state=inactive]:hidden">
            <FleetC2 />
          </TabsContent>
          <TabsContent value="map" forceMount className="mt-0 data-[state=inactive]:hidden">
            <OperatorMap op={op} />
          </TabsContent>
          <TabsContent value="setup" forceMount className="mt-0 data-[state=inactive]:hidden">
            <VehicleSetup setup={setup} />
          </TabsContent>
        </Tabs>
      </main>

      <footer className="mt-auto border-t border-border bg-background/60">
        <div className="mx-auto flex w-full max-w-7xl flex-wrap items-center justify-between gap-x-4 gap-y-1 px-4 py-2.5 pb-[max(0.625rem,env(safe-area-inset-bottom))]">
          <p className="font-mono text-[10px] text-muted-foreground">
            rustsitsim :8200 · catalog :8300 · mavfleet :8400 — relative-path fetch + <span className="font-semibold">XTransformPort</span> gateway routing
          </p>
          <p className="font-mono text-[10px] text-muted-foreground">
            Plan View (GCS_SPEC §5.1/§8.1) · Fly View (GCS_SPEC §5.2/§8.2) · 10 Hz sim/fleet · setup per ADR-0016 · op-map per ADR-0017 · catalog per ADR-0019/0020/0026
          </p>
        </div>
      </footer>
    </div>
  )
}
