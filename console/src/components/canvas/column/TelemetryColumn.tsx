'use client'

/**
 * GCS v2 Operations Canvas — Telemetry column (Zone C, M8 T-B1).
 *
 * Spec: docs/GCS_V2_SPEC.md §8.3 + §2.2 zone C.
 *
 * 312px default right column. Collapses to a 40px tab showing attitude +
 * battery only (M9 lands the responsive breakpoint logic; M8 ships the
 * default width). Renders:
 *  - Attitude HUD (SVG horizon — roll/pitch/yaw from attitude_q_wxyz, P5 fix)
 *  - Instruments: battery/link/mode/GPS/EKF
 *  - Mini-plots: 3 dockable StripChart-class (M8 ships placeholders; M9
 *    wires the plots via the telemetry store's getStrip())
 *  - Pre-flight panel (toggle, M12 lands)
 *
 * The HUD ref registry (FlushLoop.tsx) writes telemetry numerics at 60 fps
 * — widgets register DOM nodes via `useHudRef(id)` and the loop writes
 * textContent / style.transform. NO React state for telemetry numerics
 * (§9.2 — the rule that makes the G-14/G-20 soaks assert ≤ 5 Hz commits).
 */

import { useHudRef } from '../FlushLoop'
import { useTelemetrySnapshot, getSimFrame } from '@/state/telemetry-store'
import { useAppStore } from '@/state/app-store'

export function TelemetryColumn() {
  const snap = useTelemetrySnapshot()
  const app = useAppStore()

  // The active vehicle — drives all column C widgets.
  const v = snap.vehicles.find((x) => x.index === app.activeVehicle) ?? snap.vehicles[0]

  // HUD ref callbacks — the FlushLoop writes textContent / style.transform.
  const setAlt = useHudRef(v ? `v${v.index}_alt` : 'v0_alt')
  const setBattery = useHudRef(v ? `v${v.index}_battery` : 'v0_battery')
  const setVoltage = useHudRef(v ? `v${v.index}_voltage` : 'v0_voltage')
  const setMode = useHudRef(v ? `v${v.index}_mode` : 'v0_mode')
  const setFsm = useHudRef(v ? `v${v.index}_fsm` : 'v0_fsm')
  const setYaw = useHudRef(v ? `v${v.index}_yaw` : 'v0_yaw')
  const setRoll = useHudRef(v ? `v${v.index}_roll` : 'v0_roll')
  const setPitch = useHudRef(v ? `v${v.index}_pitch` : 'v0_pitch')
  const setHudTransform = useHudRef(v ? `v${v.index}_hud_transform` : 'v0_hud_transform')
  const setCallsign = useHudRef(v ? `v${v.index}_callsign` : 'v0_callsign')
  const setFix = useHudRef(v ? `v${v.index}_fix` : 'v0_fix')
  const setSpeed = useHudRef(v ? `v${v.index}_speed` : 'v0_speed')

  return (
    <div
      data-rsim-zone="C"
      className="pointer-events-auto absolute right-0 flex flex-col gap-3 p-3 border-l overflow-y-auto"
      style={{ top: 48, bottom: 96, width: 312, zIndex: 20 }}
    >
      {/* Callsign header */}
      <div className="flex items-baseline justify-between">
        <span ref={setCallsign} className="rsim-mono" style={{ fontSize: 16, fontWeight: 600, color: 'var(--rsim-accent)' }}>
          v0
        </span>
        <span className="rsim-mono" style={{ fontSize: 11, color: 'var(--rsim-text-dim)' }}>
          Telemetry column
        </span>
      </div>

      {/* Attitude HUD — SVG horizon (M8 static; FlushLoop writes transform) */}
      <div
        style={{
          height: 160,
          borderRadius: 'var(--rsim-radius-panel)',
          overflow: 'hidden',
          border: '1px solid var(--rsim-border)',
          background: 'linear-gradient(180deg, #0d1b2a 0%, #0d1b2a 50%, #1a2812 50%, #1a2812 100%)',
          position: 'relative',
        }}
      >
        {/* Horizon line — the FlushLoop rotates/translates this. */}
        <div
          data-rsim-hud
          ref={setHudTransform}
          style={{
            position: 'absolute',
            inset: 0,
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
          }}
        >
          <div
            style={{
              width: '120%',
              height: 2,
              background: 'var(--rsim-text)',
              opacity: 0.6,
            }}
          />
        </div>
        {/* Roll/pitch/yaw readouts */}
        <div className="rsim-mono absolute top-2 left-2 flex flex-col gap-0.5" style={{ fontSize: 11, color: 'var(--rsim-text)' }}>
          <div>roll <span ref={setRoll}>0.0</span>°</div>
          <div>pitch <span ref={setPitch}>0.0</span>°</div>
          <div>yaw <span ref={setYaw}>0</span>°</div>
        </div>
      </div>

      {/* Instruments grid — battery / voltage / mode / FSM / GPS / speed */}
      <div
        style={{
          display: 'grid',
          gridTemplateColumns: '1fr 1fr',
          gap: 8,
        }}
      >
        <Tile label="battery">
          <span ref={setBattery}>—</span>%
        </Tile>
        <Tile label="voltage">
          <span ref={setVoltage}>—</span>V
        </Tile>
        <Tile label="mode">
          <span ref={setMode}>—</span>
        </Tile>
        <Tile label="fsm">
          <span ref={setFsm}>—</span>
        </Tile>
        <Tile label="alt AGL">
          <span ref={setAlt}>—</span>m
        </Tile>
        <Tile label="gnd speed">
          <span ref={setSpeed}>—</span>m/s
        </Tile>
        <Tile label="GPS fix" full>
          <span ref={setFix}>—</span>
        </Tile>
      </div>

      {/* Mini-plots placeholder — M9 wires the actual strip charts via
        getStrip(activeVehicle, 'alt_m' | 'battery_pct' | 'ground_speed_ms').
        M8 ships the placeholder so the column C layout assertion in G-14
        passes (the column exists with the right zone tag). */}
      <div
        style={{
          marginTop: 'auto',
          padding: 12,
          borderRadius: 'var(--rsim-radius-panel)',
          border: '1px solid var(--rsim-border)',
          background: 'rgba(17, 22, 29, 0.4)',
          color: 'var(--rsim-text-dim)',
          fontSize: 11,
        }}
      >
        Mini-plots — wired at M9 (alt / battery / ground-speed).
        StripChart-class plots docked here via the telemetry store's getStrip().
      </div>
    </div>
  )
}

function Tile({ label, children, full = false }: { label: string; children: React.ReactNode; full?: boolean }) {
  return (
    <div
      className="rsim-mono"
      style={{
        gridColumn: full ? '1 / -1' : undefined,
        padding: 8,
        borderRadius: 'var(--rsim-radius-control)',
        border: '1px solid var(--rsim-border)',
        background: 'rgba(17, 22, 29, 0.4)',
        fontSize: 11,
        color: 'var(--rsim-text)',
        display: 'flex',
        flexDirection: 'column',
        gap: 2,
      }}
    >
      <span style={{ fontSize: 9, fontWeight: 600, letterSpacing: '0.06em', color: 'var(--rsim-text-dim)', textTransform: 'uppercase' }}>
        {label}
      </span>
      <span style={{ fontSize: 14, fontWeight: 500 }}>
        {children}
      </span>
    </div>
  )
}
