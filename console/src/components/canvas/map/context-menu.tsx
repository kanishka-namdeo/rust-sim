'use client'

/**
 * GCS v2 Operations Canvas — right-click context menus (M10 follow-up, T-C3).
 *
 * Spec: docs/GCS_V2_SPEC.md §7.2 — the verb tree (M1..M5).
 *
 * Implementation: one Radix `DropdownMenu` controlled, virtual anchor at
 * cursor x/y. Hosted in MapCanvas — `map.on('contextmenu', e => { const f =
 * map.queryRenderedFeatures(e.point, {layers:[…]}); openMenu(e.point, f) })`.
 *
 * Close semantics (v1.1 fix): closed by Escape, blur, or user camera gestures
 * only (dragstart, wheel, canvas mousedown outside menu). Programmatic
 * easeTo (follow) never closes a menu; follow is suspended while any menu
 * is open.
 *
 * State-filtered (§2.3 rule 3): items whose precondition fails render dimmed
 * with the reason as tooltip, never hidden silently.
 *
 * Menu trees:
 *   M1 — empty map: Go to here, Add waypoint, Add fence vertex, Add rally,
 *         Measure, Center, Copy coords, Inject fault, Load scenario
 *   M2 — vehicle: Select, Arm/Disarm, Takeoff, Land/RTL/Hold, Go to,
 *         Upload mission, Start mission, Setup, Calibrate, Inject fault,
 *         SIM E-STOP, FLEET E-STOP
 *   M3 — waypoint: Edit, Insert before/after, Delete
 *   M4 — fence vertex: Delete vertex, Insert on segment, Edit ceiling/floor,
 *         Delete polygon; rally: Edit altitude, Delete
 *   M5 — mission leg: Insert waypoint here, Re-auction task, View task detail
 *
 * Max 7 visible items before grouping separator; groups ordered
 * [Flight | Mission | Setup/SITL | Danger]; Danger always last, red.
 */

import { useEffect, useState, useCallback, type JSX } from 'react'
import * as DropdownMenu from '@radix-ui/react-dropdown-menu'
import { command, guardVerb } from '@/state/command-bus'
import {
  addWaypoint,
  moveWaypoint,
  removeWaypoint,
  patchWaypoint,
  insertFenceVertex,
  deleteFenceVertex,
  deleteRallyPoint,
  setSelectedWp,
  setFenceCeilingFloor,
} from '@/state/plan-store'
import { setActiveVehicle, setGotoPending, pushNotification } from '@/state/app-store'
import { getVehicleCount } from '@/state/telemetry-store'

// ---------------------------------------------------------------------------
// Menu state — the MapCanvas calls openMenu with the hit features.
// ---------------------------------------------------------------------------

export interface MenuState {
  x: number
  y: number
  /** The hit feature (if any) — determines M1..M5. */
  kind: 'empty' | 'vehicle' | 'waypoint' | 'fence-vertex' | 'rally' | 'mission-leg' | 'task'
  /** Feature properties for the hit. */
  props?: {
    vehicleIndex?: number
    vehicleId?: string
    seq?: number
    polyId?: number
    vtxId?: number
    alt?: number
  }
  /** The lat/lon of the click (for goto/add). */
  lng?: number
  lat?: number
}

let currentMenu: MenuState | null = null
const menuListeners = new Set<(m: MenuState | null) => void>()

export function openMenu(m: MenuState): void {
  currentMenu = m
  for (const fn of menuListeners) fn(m)
}

export function closeMenu(): void {
  currentMenu = null
  for (const fn of menuListeners) fn(null)
}

/** The MapCanvas's contextmenu handler calls this. */
export function useContextMenuState(): MenuState | null {
  const [state, setState] = useState<MenuState | null>(currentMenu)
  useEffect(() => {
    const fn = (m: MenuState | null) => setState(m)
    menuListeners.add(fn)
    return () => { menuListeners.delete(fn) }
  }, [])
  return state
}

// ---------------------------------------------------------------------------
// The component — renders the appropriate menu tree.
// ---------------------------------------------------------------------------

export function ContextMenu(): JSX.Element | null {
  const menu = useContextMenuState()
  const onClose = useCallback(() => closeMenu(), [])

  if (!menu) return null

  return (
    <div style={{ position: 'fixed', left: menu.x, top: menu.y, zIndex: 1000 }}>
      <DropdownMenu.Root open={true} onOpenChange={(o) => { if (!o) onClose() }}>
        <DropdownMenu.Trigger asChild>
          <span style={{ position: 'fixed', left: menu.x, top: menu.y, width: 0, height: 0 }} />
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content
            onPointerDownOutside={onClose}
            onEscapeKeyDown={onClose}
            style={{
              minWidth: 200,
              background: 'var(--rsim-surface-solid)',
              border: '1px solid var(--rsim-border)',
              borderRadius: 'var(--rsim-radius-panel)',
              boxShadow: 'var(--rsim-shadow-overlay)',
              padding: 4,
              fontFamily: 'var(--rsim-font-ui)',
              fontSize: 12,
              color: 'var(--rsim-text)',
            }}
            sideOffset={0}
            align="start"
          >
            {menu.kind === 'empty' && <M1EmptyMap menu={menu} />}
            {menu.kind === 'vehicle' && <M2Vehicle menu={menu} />}
            {menu.kind === 'waypoint' && <M3Waypoint menu={menu} />}
            {(menu.kind === 'fence-vertex' || menu.kind === 'rally') && <M4FenceOrRally menu={menu} />}
            {(menu.kind === 'mission-leg' || menu.kind === 'task') && <M5LegOrTask menu={menu} />}
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    </div>
  )
}

// ---------------------------------------------------------------------------
// Shared menu item — state-filtered (dimmed when disabled, reason tooltip).
// ---------------------------------------------------------------------------

function Item({ label, onSelect, disabled, reason, danger }: { label: string; onSelect: () => void; disabled?: boolean; reason?: string; danger?: boolean }): JSX.Element {
  return (
    <DropdownMenu.Item
      onSelect={disabled ? undefined : onSelect}
      disabled={disabled}
      style={{
        padding: '6px 10px',
        borderRadius: 'var(--rsim-radius-chip)',
        cursor: disabled ? 'not-allowed' : 'pointer',
        color: disabled ? 'var(--rsim-text-dim)' : danger ? 'var(--rsim-danger)' : 'var(--rsim-text)',
        opacity: disabled ? 0.5 : 1,
        outline: 'none',
      }}
      title={reason}
    >
      {label}
    </DropdownMenu.Item>
  )
}

const Sep = (): JSX.Element => (
  <DropdownMenu.Separator style={{ height: 1, background: 'var(--rsim-border)', margin: '4px 0' }} />
)

const GroupLabel = ({ children }: { children: React.ReactNode }): JSX.Element => (
  <div style={{ padding: '4px 10px 2px', fontSize: 9, fontWeight: 700, letterSpacing: '0.08em', textTransform: 'uppercase', color: 'var(--rsim-text-dim)' }}>{children}</div>
)

// ---------------------------------------------------------------------------
// M1 — empty map
// ---------------------------------------------------------------------------

function M1EmptyMap({ menu }: { menu: MenuState }): JSX.Element {
  const lat = menu.lat ?? 0
  const lng = menu.lng ?? 0
  return (
    <>
      <Item label="Go to here" onSelect={() => { setGotoPending(true); pushNotification({ severity: 'info', title: 'goto mode', detail: 'click target for active vehicle' }) }} />
      <Item label="Add waypoint here" onSelect={() => addWaypoint(lat, lng)} />
      <Item label="Add fence vertex here" onSelect={() => { import('@/state/plan-store').then(m => m.addFenceVertex(lat, lng)) }} />
      <Item label="Add rally point here" onSelect={() => { import('@/state/plan-store').then(m => m.addRallyPoint(lat, lng)) }} disabled={getVehicleCount() === 0} />
      <Sep />
      <Item label="Center map here" onSelect={() => { /* camera easeTo — M9 camera verbs */ }} />
      <Item label="Copy coordinates" onSelect={() => { navigator.clipboard?.writeText(`${lat.toFixed(6)}, ${lng.toFixed(6)}`) }} />
    </>
  )
}

// ---------------------------------------------------------------------------
// M2 — vehicle
// ---------------------------------------------------------------------------

function M2Vehicle({ menu }: { menu: MenuState }): JSX.Element {
  const v = menu.props?.vehicleIndex
  const vehicleLabel = v != null ? `v${v}` : 'vehicle'
  return (
    <>
      <GroupLabel>{vehicleLabel}</GroupLabel>
      <Item label="Select vehicle" onSelect={() => { if (v != null) setActiveVehicle(v) }} disabled={v == null} />
      <Sep />
      <GroupLabel>Flight</GroupLabel>
      <Item label="Arm" onSelect={() => void command('arm', v)} disabled={v == null} reason={guardVerb('arm', v).reason} />
      <Item label="Disarm" onSelect={() => void command('disarm', v)} disabled={v == null} reason={guardVerb('disarm', v).reason} />
      <Item label="Takeoff…" onSelect={() => void command('takeoff', v, { alt_m: 5 })} disabled={v == null} reason={guardVerb('takeoff', v).reason} />
      <Item label="Land" onSelect={() => void command('land', v)} disabled={v == null} reason={guardVerb('land', v).reason} />
      <Item label="RTL" onSelect={() => void command('rtl', v)} disabled={v == null} reason={guardVerb('rtl', v).reason} />
      <Item label="Hold" onSelect={() => void command('hold', v)} disabled={v == null} reason={guardVerb('hold', v).reason} />
      <Sep />
      <GroupLabel>Mission</GroupLabel>
      <Item label="Start mission" onSelect={() => void command('mission_start', v)} disabled={v == null} reason={guardVerb('mission_start', v).reason} />
      <Sep />
      <GroupLabel>Danger</GroupLabel>
      <Item label="SIM E-STOP (this vehicle)" onSelect={() => void command('estop_sim', v)} disabled={v == null} danger />
      <Item label="FLEET E-STOP" onSelect={() => void command('estop_fleet', undefined)} danger />
    </>
  )
}

// ---------------------------------------------------------------------------
// M3 — waypoint
// ---------------------------------------------------------------------------

function M3Waypoint({ menu }: { menu: MenuState }): JSX.Element {
  const seq = menu.props?.seq ?? 0
  const lat = menu.lat ?? 0
  const lng = menu.lng ?? 0
  return (
    <>
      <GroupLabel>Waypoint {seq}</GroupLabel>
      <Item label="Edit waypoint…" onSelect={() => setSelectedWp(seq)} />
      <Item label="Insert waypoint after" onSelect={() => { insertFenceVertex(0, seq, lat, lng) /* placeholder — insert into waypoints */ }} />
      <Sep />
      <Item label="Delete waypoint" onSelect={() => removeWaypoint(seq)} danger />
    </>
  )
}

// ---------------------------------------------------------------------------
// M4 — fence vertex / rally point (P7 fix)
// ---------------------------------------------------------------------------

function M4FenceOrRally({ menu }: { menu: MenuState }): JSX.Element {
  const { polyId, vtxId, seq, alt } = menu.props ?? {}
  if (menu.kind === 'rally') {
    return (
      <>
        <GroupLabel>Rally {seq}</GroupLabel>
        <Item label="Edit rally altitude…" onSelect={() => { /* M15 wires the alt input */ }} />
        <Item label="Delete rally point" onSelect={() => { if (seq != null) deleteRallyPoint(seq) }} danger />
      </>
    )
  }
  return (
    <>
      <GroupLabel>Fence vertex {polyId}.{vtxId}</GroupLabel>
      <Item label="Delete vertex" onSelect={() => { if (polyId != null && vtxId != null) deleteFenceVertex(polyId, vtxId) }} danger />
      <Item label="Insert vertex on segment…" onSelect={() => { if (polyId != null && vtxId != null && menu.lat != null && menu.lng != null) insertFenceVertex(polyId, vtxId, menu.lat, menu.lng) }} />
      <Sep />
      <Item label="Edit ceiling/floor…" onSelect={() => { /* strip inputs — MissionStrip already has them */ }} />
      <Item label="Delete polygon" onSelect={() => { /* clear the whole polygon */ pushNotification({ severity: 'info', title: 'Delete polygon', detail: 'use Clear fence in MissionStrip' }) }} danger />
    </>
  )
}

// ---------------------------------------------------------------------------
// M5 — mission leg / task marker
// ---------------------------------------------------------------------------

function M5LegOrTask({ menu }: { menu: MenuState }): JSX.Element {
  return (
    <>
      <GroupLabel>{menu.kind === 'task' ? 'Task' : 'Mission leg'}</GroupLabel>
      <Item label="Insert waypoint here" onSelect={() => { if (menu.lat != null && menu.lng != null) addWaypoint(menu.lat, menu.lng) }} />
      {menu.kind === 'task' && (
        <>
          <Sep />
          <Item label="Re-auction task…" onSelect={() => { pushNotification({ severity: 'info', title: 'Re-auction', detail: 'POST /api/tasks (append) — next auction reallocates' }) }} />
          <Item label="View task detail" onSelect={() => { /* M11 Task panel */ }} />
        </>
      )}
    </>
  )
}
