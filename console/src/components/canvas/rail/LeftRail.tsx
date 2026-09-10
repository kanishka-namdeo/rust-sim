'use client'

/**
 * GCS v2 Operations Canvas — Left rail (Zone B, M8 T-B1).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.2 + §2.2 zone B.
 *
 * Fixed 56px wide, full height (between A and D), always visible.
 * 40×40 icon buttons top→bottom with 10px labels:
 *
 *   Plan mode `P` · Mission strip `M` · Library · Fleet C2 `B` ·
 *   Setup `S` · Analyze `Y` · Pre-flight ·
 *   Notifications `F8` · Cheat sheet `?` · Settings
 *
 * Active overlay icons get the accent-dim background; max two open (§4.2).
 *
 * For M8 these toggle the corresponding overlay in the app-store; the
 * overlay panels themselves ship with T-B2..B7 (M10..M13). M8 wires the
 * rail + the toggle so the layout assertions in G-14 (zones A–G present
 * via data-rsim-zone) pass.
 */

import { type ReactElement } from 'react'
import { Plane } from 'lucide-react'
import { useAppStore, toggleOverlay, setMapMode, type OverlayId } from '@/state/app-store'
import { type MapMode } from '@/state/app-store'

interface RailItem {
  id: OverlayId | 'plan-mode' | 'fly-mode' | 'fence-mode' | 'corridor-mode'
  label: string
  shortcut: string
  icon: ReactElement
  onClick: () => void
  active: boolean
}

export function LeftRail() {
  const app = useAppStore()

  const items: RailItem[] = [
    {
      id: 'fly-mode',
      label: 'Fly',
      shortcut: 'F',
      icon: <Plane size={20} aria-hidden="true" />,
      onClick: () => setMapMode('fly'),
      active: app.mapMode === 'fly',
    },
    {
      id: 'plan-mode',
      label: 'Plan',
      shortcut: 'P',
      icon: <MapPin size={20} aria-hidden="true" />,
      onClick: () => setMapMode('plan'),
      active: app.mapMode === 'plan',
    },
    {
      id: 'fence-mode',
      label: 'Fence',
      shortcut: '—',
      icon: <Hexagon size={20} aria-hidden="true" />,
      onClick: () => setMapMode('fence'),
      active: app.mapMode === 'fence',
    },
    {
      id: 'corridor-mode',
      label: 'Corridor',
      shortcut: '—',
      icon: <Route size={20} aria-hidden="true" />,
      onClick: () => setMapMode('corridor'),
      active: app.mapMode === 'corridor',
    },
    {
      id: 'library',
      label: 'Library',
      shortcut: '—',
      icon: <Library size={20} aria-hidden="true" />,
      onClick: () => toggleOverlay('library'),
      active: app.overlays.library,
    },
    {
      id: 'fleet',
      label: 'Fleet C2',
      shortcut: 'B',
      icon: <Network size={20} aria-hidden="true" />,
      onClick: () => toggleOverlay('fleet'),
      active: app.overlays.fleet,
    },
    {
      id: 'sitl',
      label: 'SITL',
      shortcut: '—',
      icon: <Cpu size={20} aria-hidden="true" />,
      onClick: () => toggleOverlay('sitl'),
      active: app.overlays.sitl,
    },
    {
      id: 'setup',
      label: 'Setup',
      shortcut: 'S',
      icon: <Wrench size={20} aria-hidden="true" />,
      onClick: () => toggleOverlay('setup'),
      active: app.overlays.setup,
    },
    {
      id: 'analyze',
      label: 'Analyze',
      shortcut: 'Y',
      icon: <BarChart3 size={20} aria-hidden="true" />,
      onClick: () => toggleOverlay('analyze'),
      active: app.overlays.analyze,
    },
    {
      id: 'preflight',
      label: 'Pre-flight',
      shortcut: '—',
      icon: <ClipboardCheck size={20} aria-hidden="true" />,
      onClick: () => toggleOverlay('preflight'),
      active: app.overlays.preflight,
    },
    {
      id: 'settings',
      label: 'Settings',
      shortcut: '—',
      icon: <Settings size={20} aria-hidden="true" />,
      onClick: () => toggleOverlay('settings'),
      active: app.overlays.settings,
    },
    {
      id: 'cheat',
      label: 'Shortcuts',
      shortcut: '?',
      icon: <Keyboard size={20} aria-hidden="true" />,
      onClick: () => toggleOverlay('cheat'),
      active: app.overlays.cheat,
    },
  ]

  return (
    <div
      data-rsim-zone="B"
      className="pointer-events-auto absolute left-0 flex flex-col items-center gap-1 py-2 border-r"
      style={{ top: 48, bottom: 96, width: 56, zIndex: 20 }}
    >
      {items.map((item) => (
        <button
          key={item.id}
          type="button"
          onClick={item.onClick}
          className="rsim-rail-btn"
          style={{
            width: 40,
            height: 40,
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            justifyContent: 'center',
            gap: 2,
            borderRadius: 'var(--rsim-radius-control)',
            border: '1px solid transparent',
            background: item.active ? 'var(--rsim-accent-dim)' : 'transparent',
            color: item.active ? 'var(--rsim-accent)' : 'var(--rsim-text-dim)',
            cursor: 'pointer',
            transition: 'background 120ms, color 120ms',
          }}
          aria-pressed={item.active}
          aria-label={`${item.label}${item.shortcut !== '—' ? ` (${item.shortcut})` : ' — click to open'}`}
          title={`${item.label} (${item.shortcut})`}
        >
          {item.icon}
          <span style={{ fontSize: 9, fontWeight: 600, letterSpacing: '0.04em' }}>{item.label}</span>
        </button>
      ))}
    </div>
  )
}

// Icons — kept inline so the rail is self-contained; M8 ships with the
// lucide-react set already pinned at 0.525 (T-A1 dependency contract).
import { MapPin, Hexagon, Route, Library, Network, Cpu, Wrench, BarChart3, ClipboardCheck, Settings, Keyboard } from 'lucide-react'
