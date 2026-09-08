/**
 * SIMULATED vehicle-setup engine (ADR-0016 plane, backend offline).
 *
 * Mirrors the live mavfleet setup plane closely enough to demo the QGC/MP
 * workflow: same catalog shape (a curated subset of the ROMFS catalog),
 * same mode set, a small param store with the exact setup-relevant ids,
 * and the same write/apply/calibrate semantics (echo-confirmed writes,
 * armed gates, restarts flip SYS_AUTOSTART + "reboot" with fresh params).
 */

import type {
  AirframeGroup,
  CalSensor,
  ModeEntry,
  ParamEntry,
  ParamStoreView,
  VehicleSetupSummary,
} from './types'

/** Curated subset of the real ROMFS catalog (same categories/order). */
export const MOCK_AIRFRAME_GROUPS: AirframeGroup[] = [
  {
    category: 'Multirotor (UAV)',
    airframes: [
      { id: 4001, name: 'Generic Quadcopter', frame_type: 'Quadrotor x', sim_model: null },
      { id: 4019, name: 'Holybro X500 V2', frame_type: 'Quadrotor x', sim_model: null },
      { id: 5001, name: 'Generic Quad + geometry', frame_type: 'Quadrotor +', sim_model: null },
      { id: 6001, name: 'Generic Hexarotor x geometry', frame_type: 'Hexarotor x', sim_model: null },
    ],
  },
  {
    category: 'Fixed Wing (UAV)',
    airframes: [
      { id: 2100, name: 'Generic Standard Plane', frame_type: 'Standard Plane', sim_model: null },
      { id: 3000, name: 'Generic Flying Wing', frame_type: 'Flying Wing', sim_model: null },
    ],
  },
  {
    category: 'VTOL (UAV)',
    airframes: [
      { id: 13000, name: 'Generic Standard VTOL', frame_type: 'Standard VTOL', sim_model: null },
      { id: 13200, name: 'Generic VTOL Tailsitter', frame_type: 'VTOL Tailsitter', sim_model: null },
    ],
  },
  {
    category: 'Rover (UGV)',
    airframes: [
      { id: 50000, name: 'Generic Rover Differential', frame_type: 'Rover', sim_model: null },
      { id: 51000, name: 'Generic Rover Ackermann', frame_type: 'Rover', sim_model: null },
    ],
  },
  {
    category: 'Boat (USV)',
    airframes: [{ id: 1070, name: 'Boat', frame_type: '', sim_model: 'gazebo-classic_boat' }],
  },
  {
    category: 'Underwater (UUV)',
    airframes: [
      { id: 60000, name: 'Generic Underwater Robot', frame_type: 'Underwater Robot', sim_model: null },
      { id: 60002, name: 'BlueROV2 (Heavy Configuration)', frame_type: 'Vectored 6 DOF UUV', sim_model: null },
    ],
  },
  {
    category: 'Simulator (SITL models)',
    airframes: [
      { id: 10015, name: '3DR Iris Quadrotor SITL', frame_type: 'Quadrotor Wide', sim_model: 'gazebo-classic_iris' },
      { id: 6011, name: 'Typhoon H480 SITL', frame_type: 'Hexarotor x', sim_model: 'gazebo-classic_typhoon_h480' },
      { id: 1022, name: 'BlueROV2 Heavy Configuration', frame_type: '', sim_model: 'gazebo-classic_uuv_bluerov2_heavy' },
    ],
  },
]

/** All catalog entries flattened. */
export const MOCK_AIRFRAMES = MOCK_AIRFRAME_GROUPS.flatMap((g) => g.airframes)

/** The switchable mode set (fleet-modes-verified words). */
export const MOCK_MODES: ModeEntry[] = [
  { name: 'MANUAL', mode_word: 131072 },
  { name: 'ALTCTL', mode_word: 196608 },
  { name: 'POSCTL', mode_word: 262144 },
  { name: 'STABILIZED', mode_word: 327680 },
  { name: 'ACRO', mode_word: 393216 },
  { name: 'AUTO.LOITER', mode_word: 50593792 },
  { name: 'AUTO.RTL', mode_word: 50662336 },
  { name: 'AUTO.LAND', mode_word: 50695424 },
  { name: 'AUTO.MISSION', mode_word: 50462720 },
  { name: 'OFFBOARD', mode_word: 34078720 },
]

const CATEGORY_BY_COMPAT = new Set(['Multirotor (UAV)', 'Simulator (SITL models)'])

/** The demo param table (ids the setup view actually reads/edits).
 *
 * Each entry is `[id, defaultValue, group]` — `value` is seeded equal to
 * `default` (so `is_changed=false` for the freshly-booted mock), and a
 * handful of rows are marked `changed=true` to exercise the
 * diff-against-defaults highlight (QGC §8.3 step 4: "highlighted in pale
 * yellow"). */
function freshMockParams(sysAutostart: number): ParamEntry[] {
  const rows: [string, number, string][] = [
    ['SYS_AUTOSTART', sysAutostart, 'SYS'],
    ['CAL_ACC0_ID', 1310988, 'CAL'],
    ['CAL_GYRO0_ID', 1310988, 'CAL'],
    ['CAL_MAG0_ID', 19700492, 'CAL'],
    ['CAL_MAG1_ID', 0, 'CAL'],
    ['CAL_MAG2_ID', 0, 'CAL'],
    ['CAL_LEVEL', 0, 'CAL'],
    ['BAT1_N_CELLS', 4, 'BAT'],
    ['BAT1_V_EMPTY', 3.1, 'BAT'],
    ['BAT1_V_CHARGED', 4.1, 'BAT'],
    ['BAT_LOW_THR', 0.15, 'BAT'],
    ['BAT_CRIT_THR', 0.07, 'BAT'],
    ['BAT_EMERGEN_THR', 0.05, 'BAT'],
    ['BAT1_R_INTERNAL', 0.008, 'BAT'],
    ['NAV_RCL_ACT', 3, 'NAV'],
    ['NAV_DLL_ACT', 0, 'NAV'],
    ['COM_LOW_BAT_ACT', 2, 'COM'],
    ['COM_OBL_RC_ACT', 3, 'COM'],
    ['GF_ACTION', 2, 'GF'],
    ['GF_MAX_HOR_DIST', 900, 'GF'],
    ['GF_MAX_VER_DIST', 150, 'GF'],
    ['RTL_RETURN_ALT', 60, 'RTL'],
    ['MC_ROLLRATE_MAX', 720, 'MC'],
    ['MC_ROLLRATE_P', 6.5, 'MC'],
    ['MC_PITCHRATE_P', 6.5, 'MC'],
    ['MPC_TILTMAX_AIR', 35, 'MPC'],
    ['MPC_XY_VEL_MAX', 12.0, 'MPC'],
    ['MPC_Z_VEL_MAX_UP', 3.0, 'MPC'],
    ['MPC_LAND_SPEED', 0.7, 'MPC'],
    ['COM_RC_LOSS_T', 0.5, 'COM'],
    ['LNDMC_ALT_MAX', 2, 'LND'],
  ]
  // tag a few rows as "changed from default" so the diff view has signal
  // even before the operator touches anything (simulates a vehicle that
  // already has a tuned config).
  const changed: Record<string, number> = {
    MC_ROLLRATE_P: 7.5,
    MPC_XY_VEL_MAX: 8.0,
    RTL_RETURN_ALT: 80,
  }
  return rows.map(([id, defaultValue, group], i) => {
    const value = changed[id] != null ? changed[id] : defaultValue
    return {
      id,
      value,
      raw: value,
      type: 9,
      kind: 'real32',
      index: i,
      group,
      default: defaultValue,
      is_changed: changed[id] != null,
    }
  })
}

/**
 * One simulated vehicle's setup state.
 */
export class MockSetupVehicle {
  readonly index: number
  readonly sysid: number
  fsm = 'READY'
  armed = false
  mode = 'POSCTL'
  modeWord = 262144
  batteryPct = 98
  voltageV = 16.2
  params: ParamEntry[] = freshMockParams(10015)
  downloadState: 'Idle' | 'Downloading' | 'Complete' = 'Complete'
  restartPending = false
  private tick = 0

  constructor(index: number) {
    this.index = index
    this.sysid = index + 1
  }

  /** Advance the demo world (call ~1 Hz). */
  step(): void {
    this.tick++
    this.batteryPct = Math.max(11, 98 - this.tick * 0.02)
    this.voltageV = 14.4 + (this.batteryPct / 100) * 1.8
    if (this.downloadState === 'Downloading') {
      // simulate the PARAM_VALUE burst completing ~1 s after request
      this.downloadState = 'Complete'
    }
    if (this.restartPending && this.tick % 4 === 0) {
      this.restartPending = false
      this.fsm = 'READY'
      this.mode = 'MANUAL'
      this.modeWord = 131072
    }
  }

  get(id: string): number | null {
    return this.params.find((p) => p.id === id)?.value ?? null
  }

  paramStoreView(): ParamStoreView {
    return {
      total: this.params.length,
      received: this.params.length,
      state: this.downloadState,
      requested_ms: null,
      last_value_ms: null,
      params: [...this.params],
    }
  }

  summary(): VehicleSetupSummary {
    const entry = MOCK_AIRFRAMES.find((a) => a.id === this.get('SYS_AUTOSTART'))
    const group = MOCK_AIRFRAME_GROUPS.find((g) =>
      g.airframes.some((a) => a.id === this.get('SYS_AUTOSTART')),
    )
    const pv = (id: string) => this.get(id)
    return {
      index: this.index,
      sysid: this.sysid,
      compid: 1,
      fsm: this.fsm,
      mode: this.mode,
      mode_word: this.modeWord,
      armed: this.armed,
      battery_pct: Math.round(this.batteryPct),
      voltage_v: this.voltageV,
      restart_pending: this.restartPending,
      autopilot: { type: 'PX4', version: 'v1.16.2 SITL' },
      airframe: {
        sys_autostart: pv('SYS_AUTOSTART'),
        name: entry?.name ?? 'Unknown airframe',
        frame_type: entry?.frame_type ?? '',
        category: group?.category ?? 'Other',
        dynamics_compatible: group ? CATEGORY_BY_COMPAT.has(group.category) : false,
      },
      params: {
        total: this.params.length,
        received: this.params.length,
        state: this.downloadState,
      },
      calibration: {
        accel: (pv('CAL_ACC0_ID') ?? 0) !== 0,
        gyro: (pv('CAL_GYRO0_ID') ?? 0) !== 0,
        mag0: (pv('CAL_MAG0_ID') ?? 0) !== 0,
        mag1: (pv('CAL_MAG1_ID') ?? 0) !== 0,
        mag2: (pv('CAL_MAG2_ID') ?? 0) !== 0,
        level_horizon: (pv('CAL_LEVEL') ?? 0) !== 0,
      },
      power: {
        BAT1_N_CELLS: pv('BAT1_N_CELLS'),
        BAT1_V_EMPTY: pv('BAT1_V_EMPTY'),
        BAT1_V_CHARGED: pv('BAT1_V_CHARGED'),
        BAT_LOW_THR: pv('BAT_LOW_THR'),
        BAT_CRIT_THR: pv('BAT_CRIT_THR'),
        BAT_EMERGEN_THR: pv('BAT_EMERGEN_THR'),
        BAT1_R_INTERNAL: pv('BAT1_R_INTERNAL'),
      },
      safety: {
        NAV_RCL_ACT: pv('NAV_RCL_ACT'),
        NAV_DLL_ACT: pv('NAV_DLL_ACT'),
        COM_LOW_BAT_ACT: pv('COM_LOW_BAT_ACT'),
        COM_OBL_RC_ACT: pv('COM_OBL_RC_ACT'),
        GF_ACTION: pv('GF_ACTION'),
        GF_MAX_HOR_DIST: pv('GF_MAX_HOR_DIST'),
        GF_MAX_VER_DIST: pv('GF_MAX_VER_DIST'),
        RTL_RETURN_ALT: pv('RTL_RETURN_ALT'),
      },
    }
  }

  // -- actions (same semantics as the live endpoints) ---------------------

  requestParamList(): boolean {
    this.downloadState = 'Downloading'
    return true
  }

  /** Echo-confirmed write: 'unknown' ids get created (PX4 would reject).
   *
   * After a write, `is_changed` is recomputed against the cached
   * `default` (null defaults → true once written, matching the
   * "operator-set" semantic of QGC). */
  writeParam(id: string, value: number): { ok: boolean; confirmed: number | null } {
    if (this.armed && id === 'SYS_AUTOSTART') {
      return { ok: false, confirmed: null }
    }
    const row = this.params.find((p) => p.id === id)
    if (row) {
      row.value = value
      row.raw = value
      row.is_changed = row.default == null ? true : Math.abs(row.default - value) > 1e-9
    } else {
      // newly-created param has no known default → mark as changed
      this.params.push({
        id,
        value,
        raw: value,
        type: 9,
        kind: 'real32',
        index: this.params.length,
        group: id.split('_')[0] ?? '',
        default: null,
        is_changed: true,
      })
    }
    return { ok: true, confirmed: value }
  }

  applyAirframe(id: number): { ok: boolean; write_confirmed: boolean; restart_queued: boolean } {
    if (this.armed) return { ok: false, write_confirmed: false, restart_queued: false }
    if (!MOCK_AIRFRAMES.some((a) => a.id === id)) return { ok: false, write_confirmed: false, restart_queued: false }
    this.writeParam('SYS_AUTOSTART', id)
    // fresh airframe defaults on reboot (SYS_AUTOCONFIG semantics)
    this.params = freshMockParams(id)
    this.restartPending = true
    this.fsm = 'BOOTING'
    return { ok: true, write_confirmed: true, restart_queued: true }
  }

  calibrate(sensor: CalSensor): { accepted: boolean; description: string } {
    if (this.armed) return { accepted: false, description: 'vehicle armed' }
    const map: Record<CalSensor, [string, string, number]> = {
      gyro: ['CAL_GYRO0_ID', 'gyroscope', 1310988],
      accel: ['CAL_ACC0_ID', 'accelerometer', 1310988],
      accel_quick: ['CAL_ACC0_ID', 'accelerometer (quick)', 1310988],
      mag: ['CAL_MAG0_ID', 'magnetometer', 19700492],
      level: ['CAL_LEVEL', 'level horizon', 1],
      baro: ['CAL_BAR0_ID', 'barometer (ground pressure)', 6620612],
      airspeed: ['CAL_AIRSP_CAB', 'airspeed', 0],
    }
    const [param, desc, id] = map[sensor]
    this.writeParam(param, id)
    return { accepted: true, description: desc }
  }

  setMode(name: string): boolean {
    const m = MOCK_MODES.find((x) => x.name === name)
    if (!m) return false
    this.mode = m.name
    this.modeWord = m.mode_word
    return true
  }

  // -- v1: param presets (in-memory mirror of the :8300 catalog) ------------

  /** In-memory preset store (per-vehicle). Replaced on airframe reboot. */
  private presets: Map<string, { created_at: string; params: { id: string; value: number; type: number }[] }> = new Map()

  listPresets(): { name: string; created_at: string; param_count: number }[] {
    return Array.from(this.presets.entries())
      .map(([name, p]) => ({ name, created_at: p.created_at, param_count: p.params.length }))
      .sort((a, b) => a.name.localeCompare(b.name))
  }

  savePreset(name: string, params: ParamEntry[]): number {
    const trimmed = name.trim()
    if (!trimmed) return 0
    this.presets.set(trimmed, {
      created_at: new Date().toISOString(),
      params: params.map((p) => ({ id: p.id, value: p.value, type: p.type })),
    })
    return params.length
  }

  loadPreset(name: string): { id: string; value: number; type: number }[] {
    const trimmed = name.trim()
    const p = this.presets.get(trimmed)
    return p ? p.params : []
  }

  deletePreset(name: string): boolean {
    return this.presets.delete(name.trim())
  }
}

/** The fleet-wide mock (2 vehicles, like the demo scenario). */
export class MockSetupEngine {
  vehicles: MockSetupVehicle[] = [new MockSetupVehicle(0), new MockSetupVehicle(1)]
  step(): void {
    for (const v of this.vehicles) v.step()
  }
}
