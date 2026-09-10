'use client'

/**
 * GCS v2 Operations Canvas — ShortcutsProvider (M9, T-C1).
 *
 * Spec: docs/GCS_V2_SPEC.md §7.3 (Keyboard map) + §7.3.2 (guards) +
 * §7.3.3 (cheat sheet) + §7.3.4 (accessibility floor) + §13.1 stream C.
 *
 * One `window.addEventListener('keydown', handler, true)` (capture phase).
 * The handler SKIPS when (§7.3.2 — binding):
 *  - `e.target.closest('input, textarea, select, [contenteditable="true"],
 *    [role="slider"]')` (the Analyze scrubber's arrow keys must not double-fire;
 *    the param-search field must not trigger verbs).
 *  - any Radix menu/dialog is open (shortcut registry checks open-state).
 *  - a text-selection drag is in progress.
 * `preventDefault` + `stopPropagation` for handled keys. This mirrors the
 * Gmail/GitHub pattern and avoids MP's "hotkeys eat the param search" bug.
 *
 * §7.3.4 WCAG 2.1.4: single-key verbs get a disable-only toggle — Settings →
 * "Keyboard shortcuts: on/off", persisted in `localStorage`; when off, only
 * `?`/`Esc` remain live (so the toggle can be found again and re-enabled).
 *
 * Keymap (§7.3.1 — fixed, no customization):
 *   1-9    select vehicle N
 *   [ ]    cycle vehicle selection
 *   G      goto mode toggle
 *   A      ARM (hold-to-confirm 400ms)
 *   D      disarm (tap)
 *   T      takeoff (alt chip → hold-to-confirm)
 *   L      land (tap)
 *   R      RTL (tap)
 *   H      hold (tap)
 *   E      FLEET E-STOP (tap, immediate, fires on keyup)
 *   M      toggle mission strip overlay
 *   P      toggle Plan mode
 *   F      toggle follow camera
 *   C      re-center on active vehicle
 *   V      toggle NED inset view
 *   B      toggle Fleet C2 overlay
 *   S      toggle Setup drawer
 *   Y      toggle Analyze overlay
 *   N      cycle basemap
 *   X      pitch glance 0↔55°
 *   Z      reset camera (north-up / pitch 0 / re-fit fence)
 *   ?      cheat-sheet overlay
 *   Esc    cancel mode / close top-most overlay / clear selection
 *   F8     notifications panel toggle
 *
 * `E` fires on keyup (§7.3.1 — the one verb where speed beats confirmation);
 * all other verbs fire on keydown.
 */

import { useEffect, useRef, useState, type JSX } from 'react'
import {
  useAppStore,
  setActiveVehicle,
  setMapMode,
  toggleOverlay,
  setFollow,
  setGotoPending,
  setNedInset,
  type MapMode,
  type OverlayId,
} from '@/state/app-store'
import {
  command,
  guardVerb,
  isHoldConfirm,
  isBusy,
  HOLD_CONFIRM_MS,
  type CommandName,
} from '@/state/command-bus'
import { getFleetSnapshot, getVehicleCount } from '@/state/telemetry-store'

// ---------------------------------------------------------------------------
// Settings: shortcuts-disable toggle (§7.3.4 — WCAG 2.1.4 compliance path).
// Persisted in localStorage; when off, only ?/Esc remain live.
// ---------------------------------------------------------------------------

const SHORTCUTS_STORAGE_KEY = 'rsim.settings.v1'

interface Settings {
  shortcutsEnabled: boolean
}

function loadSettings(): Settings {
  if (typeof window === 'undefined') return { shortcutsEnabled: true }
  try {
    const raw = localStorage.getItem(SHORTCUTS_STORAGE_KEY)
    if (!raw) return { shortcutsEnabled: true }
    const parsed = JSON.parse(raw) as Partial<Settings>
    return { shortcutsEnabled: parsed.shortcutsEnabled ?? true }
  } catch {
    return { shortcutsEnabled: true }
  }
}

function saveSettings(s: Settings): void {
  if (typeof window === 'undefined') return
  try {
    localStorage.setItem(SHORTCUTS_STORAGE_KEY, JSON.stringify(s))
  } catch {
    // localStorage may be unavailable (private mode); non-fatal
  }
}

// ---------------------------------------------------------------------------
// Hold-to-confirm state — ARM/TAKEOFF/START MISSION/START FLEET/airframe-apply
// require a 400 ms press-and-hold (§7.4). The verb button component also
// implements this visually (amber progress fill); the key handler mirrors it.
// ---------------------------------------------------------------------------

interface HoldState {
  name: CommandName
  vehicle: number | undefined
  startedAt: number
  timer: ReturnType<typeof setTimeout>
  fired: boolean
}

// ---------------------------------------------------------------------------
// The guard selector — §7.3.2. Skips the handler when the user is typing in
// a text input / textarea / select / contenteditable / slider, so the goto
// alt chip, param search, and Analyze scrubber don't double-fire verbs.
// ---------------------------------------------------------------------------

function shouldSkipShortcut(e: KeyboardEvent): boolean {
  const target = e.target as HTMLElement | null
  if (!target) return false
  // Input / textarea / select / contenteditable / slider — Gmail/GitHub pattern.
  if (target.closest('input, textarea, select, [contenteditable="true"], [role="slider"]')) {
    return true
  }
  return false
}

// ---------------------------------------------------------------------------
// The provider — mounts the global key handler + the cheat-sheet dialog.
// ---------------------------------------------------------------------------

export function ShortcutsProvider(): JSX.Element | null {
  const app = useAppStore()
  const holdRef = useRef<HoldState | null>(null)
  // Lazy-init both the settings ref (for the key handler) and the state
  // (for the cheat-sheet dialog render) from localStorage. The ref is
  // never read during render; the state is the render-time source.
  const settingsRef = useRef<Settings | null>(null)
  if (settingsRef.current === null) {
    settingsRef.current = loadSettings()
  }
  const [shortcutsEnabled, setShortcutsEnabled] = useState(() => loadSettings().shortcutsEnabled)

  // Re-sync from localStorage when the settings overlay toggles (M15 ships
  // the Settings panel that writes the toggle; for M8/M9 the toggle is
  // always on). The ref is updated inside the effect (not during render).
  useEffect(() => {
    const s = loadSettings()
    settingsRef.current = s
    // eslint-disable-next-line react-hooks/set-state-in-effect -- legitimate external-state sync (localStorage → React state)
    setShortcutsEnabled(s.shortcutsEnabled)
  }, [app.overlays.settings])

  // The global key handler — capture phase, single listener.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      // Esc always works (even when shortcuts are disabled — §7.3.4).
      if (e.key === 'Escape') {
        // Esc: cancel mode / close top-most overlay / clear selection.
        if (holdRef.current) {
          clearTimeout(holdRef.current.timer)
          holdRef.current = null
        }
        if (app.gotoPending) {
          setGotoPending(false)
        } else {
          // Close top-most overlay (reverse priority order).
          const overlayOrder: OverlayId[] = ['cheat', 'settings', 'preflight', 'analyze', 'setup', 'fleet', 'library']
          for (const id of overlayOrder) {
            if (app.overlays[id]) {
              toggleOverlay(id, { force: false })
              e.preventDefault()
              e.stopPropagation()
              return
            }
          }
        }
        e.preventDefault()
        e.stopPropagation()
        return
      }

      // ? (Shift+/) always works (so the toggle can be found and re-enabled).
      if (e.key === '?' || (e.shiftKey && e.key === '/')) {
        toggleOverlay('cheat')
        e.preventDefault()
        e.stopPropagation()
        return
      }

      // F8 — notifications panel toggle (kept for v1 compat; M8 doesn't ship
      // a notifications panel — they're the stack in zone G — but the key is
      // reserved and discoverable in the cheat sheet).
      if (e.key === 'F8') {
        // No-op for M8 (notifications are always visible in zone G); the
        // cheat sheet lists it for forward compat with M11's event rail.
        e.preventDefault()
        e.stopPropagation()
        return
      }

      // If shortcuts are disabled (§7.3.4), bail after the Esc/?/F8 path.
      if (!settingsRef.current?.shortcutsEnabled) return

      if (shouldSkipShortcut(e)) return

      const key = e.key
      const activeVehicle = app.activeVehicle

      // --- Vehicle selection (1-9, [ ]) --------------------------------
      if (/^[1-9]$/.test(key)) {
        const n = parseInt(key, 10) - 1
        if (n < getVehicleCount()) {
          setActiveVehicle(n)
          e.preventDefault()
          e.stopPropagation()
        }
        return
      }
      if (key === '[' || key === ']') {
        const count = getVehicleCount()
        if (count === 0) return
        const dir = key === '[' ? -1 : 1
        const next = (activeVehicle + dir + count) % count
        setActiveVehicle(next)
        e.preventDefault()
        e.stopPropagation()
        return
      }

      // --- Map modes + overlays (single-key toggles) -------------------
      if (key === 'f' || key === 'F') {
        // F: toggle follow camera (§7.3.1). Note: spec §7.3.1 says `F` is
        // "toggle follow camera"; the map-mode switch uses the rail buttons.
        // (The §13.1 cheat sheet lists F = follow, not Fly mode.)
        setFollow(!app.follow)
        e.preventDefault()
        e.stopPropagation()
        return
      }
      if (key === 'p' || key === 'P') {
        setMapMode(app.mapMode === 'plan' ? 'fly' : 'plan')
        e.preventDefault()
        e.stopPropagation()
        return
      }
      if (key === 'm' || key === 'M') {
        toggleOverlay('mission') // §7.3.1 M: toggle mission strip overlay
        e.preventDefault()
        e.stopPropagation()
        return
      }
      if (key === 'b' || key === 'B') {
        toggleOverlay('fleet')
        e.preventDefault()
        e.stopPropagation()
        return
      }
      if (key === 's' || key === 'S') {
        toggleOverlay('setup')
        e.preventDefault()
        e.stopPropagation()
        return
      }
      if (key === 'y' || key === 'Y') {
        toggleOverlay('analyze')
        e.preventDefault()
        e.stopPropagation()
        return
      }

      // --- Goto flow (G) -----------------------------------------------
      if (key === 'g' || key === 'G') {
        setGotoPending(!app.gotoPending)
        e.preventDefault()
        e.stopPropagation()
        return
      }

      // --- Camera verbs (Z/X/C/N) — §6.5 + §7.3.1 ----------------------
      // Z: reset camera (north-up, pitch 0, re-fit fence)
      // X: pitch glance 0↔55°
      // C: re-center on active vehicle
      // N: cycle basemap (the ladder §6.3)
      // V: toggle NED inset view
      if (key === 'z' || key === 'Z') {
        // Reset — trigger the map's resetCamera via a custom event.
        window.dispatchEvent(new CustomEvent('rsim:camera-reset'))
        e.preventDefault()
        e.stopPropagation()
        return
      }
      if (key === 'x' || key === 'X') {
        window.dispatchEvent(new CustomEvent('rsim:camera-pitch-glance'))
        e.preventDefault()
        e.stopPropagation()
        return
      }
      if (key === 'c' || key === 'C') {
        window.dispatchEvent(new CustomEvent('rsim:camera-recenter'))
        e.preventDefault()
        e.stopPropagation()
        return
      }
      if (key === 'n' || key === 'N') {
        window.dispatchEvent(new CustomEvent('rsim:cycle-basemap'))
        e.preventDefault()
        e.stopPropagation()
        return
      }
      if (key === 'v' || key === 'V') {
        setNedInset(!app.nedInset)
        e.preventDefault()
        e.stopPropagation()
        return
      }

      // --- Flight verbs (A/D/T/L/R/H) ----------------------------------
      // ARM/TAKEOFF use hold-to-confirm (400ms); the rest are single-tap.
      if (key === 'a' || key === 'A') {
        triggerVerb('arm', activeVehicle, holdRef, e)
        return
      }
      if (key === 'd' || key === 'D') {
        triggerVerb('disarm', activeVehicle, holdRef, e)
        return
      }
      if (key === 't' || key === 'T') {
        triggerVerb('takeoff', activeVehicle, holdRef, e, { alt_m: 5 })
        return
      }
      if (key === 'l' || key === 'L') {
        triggerVerb('land', activeVehicle, holdRef, e)
        return
      }
      if (key === 'r' || key === 'R') {
        triggerVerb('rtl', activeVehicle, holdRef, e)
        return
      }
      if (key === 'h' || key === 'H') {
        triggerVerb('hold', activeVehicle, holdRef, e)
        return
      }

      // E-stop is handled on KEYUP (§7.3.1 — the one verb where speed beats
      // confirmation). The keydown handler just prevents auto-repeat.
      if (key === 'e' || key === 'E') {
        e.preventDefault()
        e.stopPropagation()
        return
      }
    }

    const onKeyUp = (e: KeyboardEvent) => {
      // E-stop fires on keyup (§7.3.1 — fires on keyup).
      if (e.key === 'e' || e.key === 'E') {
        if (!settingsRef.current?.shortcutsEnabled) return
        if (shouldSkipShortcut(e)) return
        // E-stop is always enabled (guardVerb returns enabled for estop_fleet).
        void command('estop_fleet', undefined, {})
        e.preventDefault()
        e.stopPropagation()
        return
      }
      // Hold-to-confirm: if the key is released before 400ms, cancel.
      if (holdRef.current) {
        // Map the released key back to the verb and cancel the hold.
        const key = e.key.toLowerCase()
        const holdVerb = holdRef.current.name
        const keyForVerb: Record<string, CommandName> = {
          a: 'arm',
          t: 'takeoff',
        }
        if (keyForVerb[key] === holdVerb) {
          clearTimeout(holdRef.current.timer)
          holdRef.current = null
        }
      }
    }

    window.addEventListener('keydown', onKeyDown, true) // capture phase
    window.addEventListener('keyup', onKeyUp, true)
    return () => {
      window.removeEventListener('keydown', onKeyDown, true)
      window.removeEventListener('keyup', onKeyUp, true)
      if (holdRef.current) {
        clearTimeout(holdRef.current.timer)
        holdRef.current = null
      }
    }
  }, [app])

  // Render the cheat-sheet dialog when the 'cheat' overlay is open.
  if (app.overlays.cheat) {
    return <CheatSheetDialog onClose={() => toggleOverlay('cheat', { force: false })} shortcutsEnabled={shortcutsEnabled} />
  }
  return null
}

// ---------------------------------------------------------------------------
// triggerVerb — centralizes the hold-to-confirm vs single-tap logic.
// ---------------------------------------------------------------------------

function triggerVerb(
  name: CommandName,
  vehicle: number | undefined,
  holdRef: React.MutableRefObject<HoldState | null>,
  e: KeyboardEvent,
  payload: Record<string, unknown> = {},
): void {
  const guard = guardVerb(name, vehicle)
  if (!guard.enabled) {
    // Don't fire — the verb button shows the reason tooltip.
    return
  }
  if (isBusy(name)) return

  if (isHoldConfirm(name)) {
    // Start the hold-to-confirm timer (400ms). If the key is released
    // before 400ms (onKeyUp cancels), the verb doesn't fire. The verb
    // button component renders the amber progress fill.
    const timer = setTimeout(() => {
      if (holdRef.current) {
        holdRef.current.fired = true
        void command(name, vehicle, payload)
        holdRef.current = null
      }
    }, HOLD_CONFIRM_MS)
    holdRef.current = { name, vehicle, startedAt: Date.now(), timer, fired: false }
  } else {
    // Single-tap verb — fire immediately.
    void command(name, vehicle, payload)
  }
  e.preventDefault()
  e.stopPropagation()
}

// ---------------------------------------------------------------------------
// Cheat-sheet dialog (§7.3.3) — two-column kbd table grouped [Selection |
// Flight | Camera | Modes | Panels | Replay], each row <kbd> chip + verb +
// the backend endpoint (tiny, dim — operators learn the wire truth).
// ---------------------------------------------------------------------------

interface CheatRow {
  key: string
  verb: string
  endpoint: string
}

const CHEAT_GROUPS: { title: string; rows: CheatRow[] }[] = [
  {
    title: 'Selection',
    rows: [
      { key: '1-9', verb: 'select vehicle N', endpoint: '—' },
      { key: '[ ]', verb: 'cycle vehicle selection', endpoint: '—' },
    ],
  },
  {
    title: 'Flight',
    rows: [
      { key: 'A', verb: 'ARM (hold 400ms)', endpoint: 'POST /api/vehicles/{i}/arm {arm:true}' },
      { key: 'D', verb: 'disarm', endpoint: 'POST /api/vehicles/{i}/arm {arm:false}' },
      { key: 'T', verb: 'takeoff (alt chip → hold)', endpoint: 'POST /api/vehicles/{i}/takeoff {alt_m}' },
      { key: 'L', verb: 'land', endpoint: 'POST /api/vehicles/{i}/land' },
      { key: 'R', verb: 'RTL', endpoint: 'POST /api/vehicles/{i}/rtl' },
      { key: 'H', verb: 'hold', endpoint: 'POST /api/vehicles/{i}/hold' },
      { key: 'E', verb: 'FLEET E-STOP (keyup, immediate)', endpoint: 'POST /api/fleet/estop' },
      { key: 'G', verb: 'goto mode toggle', endpoint: '→ click → POST /api/vehicles/{i}/goto' },
    ],
  },
  {
    title: 'Camera',
    rows: [
      { key: 'F', verb: 'toggle follow camera', endpoint: '—' },
      { key: 'C', verb: 're-center on active vehicle', endpoint: '—' },
      { key: 'V', verb: 'toggle NED inset view', endpoint: '—' },
      { key: 'X', verb: 'pitch glance 0↔55°', endpoint: '—' },
      { key: 'Z', verb: 'reset camera (north-up / pitch 0 / re-fit fence)', endpoint: '—' },
      { key: 'N', verb: 'cycle basemap', endpoint: '—' },
    ],
  },
  {
    title: 'Modes',
    rows: [
      { key: 'P', verb: 'toggle Plan mode', endpoint: '—' },
      { key: 'M', verb: 'toggle mission strip overlay', endpoint: '—' },
    ],
  },
  {
    title: 'Panels',
    rows: [
      { key: 'B', verb: 'toggle Fleet C2 overlay', endpoint: '—' },
      { key: 'S', verb: 'toggle Setup drawer', endpoint: '—' },
      { key: 'Y', verb: 'toggle Analyze overlay', endpoint: '—' },
      { key: '?', verb: 'this cheat sheet', endpoint: '—' },
      { key: 'Esc', verb: 'cancel mode / close overlay / clear selection', endpoint: '—' },
      { key: 'F8', verb: 'notifications panel (M11)', endpoint: '—' },
    ],
  },
  {
    title: 'Map',
    rows: [
      { key: 'N', verb: 'cycle basemap (Street → Satellite → Hybrid → Terrain → Offline)', endpoint: '—' },
      { key: 'X', verb: 'toggle 2D ↔ 3D Pitch (mirror of Settings → Map → Projection)', endpoint: '—' },
      { key: 'Z', verb: 'reset camera (north-up, pitch 0, re-fit fence)', endpoint: '—' },
      { key: 'C', verb: 're-center on active vehicle', endpoint: '—' },
      { key: 'F', verb: 'toggle follow camera', endpoint: '—' },
      { key: 'V', verb: 'toggle NED inset view', endpoint: '—' },
    ],
  },
]

function CheatSheetDialog({ onClose, shortcutsEnabled }: { onClose: () => void; shortcutsEnabled: boolean }): JSX.Element {
  return (
    <div
      role="dialog"
      aria-label="Keyboard shortcuts cheat sheet"
      aria-modal="true"
      style={{
        position: 'fixed',
        inset: 0,
        zIndex: 100,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        background: 'rgba(0, 0, 0, 0.6)',
      }}
      onClick={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div
        style={{
          background: 'var(--rsim-surface-solid)',
          border: '1px solid var(--rsim-border)',
          borderRadius: 'var(--rsim-radius-panel)',
          boxShadow: 'var(--rsim-shadow-overlay)',
          maxWidth: 760,
          width: '90vw',
          maxHeight: '80vh',
          overflow: 'auto',
          padding: 24,
          fontFamily: 'var(--rsim-font-ui)',
          color: 'var(--rsim-text)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 16 }}>
          <h2 style={{ fontSize: 16, fontWeight: 600, color: 'var(--rsim-text)' }}>Keyboard shortcuts</h2>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close cheat sheet"
            style={{
              background: 'transparent',
              border: '1px solid var(--rsim-border)',
              borderRadius: 'var(--rsim-radius-control)',
              color: 'var(--rsim-text-dim)',
              cursor: 'pointer',
              padding: '4px 10px',
              fontSize: 12,
            }}
          >
            Esc
          </button>
        </div>
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: '1fr 1fr',
            gap: 24,
            fontFamily: 'var(--rsim-font-mono)',
            fontSize: 12,
          }}
        >
          {CHEAT_GROUPS.map((group) => (
            <div key={group.title}>
              <div
                style={{
                  fontSize: 10,
                  fontWeight: 700,
                  letterSpacing: '0.08em',
                  textTransform: 'uppercase',
                  color: 'var(--rsim-accent)',
                  marginBottom: 8,
                  borderBottom: '1px solid var(--rsim-border)',
                  paddingBottom: 4,
                }}
              >
                {group.title}
              </div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
                {group.rows.map((row) => (
                  <div key={row.key} style={{ display: 'flex', flexDirection: 'column', gap: 1 }}>
                    <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                      <kbd
                        style={{
                          display: 'inline-block',
                          minWidth: 32,
                          padding: '2px 6px',
                          border: '1px solid var(--rsim-border)',
                          borderRadius: 'var(--rsim-radius-chip)',
                          background: 'rgba(17, 22, 29, 0.6)',
                          color: 'var(--rsim-text)',
                          fontSize: 11,
                          textAlign: 'center',
                          fontFamily: 'var(--rsim-font-mono)',
                        }}
                      >
                        {row.key}
                      </kbd>
                      <span style={{ color: 'var(--rsim-text)' }}>{row.verb}</span>
                    </div>
                    {row.endpoint !== '—' && (
                      <div style={{ color: 'var(--rsim-text-dim)', fontSize: 10, marginLeft: 40 }}>
                        {row.endpoint}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
        <div
          style={{
            marginTop: 16,
            paddingTop: 12,
            borderTop: '1px solid var(--rsim-border)',
            fontSize: 11,
            color: 'var(--rsim-text-dim)',
            display: 'flex',
            justifyContent: 'space-between',
            alignItems: 'center',
          }}
        >
          <span>
            Shortcuts: <strong style={{ color: shortcutsEnabled ? 'var(--rsim-ok)' : 'var(--rsim-danger)' }}>{shortcutsEnabled ? 'ON' : 'OFF'}</strong>
            {' '}
            (WCAG 2.1.4 — Settings panel toggle lands M15)
          </span>
          <span>press ? anytime to open this sheet</span>
        </div>
      </div>
    </div>
  )
}
