'use client'

/**
 * GCS v2 Operations Canvas — the zone grid (M8, T-B1).
 *
 * Spec: docs/GCS_V2_SPEC.md §4 — Information Architecture.
 *
 * The full-bleed MapLibre canvas (zone E) covers `inset: 0`; edge furniture
 * (A/C/D) overlays it with translucent `rgba(11,15,20,0.88)` + 8px backdrop-
 * blur-equivalent translucency (solid rgba fallback for print/PDF — §2.2).
 * One `pointer-events: none` overlay root spans the canvas; interactive
 * widgets inside set `pointer-events: auto` (§4.4 — the HUD-over-map iron
 * rule so map gestures pass through everywhere else).
 *
 * Zone H (event rail) lands at M11; overlay panels F (MissionStrip, Library,
 * FleetC2, SimControl, SetupDrawer, AnalyzeOverlay, PreFlight, CheatSheet)
 * ship with T-B2..B7 at M10..M13.
 *
 * M8 ships: the zone grid skeleton + the FlushLoop mount point + the
 * telemetry-store init/teardown (start on mount, stop on unmount —
 * `startTelemetry` is idempotent against React strict-mode double-mounts).
 */

import { useEffect } from 'react'
import { MapCanvas } from './MapCanvas'
import { StatusStrip } from './strip/StatusStrip'
import { LeftRail } from './rail/LeftRail'
import { TelemetryColumn } from './column/TelemetryColumn'
import { CommandBar } from './bar/CommandBar'
import { NotificationStack } from './notify/NotificationStack'
import { FlushLoop } from './FlushLoop'
import { ShortcutsProvider } from './ShortcutsProvider'
import { MissionStrip } from './overlays/MissionStrip'
import { LibraryPanel } from './overlays/LibraryPanel'
import { FleetC2Panel } from './overlays/FleetC2Panel'
import { SetupDrawer } from './overlays/SetupDrawer'
import { AnalyzeOverlay } from './overlays/AnalyzeOverlay'
import { SimControlPanel } from './overlays/SimControlPanel'
import { ContextMenu } from './map/context-menu'
import { useAppStore } from '@/state/app-store'
import { startTelemetry, stopTelemetry } from '@/state/telemetry-store'

export function OperationsCanvas() {
  // Telemetry lifecycle — start on mount, stop on unmount. The store's
  // `startTelemetry` is idempotent (guards against strict-mode double-mounts;
  // next.config.ts already has reactStrictMode:false but the guard holds).
  useEffect(() => {
    startTelemetry()
    return () => stopTelemetry()
  }, [])

  const app = useAppStore()

  return (
    <div
      className="rsim-canvas relative h-screen w-screen overflow-hidden"
      style={{ background: 'var(--rsim-bg)', color: 'var(--rsim-text)' }}
    >
      {/* Zone E — full-bleed map canvas. The map container itself sets
          `position:absolute; inset:0` so it sits under A/B/C/D (which
          overlay it with translucent surfaces). */}
      <MapCanvas />

      {/* Edge furniture — pointer-events: auto on each zone (§4.4). */}
      <StatusStrip />
      <LeftRail />
      <TelemetryColumn />
      <CommandBar />

      {/* Zone G — notification stack (top-right, below A). */}
      <NotificationStack />

      {/* The single rAF flush loop — owns the HUD ref registry writes. */}
      <FlushLoop />

      {/* M9: global key handler + cheat-sheet dialog (§7.3 + §7.3.3). */}
      <ShortcutsProvider />

      {/* M10 follow-up: right-click context menus (§7.2 M1..M5). */}
      <ContextMenu />

      {/* Zone F overlay panels — MissionStrip + Library + FleetC2 + Setup + Analyze + SimControl (M10..M13) */}
      {app.overlays.mission && <MissionStrip />}
      {app.overlays.library && <LibraryPanel />}
      {app.overlays.fleet && <FleetC2Panel />}
      {app.overlays.setup && <SetupDrawer />}
      {app.overlays.analyze && <AnalyzeOverlay />}
      {app.overlays.sim && <SimControlPanel />}
    </div>
  )
}
